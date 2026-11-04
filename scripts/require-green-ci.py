#!/usr/bin/env python3
"""Require successful full main CI for the checked-out release commit."""

import argparse
import json
import os
import subprocess
import time
from urllib.parse import urlencode

REQUIRED_JOBS = {
    "Format, lint, test (stable)",
    "Build (MSRV 1.96)",
    "Dependency security audit",
}


def gh_pages(endpoint):
    result = subprocess.run(
        ["gh", "api", "--paginate", "--slurp", endpoint],
        check=True,
        capture_output=True,
        text=True,
        timeout=30,
    )
    return json.loads(result.stdout)


def require_green_ci(repository, sha, timeout=600, poll_interval=15):
    query = urlencode(
        {
            "head_sha": sha,
            "branch": "main",
            "event": "push",
            "per_page": 100,
        }
    )
    endpoint = f"repos/{repository}/actions/workflows/ci.yml/runs?{query}"
    deadline = time.monotonic() + timeout
    while True:
        runs = [
            run
            for page in gh_pages(endpoint)
            for run in page["workflow_runs"]
            if run["head_sha"] == sha
            and run["head_branch"] == "main"
            and run["event"] == "push"
        ]
        # Never accept an old success over a newer failed or unfinished run.
        run = max(runs, key=lambda item: item["id"], default=None)
        if run and run["status"] == "completed":
            if run["conclusion"] != "success":
                raise RuntimeError(
                    f"CI for {sha} concluded {run['conclusion']}: {run['html_url']}"
                )
            jobs_endpoint = (
                f"repos/{repository}/actions/runs/{run['id']}/attempts/"
                f"{run['run_attempt']}/jobs?per_page=100"
            )
            jobs = [job for page in gh_pages(jobs_endpoint) for job in page["jobs"]]
            missing = REQUIRED_JOBS - {job["name"] for job in jobs}
            failed = {
                job["name"]
                for job in jobs
                if job["name"] in REQUIRED_JOBS and job["conclusion"] != "success"
            }
            if missing or failed:
                raise RuntimeError(
                    f"Full CI required; missing jobs: {sorted(missing)}; "
                    f"unsuccessful jobs: {sorted(failed)}"
                )
            print(f"Full CI passed for {sha}: {run['html_url']}", flush=True)
            return
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise RuntimeError(f"Timed out waiting for full main CI for {sha}")
        print(f"Waiting for full main CI for {sha}...", flush=True)
        time.sleep(min(poll_interval, remaining))


def validate_tag(ref, version):
    if ref != f"refs/tags/v{version}":
        raise RuntimeError(f"Release requires tag v{version}, got {ref!r}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--timeout", type=int, default=600)
    args = parser.parse_args()
    try:
        if os.environ.get("GITHUB_EVENT_NAME") != "push":
            raise RuntimeError("Releases require a tag push")
        # Ubuntu 22.04's Python predates tomllib; use Cargo's manifest parser.
        metadata = json.loads(
            subprocess.check_output(
                ["cargo", "metadata", "--locked", "--no-deps", "--format-version=1"],
                text=True,
                timeout=30,
            )
        )
        version = next(
            package["version"]
            for package in metadata["packages"]
            if package["name"] == "vimanam"
        )
        validate_tag(os.environ.get("GITHUB_REF", ""), version)
        sha = subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip()
        require_green_ci(os.environ["GITHUB_REPOSITORY"], sha, timeout=args.timeout)
    except (
        RuntimeError,
        KeyError,
        ValueError,
        StopIteration,
        OSError,
        subprocess.SubprocessError,
    ) as error:
        parser.exit(1, f"Release blocked: {error}\n")


if __name__ == "__main__":
    main()
