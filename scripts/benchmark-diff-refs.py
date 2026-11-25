#!/usr/bin/env python3
"""Timing check for `vimanam diff` on large `$ref` graphs (#152, #181).

Runs pinned public OpenAPI pairs through `vimanam diff` in each output mode,
serially, and reports the median wall time, the min-max range, peak RSS and
the output size and SHA-256. The cases include the 15 consecutive Stripe
release pairs of `spec3.json` from a402765 to bcfd59f, which only finish since
`diff` compares `$ref` targets as edges (#181). With `--baseline`, runs a
second binary on the same inputs and checks that the outputs are
byte-identical wherever the baseline finishes inside its timeout; pass
`--allow-different` when the two binaries are expected to disagree (a
baseline from before #181 on specs with `$ref`s), to report without failing.

Standard library only. The corpus layout matches `benchmark-public.py`:

    CORPUS/openai-repo      git clone of openai/openai-openapi
    CORPUS/stripe-openapi   git clone of stripe/openapi (blobless is fine)

Example (measure on a quiet machine; runs are serial by design):

    python3 scripts/benchmark-diff-refs.py \\
        --binary target/release/vimanam \\
        --baseline /tmp/vimanam-main \\
        --corpus /tmp/vimanam-eval-corpus --runs 5

`--only stripe-releases` runs just the release pairs; `--baseline-timeout`
bounds the baseline separately (a pre-#181 baseline times out on every
release pair).
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import statistics
import subprocess
import sys
import tempfile
import time
from pathlib import Path

# The last 16 commits touching Stripe's `openapi/spec3.json`, oldest first.
STRIPE_RELEASES = [
    "a402765", "0aaf9a6", "da3eadb", "cfe95bf", "a54c388", "d771243", "6a1ea0a", "fbd0091",
    "f4ac6d9", "7061322", "d5d11f6", "af5309c", "30d3391", "c8faccb", "2bab976", "bcfd59f",
]

# (name, repo dir, path in repo, old rev, new rev, modes, note)
CASES = [
    ("OpenAI a01e367..373f26a", "openai-repo", "openapi.json", "a01e367", "373f26a", ["md", "v1", "v2"], ""),
    ("Stripe 5237fa0 self-diff", "stripe-openapi", "openapi/spec3.json", "5237fa0", "5237fa0", ["md", "v1", "v2"], ""),
    ("Stripe 2f8d147..5237fa0", "stripe-openapi", "openapi/spec3.json", "2f8d147", "5237fa0", ["md", "v1", "v2"], ""),
    # Consecutive Stripe releases (#181). Before refs were compared as edges
    # every one timed out: a change under Stripe's 97-node reference cycle
    # was reported once per reference path, and there are millions.
    *[
        (f"Stripe {old}..{new}", "stripe-openapi", "openapi/spec3.json", old, new, ["md", "v1", "v2"], "")
        for old, new in zip(STRIPE_RELEASES, STRIPE_RELEASES[1:])
    ],
]

MODES = {
    "md": [],
    "v1": ["--format", "json", "--contract", "1"],
    "v2": ["--format", "json"],
}


def max_rss_bytes(rusage) -> int:
    # macOS reports bytes, Linux KiB.
    return rusage.ru_maxrss if sys.platform == "darwin" else rusage.ru_maxrss * 1024


def run_once(binary: str, args: list[str], out_path: Path, timeout: float) -> dict:
    """One serial run: wall time, peak RSS, exit code; stdout goes to `out_path`."""
    with open(out_path, "wb") as out:
        start = time.perf_counter()
        proc = subprocess.Popen([binary, "diff", *args], stdout=out, stderr=subprocess.DEVNULL)
        deadline = start + timeout
        while True:
            pid, status, rusage = os.wait4(proc.pid, os.WNOHANG)
            if pid:
                break
            if time.perf_counter() > deadline:
                proc.kill()
                _, status, rusage = os.wait4(proc.pid, 0)
                return {"timeout": True, "seconds": timeout, "max_rss_mib": round(max_rss_bytes(rusage) / 2**20, 1)}
            time.sleep(0.005)
        elapsed = time.perf_counter() - start
    return {
        "timeout": False,
        "exit": os.waitstatus_to_exitcode(status),
        "seconds": elapsed,
        "max_rss_mib": round(max_rss_bytes(rusage) / 2**20, 1),
    }


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def measure(binary: str, args: list[str], out_path: Path, runs: int, timeout: float) -> dict:
    samples = []
    for _ in range(runs):
        sample = run_once(binary, args, out_path, timeout)
        samples.append(sample)
        if sample["timeout"]:
            break  # one timeout is enough; do not burn N x timeout
    if samples[-1]["timeout"]:
        return {"timeout_s": timeout, "max_rss_mib": samples[-1]["max_rss_mib"]}
    seconds = [s["seconds"] for s in samples]
    return {
        "exit": samples[-1]["exit"],
        "median_ms": round(statistics.median(seconds) * 1000, 1),
        "min_ms": round(min(seconds) * 1000, 1),
        "max_ms": round(max(seconds) * 1000, 1),
        "max_rss_mib": max(s["max_rss_mib"] for s in samples),
        "output_bytes": out_path.stat().st_size,
        "output_sha256": sha256(out_path),
        "samples_ms": [round(s * 1000, 1) for s in seconds],
    }


def fmt(result: dict | None) -> str:
    if result is None:
        return "-"
    if "timeout_s" in result:
        return f"timeout >{result['timeout_s']:.0f} s"
    return f"{result['median_ms']:.0f} ms, {result['max_rss_mib']:.0f} MiB"


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--binary", required=True, help="vimanam binary to measure")
    ap.add_argument("--baseline", help="optional second binary; outputs must match it byte for byte")
    ap.add_argument("--corpus", required=True, help="directory with openai-repo/ and stripe-openapi/")
    ap.add_argument("--runs", type=int, default=5)
    ap.add_argument("--timeout", type=float, default=60.0, help="seconds per run")
    ap.add_argument("--baseline-timeout", type=float, help="seconds per baseline run (default: --timeout)")
    ap.add_argument("--allow-different", action="store_true", help="report baseline differences without failing")
    ap.add_argument(
        "--only",
        choices=["all", "stripe-releases"],
        default="all",
        help="run every case (default) or only the consecutive Stripe release pairs",
    )
    ap.add_argument("--output", help="write results as JSON here")
    args = ap.parse_args()

    binary = str(Path(args.binary).resolve())
    baseline = str(Path(args.baseline).resolve()) if args.baseline else None
    corpus = Path(args.corpus).resolve()
    results = {
        "environment": {
            "platform": platform.platform(),
            "machine": platform.machine(),
            "binary_version": subprocess.check_output([binary, "--version"], text=True).strip(),
            "baseline_version": subprocess.check_output([baseline, "--version"], text=True).strip() if baseline else None,
            "runs": args.runs,
            "timeout_s": args.timeout,
        },
        "cases": [],
    }
    mismatches = 0
    with tempfile.TemporaryDirectory() as tmp:
        tmpdir = Path(tmp)
        cases = CASES
        if args.only == "stripe-releases":
            cases = [case for case in CASES if case[3] in STRIPE_RELEASES and case[3] != case[4]]
        for name, repo, path, old_rev, new_rev, modes, note in cases:
            repo_dir = corpus / repo
            specs = []
            for rev in (old_rev, new_rev):
                spec = tmpdir / f"{repo}-{rev}.json"
                if not spec.exists():
                    spec.write_bytes(subprocess.check_output(["git", "-C", str(repo_dir), "show", f"{rev}:{path}"]))
                specs.append(str(spec))
            for mode in modes:
                flags = [*MODES[mode], *specs]
                new = measure(binary, flags, tmpdir / "binary.out", args.runs, args.timeout)
                row = {"case": name, "mode": mode, "binary": new, "note": note}
                if baseline:
                    base = measure(
                        baseline, flags, tmpdir / "baseline.out", args.runs, args.baseline_timeout or args.timeout
                    )
                    row["baseline"] = base
                    if "output_sha256" in base and "output_sha256" in new:
                        row["identical"] = base["output_sha256"] == new["output_sha256"]
                        mismatches += not row["identical"]
                results["cases"].append(row)
                same = {True: "identical", False: "DIFFERENT"}.get(row.get("identical"), "")
                print(f"{name:28} {mode:3} {fmt(new):>22}  baseline {fmt(row.get('baseline')):>22}  {same} {note}", flush=True)
    if args.output:
        Path(args.output).write_text(json.dumps(results, indent=2) + "\n")
    return 1 if mismatches and not args.allow_different else 0


if __name__ == "__main__":
    sys.exit(main())
