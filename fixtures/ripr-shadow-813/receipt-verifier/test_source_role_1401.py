"""#813 owner evidence checks; not analyzer qualification or promotion."""
import copy
import hashlib
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
import verify

ROOT = Path(__file__).resolve().parent.parent / "source-role-1401"
CHAIN = "self.stdin(Stdio::null())\n            .stdout(Stdio::piped())\n            .stderr(Stdio::piped())\n            .fixture_spawn()?\n            .wait_with_output()"


def retained_production(raw, badge):
    verify.verify_unknowns(raw, badge, "candidate")
    if raw["summary"]["findings"] != 14 or raw["summary"]["probes"] != 14:
        raise ValueError("production denominator hidden")
    if badge["counts"]["unsuppressed_exposure_gaps"] != 7 or badge["counts"]["unknowns"] != 7:
        raise ValueError("production gap/unknown denominator changed")
    matching = [row for row in raw["findings"]
                if row["probe"]["family"] == "return_value"
                and row["probe"]["expression"] == CHAIN]
    if len(matching) != 1 or matching[0]["classification"] != "weakly_exposed":
        raise ValueError("analogous production capture subject hidden or strengthened")
    return matching[0]


class SourceRoleCase(unittest.TestCase):
    def test_documented_auditor_uses_only_committed_checkout_inputs(self):
        repo = Path(__file__).resolve().parents[3]
        evidence = ROOT / "evidence"
        relative = evidence.relative_to(repo)
        tracked = subprocess.check_output(
            ["git", "ls-files", "-z", "--", str(relative)], cwd=repo
        ).decode().split("\0")
        with tempfile.TemporaryDirectory() as scratch:
            fresh = Path(scratch)
            # Recreate files only: untracked empty directories cannot leak
            # from the author's workspace into the fresh-checkout proof.
            for name in filter(None, tracked):
                source = repo / name
                destination = fresh / source.relative_to(evidence)
                destination.parent.mkdir(parents=True, exist_ok=True)
                destination.write_bytes(source.read_bytes())
            result = subprocess.run(
                [sys.executable, str(Path(verify.__file__)), str(fresh)],
                capture_output=True, text=True, timeout=15,
            )
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_same_helper_bytes_and_production_capture_subjects_are_retained(self):
        helper = (ROOT / "original-helper-prefix.rs").read_bytes()
        self.assertEqual(hashlib.sha256(helper).hexdigest(), "e04650b6cf3ee9d8f29d804a3fb78ec79469fc8ece749553902960c637595df9")
        paths = {"test-layout": "tests/common/executable_fixture.rs",
                 "cfg-test-module": "src/executable_fixture.rs",
                 "production-normal": "src/executable_fixture.rs",
                 "production-test-name": "src/executable_fixture_tests.rs"}
        for name, path in paths.items():
            with self.subTest(name=name):
                self.assertEqual((ROOT / name / path).read_bytes(), helper)
                out = ROOT / "evidence/fixture-packets" / name / "candidate"
                self.assertEqual((out / "raw-1.stdout").read_bytes(), (out / "raw-2.stdout").read_bytes())
                raw, badge = verify.load(out / "raw-1.stdout"), verify.load(out / "badge.stdout")
                if name.startswith("production"):
                    row = retained_production(raw, badge)
                    self.assertEqual(row["probe"]["file"], "./" + path)
                else:
                    self.assertEqual(raw["findings"], [])
                    self.assertEqual(raw["summary"]["probes"], 0)
                    verify.verify_unknowns(raw, badge, "candidate")
                    self.assertEqual(badge["counts"]["unsuppressed_exposure_gaps"], 0)
                for invocation in verify.load(out / "invocations.json"):
                    self.assertEqual(invocation["exit_code"], 0)
                    self.assertEqual(invocation["source"], "75ecb7fdade48665aa48d01235fd9f7dfc0a3898")
        contract = verify.load(ROOT / "evidence/manifest.json")
        verify.verify_comparison_contract(contract)
        self.assertIsNone(contract["case_approver"])
        manifest = verify.load(ROOT / "FILE_MANIFEST.json")
        for member in manifest["files"]:
            self.assertNotIn("target", Path(member["path"]).parts)
            content = (ROOT / member["path"]).read_bytes()
            self.assertEqual(len(content), member["bytes"])
            self.assertEqual(hashlib.sha256(content).hexdigest(), member["sha256"])

    def test_even_consistent_zero_counts_cannot_hide_production_helper(self):
        out = ROOT / "evidence/fixture-packets/production-normal/candidate"
        raw, badge = verify.load(out / "raw-1.stdout"), verify.load(out / "badge.stdout")
        wrong_raw, wrong_badge = copy.deepcopy(raw), copy.deepcopy(badge)
        wrong_raw["findings"] = []
        for key in ["findings", "probes", "exposed", "weakly_exposed", "reachable_unrevealed", "no_static_path", "infection_unknown", "propagation_unknown", "static_unknown"]:
            wrong_raw["summary"][key] = 0
        for key in wrong_badge["counts"]:
            wrong_badge["counts"][key] = 0
        # Unknown accounting can reconcile while production is wrongly erased.
        verify.verify_unknowns(wrong_raw, wrong_badge, "candidate")
        with self.assertRaisesRegex(ValueError, "production denominator hidden"):
            retained_production(wrong_raw, wrong_badge)
        wrong_raw = copy.deepcopy(raw)
        for row in wrong_raw["findings"]:
            if row["probe"]["expression"] == CHAIN:
                row["probe"]["expression"] = "unrelated return value"
        with self.assertRaisesRegex(ValueError, "production capture subject hidden"):
            retained_production(wrong_raw, badge)


if __name__ == "__main__":
    unittest.main()
