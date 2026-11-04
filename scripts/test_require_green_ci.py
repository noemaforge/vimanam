"""Regression tests for the publication barrier; no GitHub writes or sleeps."""

import contextlib
import importlib.util
import io
import json
import subprocess
import unittest
from pathlib import Path
from unittest.mock import patch

spec = importlib.util.spec_from_file_location(
    "gate", Path(__file__).with_name("require-green-ci.py")
)
gate = importlib.util.module_from_spec(spec)
spec.loader.exec_module(gate)
SHA = "a" * 40


def run(**changes):
    result = {
        "id": 10,
        "head_sha": SHA,
        "head_branch": "main",
        "event": "push",
        "status": "completed",
        "conclusion": "success",
        "run_attempt": 2,
        "html_url": "https://github.com/example/repo/actions/runs/10",
    }
    return result | changes


def jobs():
    return [
        {"name": name, "conclusion": "success"} for name in sorted(gate.REQUIRED_JOBS)
    ]


class ReleaseGateTests(unittest.TestCase):
    def setUp(self):
        self.output = contextlib.redirect_stdout(io.StringIO())
        self.output.__enter__()
        self.addCleanup(self.output.__exit__, None, None, None)

    def check(self, responses, **kwargs):
        with patch.object(gate, "gh_pages", side_effect=responses) as api:
            gate.require_green_ci("example/repo", SHA, **kwargs)
        return api

    def test_exact_commit_and_latest_attempt_succeed(self):
        api = self.check([[{"workflow_runs": [run()]}], [{"jobs": jobs()}]])
        self.assertIn(f"head_sha={SHA}", api.call_args_list[0].args[0])
        self.assertIn("event=push", api.call_args_list[0].args[0])
        self.assertIn("branch=main", api.call_args_list[0].args[0])
        self.assertIn("/runs/10/attempts/2/jobs", api.call_args_list[1].args[0])

    def test_failed_cancelled_or_skipped_run_is_rejected(self):
        for conclusion in ["failure", "cancelled", "skipped", "neutral", "timed_out"]:
            with (
                self.subTest(conclusion=conclusion),
                self.assertRaisesRegex(RuntimeError, "concluded"),
            ):
                self.check([[{"workflow_runs": [run(conclusion=conclusion)]}]])

    def test_old_success_cannot_mask_newer_failure(self):
        with self.assertRaisesRegex(RuntimeError, "concluded failure"):
            self.check([[{"workflow_runs": [run(), run(id=11, conclusion="failure")]}]])

    def test_unrelated_green_runs_cannot_authorize_release(self):
        for changes in [
            {"head_sha": "b" * 40},
            {"head_branch": "other"},
            {"event": "pull_request"},
            {"event": "schedule"},
        ]:
            with (
                self.subTest(changes=changes),
                self.assertRaisesRegex(RuntimeError, "Timed out"),
            ):
                self.check([[{"workflow_runs": [run(**changes)]}]], timeout=0)

    def test_missing_run_times_out(self):
        with self.assertRaisesRegex(RuntimeError, "Timed out"):
            self.check([[{"workflow_runs": []}]], timeout=0)

    def test_waits_for_pending_ci_without_rerunning_it(self):
        for status in ["queued", "in_progress"]:
            with self.subTest(status=status), patch.object(gate.time, "sleep") as sleep:
                api = self.check(
                    [
                        [{"workflow_runs": [run(status=status, conclusion=None)]}],
                        [{"workflow_runs": [run()]}],
                        [{"jobs": jobs()}],
                    ]
                )
                sleep.assert_called_once()
                self.assertEqual(api.call_count, 3)

    def test_newer_pending_run_cannot_use_old_success(self):
        with self.assertRaisesRegex(RuntimeError, "Timed out"):
            self.check(
                [[{"workflow_runs": [run(), run(id=11, status="queued")]}]], timeout=0
            )

    def test_missing_or_skipped_required_jobs_are_rejected(self):
        for name in gate.REQUIRED_JOBS:
            for conclusion in ["missing", "skipped", "failure"]:
                changed = [job for job in jobs() if job["name"] != name]
                if conclusion != "missing":
                    changed.append({"name": name, "conclusion": conclusion})
                with (
                    self.subTest(name=name, conclusion=conclusion),
                    self.assertRaisesRegex(RuntimeError, "Full CI required"),
                ):
                    self.check([[{"workflow_runs": [run()]}], [{"jobs": changed}]])

    def test_all_api_pages_are_considered(self):
        api = self.check(
            [
                [{"workflow_runs": []}, {"workflow_runs": [run()]}],
                [{"jobs": jobs()[:1]}, {"jobs": jobs()[1:]}],
            ]
        )
        self.assertEqual(api.call_count, 2)

    def test_api_errors_fail_closed(self):
        with (
            patch.object(
                gate, "gh_pages", side_effect=subprocess.CalledProcessError(1, "gh")
            ),
            self.assertRaises(subprocess.CalledProcessError),
        ):
            gate.require_green_ci("example/repo", SHA)

    def test_gh_uses_read_only_paginated_api(self):
        with patch.object(gate.subprocess, "run") as command:
            command.return_value.stdout = '[{"workflow_runs": []}]'
            self.assertEqual(
                gate.gh_pages("repos/example/repo/actions/runs"),
                [{"workflow_runs": []}],
            )
            args = command.call_args.args[0]
            self.assertEqual(
                args[:5],
                [
                    "gh",
                    "api",
                    "--paginate",
                    "--slurp",
                    "repos/example/repo/actions/runs",
                ],
            )

    def test_tag_must_match_package_version(self):
        gate.validate_tag("refs/tags/v2.1.0", "2.1.0")
        gate.validate_tag("refs/tags/v2.2.0-rc.1", "2.2.0-rc.1")
        for ref in ["refs/heads/main", "refs/tags/v2.0.0", "refs/tags/2.1.0"]:
            with (
                self.subTest(ref=ref),
                self.assertRaisesRegex(RuntimeError, "tag v2.1.0"),
            ):
                gate.validate_tag(ref, "2.1.0")

    def test_entrypoint_checks_tag_and_resolves_checked_out_commit(self):
        env = {
            "GITHUB_EVENT_NAME": "push",
            "GITHUB_REF": "refs/tags/v2.1.0",
            "GITHUB_REPOSITORY": "example/repo",
        }
        metadata = json.dumps({"packages": [{"name": "vimanam", "version": "2.1.0"}]})
        with (
            patch.dict(gate.os.environ, env),
            patch("sys.argv", ["require-green-ci.py"]),
            patch.object(
                gate.subprocess, "check_output", side_effect=[metadata, SHA + "\n"]
            ) as command,
            patch.object(gate, "require_green_ci") as require,
        ):
            gate.main()
        self.assertIn("--locked", command.call_args_list[0].args[0])
        self.assertEqual(
            command.call_args_list[1].args[0], ["git", "rev-parse", "HEAD"]
        )
        require.assert_called_once_with("example/repo", SHA, timeout=600)

    def test_entrypoint_rejects_non_tag_events_and_version_mismatches(self):
        for event, ref in [
            ("workflow_dispatch", "refs/tags/v2.1.0"),
            ("push", "refs/tags/v2.0.0"),
            ("push", "refs/heads/main"),
        ]:
            with (
                self.subTest(event=event, ref=ref),
                patch.dict(
                    gate.os.environ,
                    {
                        "GITHUB_EVENT_NAME": event,
                        "GITHUB_REF": ref,
                    },
                ),
                patch("sys.argv", ["require-green-ci.py"]),
            ):
                metadata = json.dumps(
                    {"packages": [{"name": "vimanam", "version": "2.1.0"}]}
                )
                with (
                    patch.object(
                        gate.subprocess, "check_output", return_value=metadata
                    ),
                    patch.object(gate, "require_green_ci") as require,
                ):
                    with self.assertRaises(SystemExit) as exit:
                        gate.main()
                    self.assertEqual(exit.exception.code, 1)
                    require.assert_not_called()


if __name__ == "__main__":
    unittest.main()
