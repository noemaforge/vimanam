#!/usr/bin/env python3
"""Flag Lean models whose Rust changed since the model was last read.

formal/models.toml lists, for each model under formal/VimanamProofs/, the Rust
it models and `read_digest`, a fingerprint of that Rust when the model was
last read against it. This script recomputes each fingerprint and reports the
models whose Rust no longer matches. See formal/README.md, "Keeping the models
in step with the Rust".

A ref is a whole file (`src/diff/mod.rs`) or every item of one name in it
(`src/diff/mod.rs#severity`: `fn`, `enum`, `struct`, `trait`, `type`,
`const`, `static`, `mod`, `union` called that, and the `impl` blocks of a type
of that name). The fingerprint is SHA-256 over the refs, sorted, each with its
code: comments and `#[cfg(test)]` items blanked, whitespace collapsed outside
string and char literals. So a comment, formatting or test-only edit is not
drift; any other edit inside a ref is.

Usage (Python 3.11+, from anywhere in the repository):
  python3 -I scripts/formal/check-model-drift.py            # report, exit 0
  python3 -I scripts/formal/check-model-drift.py --strict   # exit 1 on drift
  python3 -I scripts/formal/check-model-drift.py --bump Severity
  python3 -I scripts/formal/check-model-drift.py --self-test

Exit codes: 0 no drift (or drift without --strict), 1 drift with --strict,
2 usage, 3 broken manifest (a model with no entry, a ref that does not
resolve, a malformed digest).
"""

from __future__ import annotations

import argparse
import hashlib
import os
import re
import subprocess
import sys
import tomllib
from pathlib import Path

SCHEME = "v1"
ROOT = Path(__file__).resolve().parents[2]
MANIFEST = ROOT / "formal" / "models.toml"
MODELS_DIR = ROOT / "formal" / "VimanamProofs"
# Model files that read no Rust of their own: the audit, the theorems over
# Severity.lean, and the history layer.
EXEMPT = {"Audit.lean", "Direction.lean", "History.lean"}
DIGEST_RE = re.compile(r"^v(\d+):sha256:[0-9a-f]{64}$")
ITEM_KINDS = r"(?:fn|enum|struct|trait|type|const|static|mod|union)"


class ManifestError(Exception):
    pass


def scan(src: str) -> tuple[str, list[bool]]:
    """Blank comments (each becomes one space) and mark literal characters.

    Returns the code and, per character, whether it is inside a string or
    char literal (quotes included), so later passes can ignore braces and
    keywords there.
    """
    out: list[str] = []
    lit: list[bool] = []
    i, n = 0, len(src)

    def emit(text: str, in_literal: bool) -> None:
        out.extend(text)
        lit.extend([in_literal] * len(text))

    while i < n:
        c = src[i]
        if src.startswith("//", i):
            j = src.find("\n", i)
            i = n if j < 0 else j
            emit(" ", False)
        elif src.startswith("/*", i):
            depth, i = 1, i + 2
            while i < n and depth:
                if src.startswith("/*", i):
                    depth, i = depth + 1, i + 2
                elif src.startswith("*/", i):
                    depth, i = depth - 1, i + 2
                else:
                    i += 1
            emit(" ", False)
        elif c == "r" and re.match(r'r#*"', src[i:]) and (i == 0 or not (src[i - 1].isalnum() or src[i - 1] == "_")):
            hashes = re.match(r"r(#*)\"", src[i:]).group(1)
            close = '"' + hashes
            j = src.find(close, i + 2 + len(hashes))
            j = n if j < 0 else j + len(close)
            emit(src[i:j], True)
            i = j
        elif c == '"':
            j = i + 1
            while j < n and src[j] != '"':
                j += 2 if src[j] == "\\" else 1
            emit(src[i : j + 1], True)
            i = j + 1
        elif c == "'":
            m = re.match(r"'(?:\\(?:x[0-9a-fA-F]{2}|u\{[0-9a-fA-F]+\}|.)|[^\\'])'", src[i:])
            if m:
                emit(m.group(0), True)
                i += len(m.group(0))
            else:  # a lifetime
                emit(c, False)
                i += 1
        else:
            emit(c, False)
            i += 1
    return "".join(out), lit


def item_end(code: str, lit: list[bool], start: int) -> int:
    """The end of the item starting at `start`: its matching `}`, or a `;`
    reached before any `{`."""
    depth = 0
    paren = 0
    for j in range(start, len(code)):
        if lit[j]:
            continue
        ch = code[j]
        if ch in "([":
            paren += 1
        elif ch in ")]":
            paren -= 1
        elif ch == "{":
            depth += 1
        elif ch == "}":
            depth -= 1
            if depth == 0:
                return j + 1
        elif ch == ";" and depth == 0 and paren == 0:
            return j + 1
    raise ManifestError("unbalanced braces")


def test_regions(code: str, lit: list[bool]) -> list[tuple[int, int]]:
    regions = []
    for m in re.finditer(r"#\s*\[\s*cfg\s*\(\s*test\s*\)\s*\]", code):
        if lit[m.start()]:
            continue
        regions.append((m.start(), item_end(code, lit, m.end())))
    return regions


def normalize(text: str, lit: list[bool]) -> str:
    out: list[str] = []
    space = False
    for ch, in_literal in zip(text, lit):
        if not in_literal and ch.isspace():
            space = True
            continue
        if space and out:
            out.append(" ")
        space = False
        out.append(ch)
    return "".join(out)


def git(*args: str) -> str | None:
    """Run git in the repository; its stdout, or None when it fails."""
    try:
        out = subprocess.run(["git", "-C", str(ROOT), *args], capture_output=True, check=True)
    except (OSError, subprocess.CalledProcessError):
        return None
    return out.stdout.decode("utf-8", errors="replace")


_GIT_TEXT: dict[tuple[str, str, str], str | None] = {}


def git_text(commit: str, path: str) -> str | None:
    """The file at `commit`, or None when git cannot give it."""
    key = (str(ROOT), commit, path)
    if key not in _GIT_TEXT:
        _GIT_TEXT[key] = git("show", f"{commit}:{path}")
    return _GIT_TEXT[key]


def commit_exists(commit: str) -> bool:
    return git("rev-parse", "--verify", "--quiet", f"{commit}^{{commit}}") is not None


def digest_at(refs: list[str], commit: str) -> tuple[str | None, str]:
    """The digest of `refs` at `commit`, or None and the reason it has none."""
    if not commit_exists(commit):
        return None, f"{commit} is not in this clone"
    try:
        return digest(refs, commit), ""
    except ManifestError as err:
        return None, f"at {commit}: {err}"


# How many commits that touched a model's Rust files to search for the state
# its digest was taken at, when `read_against` does not give it.
HISTORY_SEARCH = 100


def digest_state(entry: dict) -> tuple[str | None, str]:
    """A commit whose refs fingerprint to the entry's `read_digest`: its
    `read_against` when that works, else the newest such commit among the
    last HISTORY_SEARCH commits that touched its files (this survives a squash
    merge, whose commit has the bumped branch's tree). None, with the reason,
    when there is none."""
    refs = entry["rust"]
    reasons = []
    commit = entry.get("read_against")
    if commit:
        got, why = digest_at(refs, commit)
        if got == entry["read_digest"]:
            return commit, ""
        reasons.append(why or f"read_against {commit} is not the digest's state")
    else:
        reasons.append("no read_against")
    paths = sorted({ref.partition("#")[0] for ref in refs})
    log = git("log", f"-n{HISTORY_SEARCH}", "--format=%H", "HEAD", "--", *paths)
    candidates = (log or "").split()
    for candidate in candidates:
        got, _ = digest_at(refs, candidate)
        if got == entry["read_digest"]:
            return candidate[:12], ""
    searched = (
        f"the last {HISTORY_SEARCH} commits"
        if len(candidates) == HISTORY_SEARCH
        else f"the {len(candidates)} commit{'' if len(candidates) == 1 else 's'} in this clone"
    )
    reasons.append(f"none of {searched} that touched {', '.join(paths)} has the digest's state")
    return None, "; ".join(reasons)


def ref_code(ref: str, commit: str | None = None) -> str:
    """The normalised code of `ref` in the working tree, or at `commit`."""
    path, _, name = ref.partition("#")
    if commit is None:
        file = ROOT / path
        if not file.is_file():
            raise ManifestError(f"{ref}: no such file")
        text = file.read_text(encoding="utf-8")
    else:
        text = git_text(commit, path)
        if text is None:
            raise ManifestError(f"{ref}: not in {commit}")
    code, lit = scan(text)
    blank = test_regions(code, lit)

    def in_test(pos: int) -> bool:
        return any(a <= pos < b for a, b in blank)

    if not name:
        keep = [(i, ch) for i, ch in enumerate(code) if not in_test(i)]
        return normalize("".join(ch for _, ch in keep), [lit[i] for i, _ in keep])
    if not re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", name):
        raise ManifestError(f"{ref}: an item ref names one identifier")
    pattern = re.compile(
        rf"\b{ITEM_KINDS}\s+{name}\b|\bimpl\b(?:\s*<[^{{;]*?>)?\s+(?:[\w:<>, ]+\s+for\s+)?{name}\b"
    )
    pieces = []
    for m in pattern.finditer(code):
        if lit[m.start()] or in_test(m.start()):
            continue
        end = item_end(code, lit, m.start())
        pieces.append(normalize(code[m.start() : end], lit[m.start() : end]))
    if not pieces:
        raise ManifestError(f"{ref}: no item called {name}")
    return "\n".join(pieces)


def digest(refs: list[str], commit: str | None = None) -> str:
    h = hashlib.sha256()
    for ref in sorted(refs):
        h.update(ref.encode())
        h.update(b"\n")
        h.update(ref_code(ref, commit).encode())
        h.update(b"\n")
    return f"{SCHEME}:sha256:{h.hexdigest()}"


def load_manifest() -> list[dict]:
    try:
        data = tomllib.loads(MANIFEST.read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError) as err:
        raise ManifestError(f"{MANIFEST}: {err}") from err
    models = data.get("model", [])
    listed = set()
    for entry in models:
        for key in ("lean", "read_digest", "rust"):
            if key not in entry:
                raise ManifestError(f"an entry lacks `{key}`: {entry}")
        if "read_against" in entry and not re.fullmatch(r"[0-9a-f]{7,40}", entry["read_against"]):
            raise ManifestError(f"{entry['lean']}: malformed read_against")
        if not (ROOT / entry["lean"]).is_file():
            raise ManifestError(f"{entry['lean']}: no such model file")
        m = DIGEST_RE.match(entry["read_digest"])
        if not m:
            raise ManifestError(f"{entry['lean']}: malformed read_digest")
        if f"v{m.group(1)}" != SCHEME:
            raise ManifestError(f"{entry['lean']}: digest scheme v{m.group(1)}, this script is {SCHEME}")
        listed.add(Path(entry["lean"]).name)
    for model in sorted(MODELS_DIR.glob("*.lean")):
        if model.name not in EXEMPT and model.name not in listed:
            raise ManifestError(f"formal/VimanamProofs/{model.name} has no entry in formal/models.toml")
    return models


def bump(names: list[str]) -> None:
    text = MANIFEST.read_text(encoding="utf-8")
    models = load_manifest()
    for name in names:
        stem = Path(name).stem
        matches = [e for e in models if Path(e["lean"]).stem == stem]
        if not matches:
            raise ManifestError(f"no model called {name}")
        entry = matches[0]
        new = digest(entry["rust"])
        anchor, note = bump_anchor(entry["rust"], new)
        start = text.find(f'lean = "{entry["lean"]}"\n')
        end = text.find("rust = [", start)
        if start < 0 or end < 0:
            raise ManifestError(f"{entry['lean']}: cannot find its entry")
        head = text[start:end]
        head = re.sub(r'read_against = "[^"]*"\n', "", head)
        head, count = re.subn(r'read_digest = "[^"]*"', f'read_digest = "{new}"', head)
        if count != 1:
            raise ManifestError(f"{entry['lean']}: cannot find its read_digest line")
        if anchor:
            head = head.replace("read_digest = ", f'read_against = "{anchor}"\nread_digest = ', 1)
        text = text[:start] + head + text[end:]
        print(f"bumped {entry['lean']} to {new}; {note}")
    MANIFEST.write_text(text, encoding="utf-8")


def bump_anchor(refs: list[str], new: str) -> tuple[str | None, str]:
    """The commit `--bump` records as `read_against`: the merge-base with
    origin/main when its refs are the working tree's (it survives a squash
    merge), else HEAD when its refs are (the usual case after committing a
    Rust change on a branch). A branch commit can leave the history after a
    squash merge; the drift report then finds the state by searching history,
    so the anchor is a hint, not a requirement."""
    for label, rev in (("merge-base with origin/main", None), ("HEAD", "HEAD")):
        if rev is None:
            base = git("merge-base", "HEAD", "origin/main")
            if not base:
                continue
            rev = base.strip()
        sha = git("rev-parse", "--verify", "--quiet", f"{rev}^{{commit}}")
        if sha and digest_at(refs, sha.strip())[0] == new:
            return sha.strip()[:12], f"read_against set to {sha.strip()[:12]} ({label})"
    return None, (
        "read_against removed: no commit has these refs yet (commit the Rust, then bump "
        "again); until then a drift report searches history or lists every ref"
    )


def changed_refs(entry: dict) -> tuple[list[str], str]:
    """The refs that changed since the state `read_digest` was taken at
    (`digest_state`); every ref, with the reason, when that state is not
    found."""
    commit, why = digest_state(entry)
    if commit is None:
        return entry["rust"], f"; every ref is listed: {why}"
    changed = [ref for ref in entry["rust"] if ref_code(ref, commit) != ref_code(ref)]
    paths = " ".join(sorted({ref.partition("#")[0] for ref in changed}))
    return changed, f"; changed since {commit}, see git diff {commit} -- {paths}"


def check(strict: bool) -> int:
    models = load_manifest()
    drifted = []
    for entry in models:
        if digest(entry["rust"]) != entry["read_digest"]:
            drifted.append(entry)
    gha = os.environ.get("GITHUB_ACTIONS") == "true"
    for entry in drifted:
        stem = Path(entry["lean"]).stem
        changed, why = changed_refs(entry)
        msg = (
            f"{entry['lean']} has drifted: the Rust it models changed since it was last read "
            f"({', '.join(changed)}{why}). Re-read the model against the Rust, update it if "
            f"the change matters, then run: python3 -I scripts/formal/check-model-drift.py --bump {stem}"
        )
        if gha:
            for path in sorted({ref.partition("#")[0] for ref in changed}):
                print(f"::warning file={path}::{msg}")
        else:
            print(f"warning: {msg}", file=sys.stderr)
    if not drifted:
        print(f"{len(models)} models in step with the Rust")
    return 1 if drifted and strict else 0


def self_test() -> None:
    src = (
        "fn a() { let s = \"} // {\"; let c = '}'; } // tail\n"
        "#[cfg(test)]\nmod tests { fn a() {} }\n"
        "impl<'a> Foo for Bar<'a> { fn x(&self) {} }\n"
        "/* block /* nested */ */ enum Bar { X }\n"
    )
    code, lit = scan(src)
    assert "tail" not in code and "nested" not in code
    regions = test_regions(code, lit)
    assert len(regions) == 1 and "mod tests" in code[regions[0][0] : regions[0][1]]
    end = item_end(code, lit, code.index("fn a"))
    assert normalize(code[:end], lit[:end]) == "fn a() { let s = \"} // {\"; let c = '}'; }"
    self_test_bump()
    print("self-test passed")


def self_test_bump() -> None:
    """Bump, change one ref, check the report names only it; then bump again
    after a squash-merge-like rewrite (the anchor commit leaves history) and
    check the report still narrows."""
    import tempfile

    global ROOT, MANIFEST, MODELS_DIR
    saved = (ROOT, MANIFEST, MODELS_DIR)
    with tempfile.TemporaryDirectory() as tmp:
        ROOT = Path(tmp)
        MANIFEST = ROOT / "formal" / "models.toml"
        MODELS_DIR = ROOT / "formal" / "VimanamProofs"
        try:
            MODELS_DIR.mkdir(parents=True)
            (MODELS_DIR / "M.lean").write_text("-- model\n")
            (ROOT / "src").mkdir()
            rs = ROOT / "src" / "a.rs"
            rs.write_text("fn one() { 1 }\nfn two() { 2 }\n")
            zero = "v1:sha256:" + "0" * 64
            MANIFEST.write_text(
                f'[[model]]\nlean = "formal/VimanamProofs/M.lean"\nread_digest = "{zero}"\n'
                'rust = ["src/a.rs#one", "src/a.rs#two"]\n'
            )
            ident = ["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false"]

            def commit(msg: str) -> None:
                assert git("add", "-A") is not None
                assert git(*ident, "commit", "-qm", msg) is not None

            assert git("init", "-q") is not None
            commit("init")
            bump(["M"])
            entry = load_manifest()[0]
            assert entry.get("read_against"), "bump sets read_against"
            assert digest(entry["rust"]) == entry["read_digest"]
            commit("bump")
            rs.write_text("fn one() { 1 }\nfn two() { 3 }\n")
            changed, why = changed_refs(load_manifest()[0])
            assert changed == ["src/a.rs#two"], (changed, why)
            # A bump on a branch, then a squash merge: the anchor commit is
            # gone, but history still has a commit with the digest's state.
            commit("change two")
            bump(["M"])
            commit("bump again")
            tree = git("rev-parse", "HEAD^{tree}").strip()
            squashed = git(*ident, "commit-tree", tree, "-p", "HEAD~3", "-m", "squash").strip()
            assert git("reset", "-q", "--hard", squashed) is not None
            assert git("reflog", "expire", "--expire=now", "--all") is not None
            assert git("gc", "-q", "--prune=now") is not None
            rs.write_text("fn one() { 9 }\nfn two() { 3 }\n")
            changed, why = changed_refs(load_manifest()[0])
            assert changed == ["src/a.rs#one"], (changed, why)
        finally:
            ROOT, MANIFEST, MODELS_DIR = saved
            _GIT_TEXT.clear()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--strict", action="store_true", help="exit 1 when a model drifted")
    parser.add_argument("--bump", nargs="+", metavar="MODEL", help="re-fingerprint these models")
    parser.add_argument("--bump-all", action="store_true", help="re-fingerprint every model")
    parser.add_argument("--self-test", action="store_true", help="run the script's own tests")
    args = parser.parse_args()
    try:
        if args.self_test:
            self_test()
            return 0
        if args.bump_all:
            bump([e["lean"] for e in load_manifest()])
            return 0
        if args.bump:
            bump(args.bump)
            return 0
        return check(args.strict)
    except ManifestError as err:
        print(f"error: {err}", file=sys.stderr)
        return 3


if __name__ == "__main__":
    sys.exit(main())
