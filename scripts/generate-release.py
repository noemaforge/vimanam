#!/usr/bin/env python3
"""Generate cargo-dist CI with a green-commit guard before any dist commands."""

import argparse
import difflib
import shutil
import subprocess
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
WORKFLOW = Path(".github/workflows/release.yml")
# dist's host job permits skipped builds, but requires the plan job to succeed.
GUARD = """      - name: Require successful CI for the exact release commit
        env:
          GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}
        run: python3 scripts/require-green-ci.py
"""


def generate():
    # Generate from current files in an isolated snapshot, so --check never edits
    # the worktree. Exclude ignored local specs, build outputs and credentials.
    files = (
        subprocess.check_output(
            ["git", "ls-files", "--cached", "--others", "--exclude-standard", "-z"],
            cwd=ROOT,
        )
        .decode()
        .split("\0")
    )
    with tempfile.TemporaryDirectory(prefix="vimanam-release-") as scratch:
        snapshot = Path(scratch)
        for name in set(files) - {""}:
            source = ROOT / name
            if source.is_file():
                destination = snapshot / name
                destination.parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(source, destination)
        # dist refuses to regenerate a workflow marked allow-dirty. Remove that
        # exemption only in the disposable snapshot, never in the real config.
        config = snapshot / "dist-workspace.toml"
        config.write_text(
            config.read_text().replace('allow-dirty = ["ci"]', "allow-dirty = []")
        )
        subprocess.run(["dist", "generate", "--mode", "ci"], cwd=snapshot, check=True)
        workflow = (snapshot / WORKFLOW).read_text()
    anchor = "      - name: Install dist\n"
    if workflow.count(anchor) != 2:
        raise RuntimeError(
            "cargo-dist template changed; review the pre-plan guard insertion"
        )
    workflow = workflow.replace(anchor, GUARD + anchor, 1)
    # crates.io accepted prerelease tags before coordination with dist. Keep
    # that behavior without publishing prereleases to the stable Homebrew tap.
    anchor = (
        "  custom-publish-crate:\n    needs:\n      - plan\n      - host\n"
        "    if: ${{ !fromJson(needs.plan.outputs.val).announcement_is_prerelease || "
        "fromJson(needs.plan.outputs.val).publish_prereleases }}\n"
    )
    if workflow.count(anchor) != 1:
        raise RuntimeError(
            "cargo-dist publish hook changed; review prerelease publishing"
        )
    workflow = workflow.replace(
        anchor, "  custom-publish-crate:\n    needs:\n      - plan\n      - host\n", 1
    )
    anchor = '  plan:\n    runs-on: "ubuntu-22.04"\n'
    if workflow.count(anchor) != 1:
        raise RuntimeError("cargo-dist plan job changed; review its token permissions")
    workflow = workflow.replace(
        anchor,
        "  plan:\n    permissions:\n      actions: read\n      contents: write\n"
        '    timeout-minutes: 15\n    runs-on: "ubuntu-22.04"\n',
        1,
    )
    return "# Regenerate with: python3 scripts/generate-release.py\n" + workflow


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    workflow = generate()
    path = ROOT / WORKFLOW
    if args.check:
        existing = path.read_text()
        if existing != workflow:
            print(
                "".join(
                    difflib.unified_diff(
                        existing.splitlines(True),
                        workflow.splitlines(True),
                        fromfile=str(WORKFLOW),
                        tofile="expected release workflow",
                    )
                ),
                end="",
            )
            parser.exit(1, "Run python3 scripts/generate-release.py\n")
    else:
        path.write_text(workflow)


if __name__ == "__main__":
    main()
