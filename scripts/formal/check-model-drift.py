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


def ref_code(ref: str) -> str:
    path, _, name = ref.partition("#")
    file = ROOT / path
    if not file.is_file():
        raise ManifestError(f"{ref}: no such file")
    code, lit = scan(file.read_text(encoding="utf-8"))
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


def digest(refs: list[str]) -> str:
    h = hashlib.sha256()
    for ref in sorted(refs):
        h.update(ref.encode())
        h.update(b"\n")
        h.update(ref_code(ref).encode())
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
        block = re.compile(
            rf'(lean = "{re.escape(entry["lean"])}"\n(?:[^\[]*?\n)?read_digest = ")[^"]*(")'
        )
        text, count = block.subn(rf"\g<1>{new}\g<2>", text, count=1)
        if count != 1:
            raise ManifestError(f"{entry['lean']}: cannot find its read_digest line")
        print(f"bumped {entry['lean']} to {new}")
    MANIFEST.write_text(text, encoding="utf-8")


def check(strict: bool) -> int:
    models = load_manifest()
    drifted = []
    for entry in models:
        if digest(entry["rust"]) != entry["read_digest"]:
            drifted.append(entry)
    gha = os.environ.get("GITHUB_ACTIONS") == "true"
    for entry in drifted:
        stem = Path(entry["lean"]).stem
        msg = (
            f"{entry['lean']} has drifted: the Rust it models changed since it was last read "
            f"({', '.join(entry['rust'])}). Re-read the model against the Rust, update it if "
            f"the change matters, then run: python3 -I scripts/formal/check-model-drift.py --bump {stem}"
        )
        if gha:
            for ref in entry["rust"]:
                print(f"::warning file={ref.partition('#')[0]}::{msg}")
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
    print("self-test passed")


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
