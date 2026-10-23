#!/usr/bin/env python3
"""Execute the composite Action's real bootstrap shell with a recording Cargo.

No Rust compilation, network request, provider call, or GitHub write occurs.
The fake Cargo checks routing/failure behavior, not compiler or cache correctness.
Owned by release/ci in policy/allow.toml (action-bootstrap-contract-tests).
"""
from __future__ import annotations

import json
import os
import re
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


def runner_shell() -> str:
    text = (ROOT / "action.yml").read_text(encoding="utf-8")
    marker = "    - name: Resolve ub-review runner\n"
    if text.count(marker) != 1:
        raise AssertionError("expected one runner-resolution step")
    step = text.split(marker, 1)[1].split("\n    - ", 1)[0]
    script = step.split("      run: |\n", 1)[1]
    if any(line and not line.startswith("        ") for line in script.splitlines()):
        raise AssertionError("unexpected shell block indentation")
    return "\n".join(line[8:] if line else "" for line in script.splitlines()) + "\n"


class BootstrapContract(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory(prefix="ub-review-bootstrap-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.action = self.root / "action source"
        self.action.mkdir()
        (self.action / "Cargo.toml").write_text('[package]\nname="ub-review"\n', encoding="utf-8")
        (self.action / "Cargo.lock").write_text("# fixture lockfile\n", encoding="utf-8")
        (self.action / "src").mkdir()
        (self.action / "src/main.rs").write_text("fn main() {}\n", encoding="utf-8")
        (self.action / ".cargo").mkdir()
        (self.action / ".cargo/config.toml").write_text("[build]\njobs=1\n", encoding="utf-8")
        (self.action / ".ub-review.toml").write_text("# retained configuration\n", encoding="utf-8")
        self.caller = self.root / "reviewed repository"
        self.caller.mkdir()
        self.runtime = self.root / "runner temp"
        self.runtime.mkdir()
        self.fake_bin = self.root / "bin"
        self.fake_bin.mkdir()
        self.output = self.root / "github-output"
        self.output.touch()
        self.log = self.root / "cargo.json"
        self.env_file = self.root / "github-env"
        self.env_file.touch()
        cargo = self.fake_bin / "cargo"
        cargo.write_text(
            f"#!{sys.executable}\n" + '''import json, os, pathlib, sys
args = sys.argv[1:]
pathlib.Path(os.environ["BOOTSTRAP_TEST_LOG"]).write_text(json.dumps({
    "argv": args, "cargo_target_dir": os.environ.get("CARGO_TARGET_DIR")
}), encoding="utf-8")
if os.environ.get("BOOTSTRAP_TEST_FAIL") == "1":
    sys.exit(23)
if os.environ.get("BOOTSTRAP_TEST_NO_BINARY") != "1":
    target = pathlib.Path(args[args.index("--target-dir") + 1])
    binary = target / "release/ub-review"
    binary.parent.mkdir(parents=True, exist_ok=True)
    binary.write_text("#!/bin/sh\\nexit 0\\n", encoding="utf-8")
    binary.chmod(0o755)
''', encoding="utf-8")
        cargo.chmod(0o755)
        # Release-mode negative tests cannot accidentally reach the network.
        curl = self.fake_bin / "curl"
        curl.write_text("#!/bin/sh\nexit 22\n", encoding="utf-8")
        curl.chmod(0o755)
        self.env = {
            "PATH": f"{self.fake_bin}:/usr/bin:/bin",
            "HOME": str(self.root),
            "RUNNER_TEMP": str(self.runtime),
            "GITHUB_ACTION_PATH": str(self.action),
            "GITHUB_OUTPUT": str(self.output),
            "GITHUB_ENV": str(self.env_file),
            "UB_REVIEW_INSTALL_MODE": "source",
            "BOOTSTRAP_TEST_LOG": str(self.log),
        }

    def run_shell(self, **updates: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            ["bash", "--noprofile", "--norc", "-c", runner_shell()],
            cwd=self.caller, env={**self.env, **updates},
            capture_output=True, text=True, timeout=15, check=False,
        )

    def success(self, **updates: str) -> dict:
        result = self.run_shell(**updates)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        return json.loads(self.log.read_text(encoding="utf-8"))

    def target(self, log: dict) -> Path:
        return Path(log["argv"][log["argv"].index("--target-dir") + 1])

    def test_default_source_build_is_locked_and_uses_complete_source(self) -> None:
        log = self.success()
        self.assertEqual(log["argv"], [
            "build", "--manifest-path", str(self.runtime / "ub-review-action-src/Cargo.toml"),
            "--locked", "--release", "--target-dir", str(self.runtime / "ub-review-action-src/target"),
        ])
        for name in ["Cargo.lock", "src/main.rs", ".cargo/config.toml", ".ub-review.toml"]:
            self.assertEqual((self.action / name).read_bytes(),
                             (self.runtime / "ub-review-action-src" / name).read_bytes())

    def test_explicit_absolute_bootstrap_target_wins_without_exporting_cargo_override(self) -> None:
        target = self.root / "bootstrap cache"
        caller_target = self.caller / "target"
        log = self.success(UB_REVIEW_SOURCE_TARGET_DIR=str(target), CARGO_TARGET_DIR=str(caller_target))
        self.assertEqual(self.target(log), target)
        self.assertEqual(log["cargo_target_dir"], str(caller_target))
        self.assertEqual(self.env_file.read_text(), "")
        self.assertIn(f"bin={target}/release/ub-review\n", self.output.read_text())

    def test_relative_bootstrap_target_is_relative_to_caller(self) -> None:
        log = self.success(UB_REVIEW_SOURCE_TARGET_DIR="build cache")
        self.assertEqual(self.target(log), self.caller / "build cache")

    def test_legacy_absolute_cargo_target_remains_supported(self) -> None:
        target = self.root / "legacy cache"
        self.assertEqual(self.target(self.success(CARGO_TARGET_DIR=str(target))), target)

    def test_legacy_relative_cargo_target_remains_supported(self) -> None:
        self.assertEqual(self.target(self.success(CARGO_TARGET_DIR="legacy cache")),
                         self.caller / "legacy cache")

    def test_cached_dependency_survives_source_refresh(self) -> None:
        target = self.root / "bootstrap cache"
        self.success(UB_REVIEW_SOURCE_TARGET_DIR=str(target))
        sentinel = target / "dependency-sentinel"
        sentinel.write_bytes(b"compiled dependency fixture")
        source = self.runtime / "ub-review-action-src"
        (source / "stale-source").write_text("stale", encoding="utf-8")
        self.success(UB_REVIEW_SOURCE_TARGET_DIR=str(target))
        self.assertEqual(sentinel.read_bytes(), b"compiled dependency fixture")
        self.assertFalse((source / "stale-source").exists())

    def test_source_copy_does_not_copy_git_or_target(self) -> None:
        for folder in [".git", "target"]:
            (self.action / folder).mkdir()
            (self.action / folder / "excluded-sentinel").write_text("cache metadata", encoding="utf-8")
        self.success()
        source = self.runtime / "ub-review-action-src"
        self.assertFalse((source / ".git").exists())
        self.assertFalse((source / "target/excluded-sentinel").exists())

    def test_missing_lockfile_fails_before_cargo(self) -> None:
        (self.action / "Cargo.lock").unlink()
        result = self.run_shell()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("committed regular Cargo.lock", result.stdout)
        self.assertFalse(self.log.exists())
        self.assertEqual(self.output.read_text(), "")

    def test_symlink_lockfile_fails_before_cargo(self) -> None:
        lock = self.action / "Cargo.lock"
        lock.unlink()
        (self.root / "other.lock").write_text("lock", encoding="utf-8")
        lock.symlink_to(self.root / "other.lock")
        result = self.run_shell()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("committed regular Cargo.lock", result.stdout)
        self.assertFalse(self.log.exists())

    def test_failed_build_cannot_accept_an_old_cached_binary(self) -> None:
        target = self.root / "bootstrap cache"
        self.success(UB_REVIEW_SOURCE_TARGET_DIR=str(target))
        self.output.write_text("")
        result = self.run_shell(UB_REVIEW_SOURCE_TARGET_DIR=str(target), BOOTSTRAP_TEST_FAIL="1")
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(self.output.read_text(), "")

    def test_success_without_binary_is_rejected(self) -> None:
        result = self.run_shell(BOOTSTRAP_TEST_NO_BINARY="1")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("current source build did not produce", result.stdout)
        self.assertEqual(self.output.read_text(), "")

    def test_success_without_binary_cannot_reuse_cached_executable(self) -> None:
        target = self.root / "bootstrap cache"
        self.success(UB_REVIEW_SOURCE_TARGET_DIR=str(target))
        stale = target / "release/ub-review"
        self.assertTrue(stale.exists())
        self.output.write_text("")

        result = self.run_shell(
            UB_REVIEW_SOURCE_TARGET_DIR=str(target),
            BOOTSTRAP_TEST_NO_BINARY="1",
        )

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("current source build did not produce", result.stdout)
        self.assertFalse(stale.exists())
        self.assertEqual(self.output.read_text(), "")

    def test_explicit_source_mode_does_not_select_ambient_binary(self) -> None:
        binary = self.fake_bin / "ub-review"
        binary.write_text("#!/bin/sh\nexit 99\n", encoding="utf-8")
        binary.chmod(0o755)
        self.success()
        self.assertNotIn(f"bin={binary}\n", self.output.read_text())

    def test_unknown_install_mode_does_not_build(self) -> None:
        self.assertNotEqual(self.run_shell(UB_REVIEW_INSTALL_MODE="typo").returncode, 0)
        self.assertFalse(self.log.exists())

    def test_path_mode_does_not_build(self) -> None:
        binary = self.fake_bin / "selected"
        binary.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
        binary.chmod(0o755)
        result = self.run_shell(UB_REVIEW_INSTALL_MODE="path", UB_REVIEW_BINARY_PATH=str(binary))
        self.assertEqual(result.returncode, 0)
        self.assertEqual(self.output.read_text(), f"bin={binary}\n")
        self.assertFalse(self.log.exists())

    def test_release_failure_cannot_fall_back_to_compilation(self) -> None:
        result = self.run_shell(UB_REVIEW_INSTALL_MODE="release", UB_REVIEW_RELEASE_VERSION="v0.1.1",
                                UB_REVIEW_ACTION_REPOSITORY="EffortlessMetrics/ub-review")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("install-mode=release failed", result.stdout)
        self.assertFalse(self.log.exists())

    def test_action_forwards_dedicated_source_target_input(self) -> None:
        action = (ROOT / "action.yml").read_text()
        self.assertIn("  source-target-dir:\n", action)
        self.assertIn("UB_REVIEW_SOURCE_TARGET_DIR: ${{ inputs['source-target-dir'] }}", action)

    def test_dogfood_cache_and_bootstrap_use_the_same_isolated_directory(self) -> None:
        workflow = (ROOT / ".github/workflows/ub-review-gate.yml").read_text()
        target = "${{ runner.temp }}/ub-review-bootstrap-target"
        self.assertIn(f"cache-directories: {target}", workflow)
        self.assertIn(f"source-target-dir: {target}", workflow)
        self.assertIn("install-mode: source", workflow)
        self.assertIn("setup-rust: 'false'", workflow)
        self.assertNotIn("CARGO_TARGET_DIR:", workflow)
        self.assertIn("hashFiles('action.yml', 'scripts/install-gh-runner-tools.sh')", workflow)
        self.assertIn("python scripts/test-action-bootstrap.py", workflow)


    def test_coverage_handoff_uses_only_lcov_and_keeps_oidc_outside_candidate(self) -> None:
        workflow = (ROOT / ".github/workflows/ub-review-gate.yml").read_text()
        gate, telemetry = workflow.split("\n  coverage-upload:\n", 1)
        self.assertIn("name: ub-review-coverage\n          path: target/ub-review/sensors/coverage/lcov.info", gate)
        self.assertIn("name: ub-review-gate\n          path: target/ub-review\n", gate)
        self.assertNotIn("src/main.rs", gate)
        self.assertIn("name: ub-review-coverage\n          path: target/ub-review/sensors/coverage", telemetry)
        self.assertNotIn("name: ub-review-gate", telemetry)
        self.assertIn("files: target/ub-review/sensors/coverage/lcov.info", telemetry)
        self.assertNotIn("id-token: write", gate)
        self.assertIn("id-token: write", telemetry)
        self.assertNotIn("actions/checkout", telemetry)
        self.assertIn("continue-on-error: true", telemetry)


def action_step(name: str) -> tuple[str, dict[str, str]]:
    """Extract executable bytes and explicit expression transports, without YAML deps."""
    text = (ROOT / "action.yml").read_text(encoding="utf-8")
    step = text.split(f"    - name: {name}\n", 1)[1].split("\n    - ", 1)[0]
    front, script = step.split("      run: |\n", 1)
    bindings = dict(re.findall(r"^        ([A-Z_]+): \$\{\{ (.*?) \}\}$", front, re.M))
    lines = []
    for line in script.splitlines():
        if line and not line.startswith("        "):
            break
        lines.append(line[8:] if line else "")
    return "\n".join(lines) + "\n", bindings


class RuntimeDataContract(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory(prefix="ub-review-runtime-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.output = self.root / "github-output"
        self.log = self.root / "argv.json"
        self.sentinel = self.root / "SENTINEL"
        self.binary = self.root / "runner ' $ ` literal"
        self.binary.write_text(f"#!{sys.executable}\n" +
                               'import json, os, pathlib, sys\n'
                               'pathlib.Path(os.environ["ARGV_LOG"]).write_text(json.dumps(sys.argv[1:]))\n')
        self.binary.chmod(0o755)
        self.literal = f"space ' quote \" dollar $HOME backtick `touch {self.sentinel}` $(touch {self.sentinel}) <<EOF"

    def run_step(self, name: str, **updates: str) -> subprocess.CompletedProcess[str]:
        script, bindings = action_step(name)
        # Every binding gets an explicit value; runner is the only executable fixture.
        env = {key: "" for key in bindings}
        env.update(PATH="/usr/bin:/bin", HOME=str(self.root),
                   GITHUB_ACTION_PATH=str(ROOT), GITHUB_OUTPUT=str(self.output),
                   GITHUB_STEP_SUMMARY=str(self.root / "summary"), ARGV_LOG=str(self.log),
                   UB_REVIEW_ACTION_RUNNER_BIN=str(self.binary),
                   UB_REVIEW_ACTION_OUT=str(self.root / "packet"))
        env.update(updates)
        return subprocess.run(["bash", "--noprofile", "--norc", "-c", script],
                              cwd=self.root, env=env, capture_output=True, text=True,
                              timeout=15, check=False)

    def test_no_expressions_in_any_executable_block(self) -> None:
        text = (ROOT / "action.yml").read_text(encoding="utf-8")
        for step in text.split("    - name: ")[1:]:
            if "      run: |\n" in step:
                self.assertNotIn("${{", step.split("      run: |\n", 1)[1], step.splitlines()[0])

    def test_every_runtime_argument_is_one_literal_value(self) -> None:
        _, bindings = action_step("Build UB review packet")
        # Exercise each caller/derived transport, including optional arrays and auth.
        for key in bindings:
            if key in {"UB_REVIEW_ACTION_RUNNER_BIN", "UB_REVIEW_ACTION_ALLOW_HEAVY"}:
                continue
            with self.subTest(binding=key):
                result = self.run_step("Build UB review packet", **{key: self.literal})
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                argv = json.loads(self.log.read_text())
                if key not in {"UB_REVIEW_PROFILE", "UB_REVIEW_PRESET", "UB_REVIEW_ACTION_PRESET",
                               "UB_REVIEW_MINIMAX_API_KEY", "UB_REVIEW_MINIMAX_API_URL",
                               "UB_REVIEW_OPENCODE_API_KEY", "UB_REVIEW_OPENCODE_API_URL",
                               "UB_REVIEW_GITHUB_EVENT_ACTION", "UB_REVIEW_PR_HEAD_SHA"}:
                    self.assertIn(self.literal, argv, key)
                self.assertFalse(self.sentinel.exists())

    def test_packet_paths_are_exact_single_line_records(self) -> None:
        result = self.run_step("Set output paths", UB_REVIEW_ACTION_OUT=self.literal)
        self.assertEqual(result.returncode, 0, result.stderr)
        raw = self.output.read_bytes()
        lines = raw.decode().splitlines()
        self.assertEqual(len(lines), 11)
        values = dict(line.split("=", 1) for line in lines)
        self.assertEqual(len(values), 11)
        self.assertEqual(values["gate-outcome-path"], self.literal + "/review/gate_outcome.json")
        self.assertEqual(values["summary-path"], self.literal + "/running-summary.md")
        self.assertFalse(self.sentinel.exists())

    def test_multiline_paths_reject_before_execution_or_output(self) -> None:
        # Duplicate, arbitrary key, bare record and heredoc/delimiter shapes,
        # together with isolated CR and CRLF, all reject without emitting a byte.
        for suffix in ["\ngate-outcome-path=/tmp/forged", "\nextra=forged", "\nbare",
                       "\nname<<EOF\nforged\nEOF", "\r", "\r\nextra=forged"]:
            for name in ["Build UB review packet", "Post grouped PR review",
                         "Append UB review summary", "Set output paths"]:
                with self.subTest(step=name, suffix=suffix):
                    self.output.unlink(missing_ok=True)
                    self.log.unlink(missing_ok=True)
                    result = self.run_step(name, UB_REVIEW_ACTION_OUT="packet" + suffix)
                    self.assertNotEqual(result.returncode, 0)
                    self.assertIn("out must not contain CR or LF", result.stderr)
                    self.assertFalse(self.log.exists())
                    self.assertFalse(self.output.exists())

    def test_prior_receipt_and_binary_paths_preserve_literal_bytes(self) -> None:
        result = self.run_step("Resolve prior resolved candidates",
                               MANUAL_PRIOR_RESOLVED_CANDIDATES=self.literal)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.output.read_text(), "path=" + self.literal + "\n")
        self.output.unlink()
        result = self.run_step("Resolve ub-review runner", UB_REVIEW_INSTALL_MODE="path",
                               UB_REVIEW_BINARY_PATH=str(self.binary))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.output.read_text(), "bin=" + str(self.binary) + "\n")
        self.assertFalse(self.sentinel.exists())

    def test_prior_receipt_and_bootstrap_paths_reject_multiline(self) -> None:
        for suffix in ["\nextra=forged", "\r", "\r\n"]:
            result = self.run_step("Resolve prior resolved candidates",
                                   MANUAL_PRIOR_RESOLVED_CANDIDATES="prior" + suffix)
            self.assertNotEqual(result.returncode, 0)
            self.assertFalse(self.output.exists())
            for key in ["UB_REVIEW_BINARY_PATH", "UB_REVIEW_SOURCE_TARGET_DIR", "CARGO_TARGET_DIR", "RUNNER_TEMP"]:
                with self.subTest(binding=key, suffix=suffix):
                    result = self.run_step("Resolve ub-review runner", **{key: "path" + suffix})
                    self.assertNotEqual(result.returncode, 0)
                    self.assertIn("must not contain CR or LF", result.stderr)
                    self.assertFalse(self.output.exists())

    def test_post_and_gate_consume_configured_identity(self) -> None:
        for name, command in [("Post grouped PR review", "post"), ("Enforce gate outcome", "gate-check")]:
            result = self.run_step(name, UB_REVIEW_ACTION_OUT=self.literal,
                                   UB_REVIEW_GATE_OUTCOME_PATH=self.literal + "/review/gate_outcome.json",
                                   UB_REVIEW_ACTION_REVIEW_MODE=self.literal)
            self.assertEqual(result.returncode, 0, result.stderr)
            argv = json.loads(self.log.read_text())
            self.assertEqual(argv[0], command)
            if command == "gate-check":
                self.assertEqual(argv[argv.index("--gate-outcome") + 1], self.literal + "/review/gate_outcome.json")
                self.assertEqual(argv[argv.index("--review-mode") + 1], self.literal)
            else:
                self.assertEqual(argv[argv.index("--review-json") + 1], self.literal + "/review/github-review.json")
            self.assertFalse(self.sentinel.exists())

    def test_summary_reads_literal_path(self) -> None:
        out = self.root / self.literal.replace(str(self.root), "inert")
        out.mkdir(parents=True)
        (out / "running-summary.md").write_text("summary bytes\n")
        result = self.run_step("Append UB review summary", UB_REVIEW_ACTION_OUT=str(out))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((self.root / "summary").read_text(), "summary bytes\n")
        self.assertFalse(self.sentinel.exists())

    def test_derived_result_scalar_cannot_add_output_record(self) -> None:
        out = self.root / "packet/review"
        out.mkdir(parents=True)
        path = out / "gate_outcome.json"
        for field in ["conclusion", "analysis_result", "publication_result", "gate_result"]:
            for value in ["pass\nextra=forged", "pass\r", "pass\r\nextra=forged", "pass\n", "pa\0ss"]:
                self.output.unlink(missing_ok=True)
                path.write_text(json.dumps({field: value}))
                result = self.run_step("Publish gate result outputs", UB_REVIEW_GATE_OUTCOME_PATH=str(path))
                self.assertEqual(result.returncode, 0, result.stderr)
                records = dict(line.split("=", 1) for line in self.output.read_text().splitlines())
                self.assertEqual(len(records), 6)
                expected = "missing" if field == "conclusion" else "not_proven"
                self.assertEqual(records[field.replace("_", "-") if field != "conclusion" else "gate-conclusion"], expected)


if __name__ == "__main__":
    unittest.main(verbosity=2)
