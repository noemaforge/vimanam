#!/usr/bin/env python3
"""Compare release binaries on generated, public-data-only Git histories."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import statistics
import subprocess
import sys
import tempfile
import time


SPEC = b'{"openapi":"3.0.3","info":{"title":"Scratch API","version":"1"},"paths":{}}'


def git(repo, *args, data=None):
    env = dict(os.environ)
    for key in ("GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE"):
        env.pop(key, None)
    return subprocess.run(
        [REAL_GIT, "-C", str(repo), *args], input=data,
        stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=True, env=env,
    ).stdout


def make_repo(root, kind, commits, renames):
    repo = root / kind
    repo.mkdir()
    git(repo, "init", "-q")
    stream = bytearray()
    mark = 0

    def commit(branch, changes, parent=None, merge=None):
        nonlocal mark
        mark += 1
        stream.extend(f"commit refs/heads/{branch}\nmark :{mark}\n".encode())
        stream.extend(f"committer Scratch <scratch@example.invalid> {1700000000 + mark} +0000\n".encode())
        stream.extend(b"data 7\nscratch\n")
        if parent is not None:
            stream.extend(f"from :{parent}\n".encode())
        if merge is not None:
            stream.extend(f"merge :{merge}\n".encode())
        for change in changes:
            if change[0] == "M":
                _, path, contents = change
                stream.extend(f"M 100644 inline {path}\ndata {len(contents)}\n".encode())
                stream.extend(contents + b"\n")
            else:
                stream.extend((" ".join(change) + "\n").encode())
        stream.extend(b"\n")
        return mark

    old = commit("main", [("M", "api.json", SPEC)])
    path = "api.json"
    rename_every = max(1, commits // renames) if renames else commits + 1
    renamed = 0
    for i in range(commits):
        changes = [("M", "notes.txt", f"unrelated {i}\n".encode())]
        if kind == "rename_chain" and (i + 1) % rename_every == 0 and renamed < renames:
            renamed += 1
            new_path = f"api-{renamed}.json"
            changes.append(("R", path, new_path))
            path = new_path
        commit("main", changes)
    if kind == "rename_chain":
        rewritten = json.dumps({
            "openapi": "3.0.3", "info": {"title": "Scratch API", "version": "2"},
            "paths": {}, "description": "major later rewrite " * 500,
        }).encode()
        commit("main", [("M", path, rewritten)])
    if kind == "merge_fallback":
        main = mark
        side = commit("side", [("M", "side.txt", b"side\n")], parent=old)
        commit("main", [("M", "side.txt", b"side\n")], parent=main, merge=side)
    stream.extend(f"reset refs/tags/old\nfrom :{old}\n\nreset refs/tags/new\nfrom :{mark}\n\ndone\n".encode())
    git(repo, "fast-import", "--quiet", data=bytes(stream))
    return repo, path


def measure(binary, repo, path, wrapper_dir, root, runs):
    elapsed, counts = [], []
    expected = None
    count_file = root / "git-count"
    env = dict(os.environ)
    env.update({
        "RUST_LOG": "off",
        "PATH": str(wrapper_dir) + os.pathsep + os.environ.get("PATH", ""),
        "VIMANAM_BENCH_REAL_GIT": REAL_GIT,
        "VIMANAM_BENCH_GIT_COUNT": str(count_file),
    })
    args = [str(binary), "diff", "--from-ref", "old", "--to-ref", "new", "--spec", path, "--format", "json"]
    for iteration in range(runs + 1):
        count_file.write_bytes(b"")
        started = time.perf_counter()
        output = subprocess.run(args, cwd=repo, env=env, capture_output=True, check=True)
        seconds = time.perf_counter() - started
        if output.stderr:
            raise RuntimeError(output.stderr.decode(errors="replace"))
        result = json.loads(output.stdout)
        result["generator"].pop("version")  # Only the package version may differ.
        if expected is not None and result != expected:
            raise RuntimeError("Output changed between repetitions")
        expected = result
        if iteration:  # One warmup per binary/scenario, outside measurements.
            elapsed.append(seconds)
            counts.append(count_file.stat().st_size)
    if len(set(counts)) != 1:
        raise RuntimeError(f"Git subprocess count varied: {counts}")
    return {
        "median_seconds": statistics.median(elapsed),
        "min_seconds": min(elapsed), "max_seconds": max(elapsed),
        "git_subprocesses": counts[0], "seconds": elapsed,
    }, expected


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("baseline", type=Path)
    parser.add_argument("optimized", type=Path)
    parser.add_argument("--commits", type=int, default=400)
    parser.add_argument("--renames", type=int, default=20)
    parser.add_argument("--runs", type=int, default=5)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    if args.commits < 1 or args.renames < 1 or args.runs < 1:
        parser.error("commits, renames and runs must be positive")
    if args.renames > args.commits:
        parser.error("renames cannot exceed commits")
    binaries = [args.baseline.resolve(strict=True), args.optimized.resolve(strict=True)]
    global REAL_GIT
    REAL_GIT = shutil.which("git")
    if REAL_GIT is None:
        parser.error("Git is required")
    result = {
        "platform": platform.platform(), "git": subprocess.check_output([REAL_GIT, "--version"], text=True).strip(),
        "commits": args.commits, "renames": args.renames, "runs": args.runs,
        "binaries": {
            label: {
                "version": subprocess.check_output([str(binary), "--version"], text=True).strip(),
                "sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
            }
            for label, binary in zip(("baseline", "optimized"), binaries)
        },
        "measurement": "CLI wall time including captured Git wrapper overhead; one warmup; exact JSON equality except generator.version",
        "scenarios": {},
    }
    with tempfile.TemporaryDirectory(prefix="vimanam-gitrefs-bench-") as temp:
        root = Path(temp)
        wrappers = root / "bin"
        wrappers.mkdir()
        wrapper = wrappers / "git"
        wrapper.write_text('#!/bin/sh\nprintf x >> "$VIMANAM_BENCH_GIT_COUNT"\nexec "$VIMANAM_BENCH_REAL_GIT" "$@"\n')
        wrapper.chmod(0o755)
        for kind in ("linear_unrelated", "rename_chain", "merge_fallback"):
            repo, path = make_repo(root, kind, args.commits, args.renames)
            baseline, old_output = measure(binaries[0], repo, path, wrappers, root, args.runs)
            optimized, new_output = measure(binaries[1], repo, path, wrappers, root, args.runs)
            if old_output != new_output:
                raise RuntimeError(f"Baseline/optimized JSON differs for {kind}")
            result["scenarios"][kind] = {
                "baseline": baseline, "optimized": optimized,
                "median_speedup": baseline["median_seconds"] / optimized["median_seconds"],
            }
            print(
                f"{kind}: median {baseline['median_seconds']:.3f}s -> "
                f"{optimized['median_seconds']:.3f}s; "
                f"Git {baseline['git_subprocesses']} -> {optimized['git_subprocesses']}",
                file=sys.stderr, flush=True,
            )
    report = json.dumps(result, indent=2) + "\n"
    if args.output:
        args.output.write_text(report)
    print(report, end="")


if __name__ == "__main__":
    main()
