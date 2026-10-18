#!/usr/bin/env python3
"""Cheap production adapter/YAML-path controls; never run product qualification."""
import copy
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
import textwrap
import unittest

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / "scripts/premerge-admission.py"
SPEC = importlib.util.spec_from_file_location("admission", SOURCE)
ADMISSION = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(ADMISSION)
WORKFLOWS = {lane: (ROOT / path).read_text() for lane, path in
             (("gate", ".github/workflows/ub-review-gate.yml"),
              ("baseline", ".github/workflows/independent-baseline.yml"))}


def shell(source, name):
    tail = source.split("- name: " + name + "\n", 1)[1]
    lines = tail.splitlines()
    start = next(i for i, line in enumerate(lines) if line.strip() == "run: |") + 1
    indent = len(lines[start]) - len(lines[start].lstrip())
    result = []
    for line in lines[start:]:
        if line.strip() and len(line) - len(line.lstrip()) < indent:
            break
        result.append(line[indent:])
    return "\n".join(result) + "\n"


def job(source, name):
    tail = source.split("\n  " + name + ":\n", 1)[1]
    return re.split(r"\n  [a-z][a-z-]*:\n", tail, maxsplit=1)[0]


class Missing(str):
    def __getattr__(self, _key):
        return self
    def __eq__(self, other):
        return other is None or str.__eq__(self, other)
    def __ne__(self, other):
        return not self == other


class Lookup(dict):
    def __getattr__(self, key):
        value = self.get(key, Missing(""))
        return Lookup(value) if isinstance(value, dict) else value


def expression(value, context):
    """Execute the small native expressions from the actual YAML, not a replica guard."""
    value = value.strip().removeprefix("${{").removesuffix("}}")
    value = value.replace("&&", " and ").replace("||", " or ")
    value = re.sub(r"!(?!=)", "not ", value)
    for token, replacement in (("true", "True"), ("false", "False"), ("null", "None")):
        # Quoted output strings ('true') deliberately remain strings.
        value = re.sub(r"(?<![A-Za-z0-9_'\"])\b" + token + r"\b(?!['\"])", replacement, value)
    return eval(value, {"__builtins__": {}},
                {**{key: Lookup(item) for key, item in context.items()},
                 "startsWith": lambda item, prefix: str(item).lower().startswith(prefix.lower()),
                 "format": lambda fmt, *args: fmt.format(*args), "always": lambda: True,
                 "cancelled": lambda: False})


class Fixture:
    def __init__(self, lane):
        self.lane = lane
        self.workflow_id, self.anchor_id, self.floor, self.path = ADMISSION.ANCHORS[lane]
        self.run_id = 999001
        self.owner = {"id": 30, "login": "assigned-lead", "type": "User"}
        self.reviews = [{"id": 55, "user": {"id": 31, "type": "User"}, "state": "COMMENTED",
                         "commit_id": "a" * 40, "submitted_at": "2026-10-18T00:00:00Z",
                         "body": "Independent exact-head report; no material findings in bounded scope.",
                         "author_association": "COLLABORATOR"}]
        self.request = {"schema": ADMISSION.SCHEMA, "repository": ADMISSION.REPO, "pr": 1502,
                        "head_repository": ADMISSION.REPO, "head_sha": "a" * 40,
                        "base_ref": "main", "base_sha": "b" * 40,
                        "merge_sha": "c" * 40, "merge_tree": "d" * 40,
                        "policy_sha256": hashlib.sha256(SOURCE.read_bytes()).hexdigest(),
                        "owner_id": 30, "objective": "https://github.com/" + ADMISSION.REPO + "/issues/1465",
                        "review_ids": [55], "review_generation": ADMISSION.generation(self.reviews)}
        self.body = ADMISSION.canonical(self.request)
        self.label = ADMISSION.label_for(self.body)
        self.live = {"number": 1502, "state": "open", "draft": False, "mergeable": True,
                     "head": {"sha": "a" * 40, "repo": {"full_name": ADMISSION.REPO}},
                     "base": {"sha": "b" * 40, "ref": "main", "repo": {"full_name": ADMISSION.REPO}},
                     "merge_commit_sha": "c" * 40, "assignees": [self.owner], "labels": [{"name": self.label}]}
        self.event = {"action": "labeled", "label": {"name": self.label}, "sender": self.owner,
                      "repository": {"full_name": ADMISSION.REPO, "private": False},
                      "pull_request": copy.deepcopy(self.live)}
        self.event_name = "pull_request_target" if lane == "baseline" else "pull_request"
        self.comments = [{"id": 77, "user": self.owner, "body": self.body, "author_association": "MEMBER"}]
        self.anchor = self.row(self.floor, self.anchor_id, "legacy", "opened")
        self.own = self.row(self.floor + 1, self.run_id)
        self.rows = [self.own, self.anchor]
        self.events = [{"event": "labeled", "label": {"name": self.label}, "actor": self.owner}]
        self.extra = {}

    def row(self, number, run_id, title=None, action="labeled"):
        return {"id": run_id, "workflow_id": self.workflow_id, "run_number": number, "path": self.path,
                "repository": {"full_name": ADMISSION.REPO}, "actor": self.owner,
                "head_sha": "a" * 40, "event": self.event_name,
                "display_title": title or f"premerge-v1 1502 {'a'*40} {'b'*40} main {action} 30 {self.label}",
                "pull_requests": [{"number": 1502, "head": {"sha": "a" * 40},
                                   "base": {"sha": "b" * 40, "ref": "main"}}],
                "conclusion": None, "status": "queued"}

    def data(self):
        return {"/pulls/1502": self.live,
                "/issues/1502/comments?per_page=100&page=1": self.comments,
                "/issues/1465": {"state": "open"},
                "/pulls/1502/reviews?per_page=100&page=1": self.reviews,
                "/git/commits/" + "c" * 40: {"sha": "c" * 40, "tree": {"sha": "d" * 40},
                                             "parents": [{"sha": "b" * 40}, {"sha": "a" * 40}]},
                "/issues/1502/events?per_page=100&page=1": self.events,
                f"/actions/runs/{self.anchor_id}": self.anchor,
                f"/actions/runs/{self.run_id}/artifacts?per_page=100&page=1": {"artifacts": [
                    {"name": "ub-review-gate", "expired": False, "size_in_bytes": 100,
                     "digest": "sha256:" + "f"*64, "workflow_run": {"id": self.run_id, "head_sha": "a"*40}}]},
                f"/actions/workflows/{self.workflow_id}/runs?per_page=100&page=1": {"workflow_runs": self.rows},
                **self.extra}

    def context(self, outputs=None, result="success"):
        return {"github": {"event": self.event, "event_name": self.event_name},
                "needs": {"admission": {"outputs": outputs or {}, "result": result},
                          "gate": {"result": "skipped"}, "evidence": {"result": "skipped"}}}

    def invoke(self, source, name, phase, mode="", product="success", cancelled=False,
               admission_result="success", attempt=1, bootstrap_failure=False):
        with tempfile.TemporaryDirectory(prefix="premerge-control-") as directory:
            work = Path(directory)
            fixture = work / "fixture.json"
            fixture.write_text(json.dumps(self.data()))
            event = work / "event.json"
            event.write_text(json.dumps(self.event))
            output = work / "output"
            calls = work / "calls"
            # Inject the transport beneath the real PublicGitHub reader in a fresh process.
            # Unknown URLs raise; no test can silently contact GitHub.
            (work / "sitecustomize.py").write_text(textwrap.dedent('''
                import io, json, os, urllib.request
                from pathlib import Path
                class Transport:
                    def open(self, request, timeout):
                        prefix = "https://api.github.com/repos/EffortlessMetrics/ub-review"
                        assert request.full_url.startswith(prefix) and timeout == 10
                        assert "Authorization" not in request.headers
                        suffix = request.full_url[len(prefix):]
                        with open(os.environ["TEST_CALLS"], "a") as log: log.write(suffix + "\\n")
                        data = json.loads(Path(os.environ["TEST_FIXTURE"]).read_text())
                        if suffix not in data: raise OSError("fixture metadata unavailable")
                        return io.BytesIO(json.dumps(data[suffix]).encode())
                urllib.request.build_opener = lambda *args: Transport()
            '''))
            (work / "curl").write_text('#!/usr/bin/env python3\n' + textwrap.dedent('''
                import os, shutil, sys
                from pathlib import Path
                with open(os.environ["TEST_CALLS"], "a") as log: log.write("bootstrap\\n")
                if os.environ["TEST_BOOTSTRAP_FAILURE"] == "true": sys.exit(22)
                assert "https://raw.githubusercontent.com/EffortlessMetrics/ub-review/" + "b"*40 + "/scripts/premerge-admission.py" in sys.argv
                shutil.copyfile(os.environ["TEST_SOURCE"], sys.argv[sys.argv.index("-o")+1])
            '''))
            (work / "curl").chmod(0o700)
            possible = re.search(r"^  POSSIBLE_ADMISSION: (.+)$", source, re.M).group(1)
            env = {**os.environ, "PATH": str(work) + os.pathsep + os.environ["PATH"],
                   "PYTHONPATH": str(work), "TEST_CALLS": str(calls), "TEST_FIXTURE": str(fixture),
                   "TEST_SOURCE": str(SOURCE), "TEST_BOOTSTRAP_FAILURE": str(bootstrap_failure).lower(),
                   "RUNNER_TEMP": str(work), "GITHUB_EVENT_PATH": str(event), "GITHUB_OUTPUT": str(output),
                   "GITHUB_EVENT_NAME": self.event_name, "GITHUB_RUN_ID": str(self.run_id),
                   "GITHUB_RUN_ATTEMPT": str(attempt), "ADMISSION_LANE": self.lane,
                   "ADMISSION_PHASE": phase, "ADMISSION_MODE": mode, "PRODUCT_RESULT": product,
                   "ADMISSION_RESULT": admission_result, "WORKFLOW_CANCELLED": str(cancelled).lower(),
                   "POLICY_BASE_SHA": "b" * 40, "NATIVE_SHA": "c" * 40,
                   "ADAPTER_SHA256": re.search(r"^  ADAPTER_SHA256: (.+)$", source, re.M).group(1),
                   "POSSIBLE_ADMISSION": str(bool(expression(possible, self.context()))).lower()}
            result = subprocess.run(["bash", "-euo", "pipefail", "-c", shell(source, name)],
                                    env=env, capture_output=True, text=True, timeout=10, check=False)
            outputs = dict(line.split("=", 1) for line in output.read_text().splitlines()) if output.exists() else {}
            return result.returncode, outputs, calls.read_text().splitlines() if calls.exists() else [], result.stderr


class NativeAdmission(unittest.TestCase):
    def assert_suppressed(self, fixture, expected=None):
        source = WORKFLOWS[fixture.lane]
        code, outputs, calls, error = fixture.invoke(source, "Admit one explicit native request", "admit")
        if expected:
            self.assertEqual(outputs.get("mode"), expected, error)
        worker = "gate" if fixture.lane == "gate" else "evidence"
        guard = re.search(r"^    if: (.+)$", job(source, worker), re.M).group(1)
        context = fixture.context(outputs, "success" if code == 0 else "failure")
        self.assertFalse(expression(guard, context), "production worker guard allocated expensive execution")
        name = re.search(r"^    name: (.+)$", "\n" + job(source, "finalize"), re.M).group(1)
        native_name = expression(name, context)
        final_code, _, _, _ = fixture.invoke(source, "Require current complete uncancelled qualification",
                                            "finalize", outputs.get("mode", ""),
                                            admission_result="success" if code == 0 else "failure")
        if final_code == 0:
            self.assertTrue(native_name.startswith("admission observation"), "non-admitted required-context success")
        return outputs, calls

    def test_ordinary_events_have_no_reads_no_workers_and_no_successful_required_context(self):
        for lane in WORKFLOWS:
            for action in ("opened", "reopened", "ready_for_review", "edited", "labeled", "unlabeled"):
                for draft in (False, True):
                    with self.subTest(lane=lane, action=action, draft=draft):
                        f = Fixture(lane)
                        f.event.update(action=action, label={"name": "unrelated"})
                        f.event["pull_request"]["draft"] = draft
                        outputs, calls = self.assert_suppressed(f, "observation")
                        self.assertEqual(calls, [], "ordinary event contacted metadata/source or product")
            f = Fixture(lane)
            f.event_name = "workflow_dispatch"
            self.assert_suppressed(f, "observation")

    def test_material_events_never_allocate_and_never_write_required_success(self):
        for lane in WORKFLOWS:
            for action in ("synchronize", "converted_to_draft", "closed", "unlabeled", "edited"):
                with self.subTest(lane=lane, action=action):
                    f = Fixture(lane)
                    f.event["action"] = action
                    if action == "edited": f.event["changes"] = {"base": {"ref": {"from": "other"}}}
                    self.assert_suppressed(f, "invalidated")

    def test_explicit_request_reaches_real_worker_preflight_and_finalizer(self):
        for lane, source in WORKFLOWS.items():
            f = Fixture(lane)
            code, outputs, _, error = f.invoke(source, "Admit one explicit native request", "admit")
            self.assertEqual(code, 0, error)
            self.assertEqual(outputs["mode"], "admitted")
            worker = "gate" if lane == "gate" else "evidence"
            guard = re.search(r"^    if: (.+)$", job(source, worker), re.M).group(1)
            self.assertTrue(expression(guard, f.context(outputs)))
            code, _, _, error = f.invoke(source, "Revalidate request before candidate checkout", "recheck", "admitted")
            self.assertEqual(code, 0, error)
            code, outputs, _, error = f.invoke(source, "Require current complete uncancelled qualification", "finalize", "admitted")
            self.assertEqual(code, 0, error)
            self.assertEqual(outputs["mode"], "admitted")

    def test_stale_malformed_and_unavailable_requests_never_allocate(self):
        mutations = {
            "head moved": lambda f: f.live["head"].update(sha="e" * 40),
            "base moved": lambda f: f.live["base"].update(sha="e" * 40),
            "retarget": lambda f: f.live["base"].update(ref="other"),
            "head repository": lambda f: f.live["head"]["repo"].update(full_name="other/repo"),
            "owner removed": lambda f: f.live.update(assignees=[]),
            "wrong actor": lambda f: f.event.update(sender={"id": 99, "type": "User"}),
            "App actor": lambda f: f.event.update(sender={"id": 30, "type": "Bot"}),
            "draft": lambda f: f.live.update(draft=True),
            "closed": lambda f: f.live.update(state="closed"),
            "merge unavailable": lambda f: f.live.update(mergeable=None),
            "withdrawal": lambda f: f.live.update(labels=[]),
            "reapplied request": lambda f: f.events.append({"event": "unlabeled", "label": {"name": f.label}, "actor": f.owner}),
            "review changed": lambda f: f.reviews[0].update(body="new material finding"),
            "blocking review": lambda f: f.reviews.append({**f.reviews[0], "id": 56, "state": "CHANGES_REQUESTED"}),
            "request edited": lambda f: f.comments[0].update(body=f.body + " "),
            "missing request": lambda f: f.comments.clear(),
            "objective closed": lambda f: f.extra.update({"/issues/1465": {"state": "closed"}}),
            "merge subject moved": lambda f: f.live.update(merge_commit_sha="e" * 40),
            "merge parents": lambda f: f.extra.update({"/git/commits/" + "c"*40: {"sha": "c"*40, "tree": {"sha": "d"*40}, "parents": []}}),
            "metadata failure": lambda f: f.extra.update({"/pulls/1502": None}),
            "missing history": lambda f: f.rows.clear(),
            "deleted history row": lambda f: f.rows.insert(0, f.row(f.floor+3, 999003)),
            "wrong workflow": lambda f: f.own.update(workflow_id=1),
            "wrong native subject": lambda f: f.own.update(head_sha="e"*40),
            "wrong native PR": lambda f: f.own.update(pull_requests=[]),
            "forged run title": lambda f: f.own.update(display_title=f.own["display_title"].replace(" 30 ", " 99 ")),
        }
        for lane in WORKFLOWS:
            for description, mutate in mutations.items():
                with self.subTest(lane=lane, condition=description):
                    f = Fixture(lane)
                    mutate(f)
                    self.assert_suppressed(f)

    def test_every_worker_and_finalizer_rechecks_currentness(self):
        for lane, source in WORKFLOWS.items():
            for mutate in (lambda f: f.live["head"].update(sha="e"*40),
                           lambda f: f.live["base"].update(sha="e"*40),
                           lambda f: f.live.update(labels=[]),
                           lambda f: f.reviews[0].update(state="CHANGES_REQUESTED")):
                for phase, name in (("recheck", "Revalidate request before candidate checkout"),
                                    ("finalize", "Require current complete uncancelled qualification")):
                    with self.subTest(lane=lane, phase=phase):
                        f = Fixture(lane)
                        mutate(f)
                        code, _, _, _ = f.invoke(source, name, phase, "admitted")
                        self.assertNotEqual(code, 0, "stale request crossed real production boundary")

    def test_duplicate_lost_response_and_replay_preserve_original_result(self):
        for lane in WORKFLOWS:
            for status, conclusion in (("queued", None), ("in_progress", None),
                                       ("completed", "success"), ("completed", "failure"),
                                       ("completed", "cancelled")):
                with self.subTest(lane=lane, status=status, conclusion=conclusion):
                    f = Fixture(lane)
                    f.own["status"], f.own["conclusion"] = status, conclusion
                    second = f.row(f.floor+2, 999002)
                    f.rows.insert(0, second)
                    f.run_id = 999002
                    outputs, _ = self.assert_suppressed(f, "duplicate")
                    self.assertEqual(outputs["winner"], "999001")
            f = Fixture(lane)
            # A later webhook can start first; election uses native sequence, never start order.
            f.rows.insert(0, f.row(f.floor+2, 999002))
            code, output, _, error = f.invoke(WORKFLOWS[lane], "Admit one explicit native request", "admit")
            self.assertEqual(code, 0, error)
            self.assertEqual(output["mode"], "admitted")
            code, output, calls, _ = f.invoke(WORKFLOWS[lane], "Admit one explicit native request", "admit", attempt=2)
            self.assertEqual((code, output["mode"]), (0, "replay"))
            self.assertEqual(calls, ["bootstrap"])

    def test_material_event_after_admission_permanently_retires_request(self):
        for lane, source in WORKFLOWS.items():
            for action in ("synchronize", "converted_to_draft", "closed", "unlabeled"):
                with self.subTest(lane=lane, action=action):
                    f = Fixture(lane)
                    f.rows.insert(0, f.row(f.floor+2, 999002, action=action))
                    code, _, _, _ = f.invoke(source, "Revalidate request before candidate checkout", "recheck", "admitted")
                    self.assertNotEqual(code, 0)

    def test_failure_skip_cancellation_and_bootstrap_never_produce_required_success(self):
        for lane, source in WORKFLOWS.items():
            for product in ("failure", "cancelled", "skipped", ""):
                f = Fixture(lane)
                code, _, calls, _ = f.invoke(source, "Require current complete uncancelled qualification", "finalize", "admitted", product)
                self.assertNotEqual(code, 0)
                self.assertEqual(calls, [])
            f = Fixture(lane)
            code, _, calls, _ = f.invoke(source, "Require current complete uncancelled qualification", "finalize", "admitted", cancelled=True)
            self.assertNotEqual(code, 0)
            self.assertEqual(calls, [])
            code, outputs, _, _ = f.invoke(source, "Admit one explicit native request", "admit", bootstrap_failure=True)
            self.assertNotEqual(code, 0)
            self.assertNotEqual(outputs.get("mode"), "admitted")
            self.assertIn("if: ${{ cancelled() }}\n        shell: bash\n        run: exit 1", source)

    def test_complete_proof_workers_are_preserved(self):
        for lane, source in WORKFLOWS.items():
            path = ".github/workflows/" + ("ub-review-gate.yml" if lane == "gate" else "independent-baseline.yml")
            baseline = subprocess.check_output(["git", "show", "ac06293dd0871531c1826e4a3266c8dc537cf4f6:" + path], cwd=ROOT, text=True)
            worker = "gate" if lane == "gate" else "evidence"
            actual = job(source, worker)
            actual = actual.replace("    needs: admission\n", "", 1).replace("    if: ${{ !cancelled() && needs.admission.result == 'success' && needs.admission.outputs.mode == 'admitted' }}\n", "", 1)
            start = actual.index("      - name: Revalidate request before candidate checkout\n")
            end = actual.index("      - ", start + len("      - name: Revalidate request before candidate checkout\n"))
            actual = actual[:start] + actual[end:]
            if lane == "gate":
                actual = actual.replace("name: contained candidate evidence", "name: ub-review/gate")
                actual = actual.replace("          ref: ${{ github.sha }}\n", "", 1)
                actual = actual.replace('      - name: Verify exact native merge checkout\n        shell: bash\n        run: test "$(git rev-parse HEAD)" = "$NATIVE_SHA"\n\n', '', 1)
                actual = actual.replace("run-pass: manual", "run-pass: auto")
                actual = actual.replace("depth: standard", "depth: ${{ github.event_name == 'workflow_dispatch' && 'deep' || 'standard' }}")
            self.assertEqual(actual, job(baseline, worker), "proof command, matrix, permission or artifact drift")

    def test_parsing_bounds_and_digest_custody(self):
        with self.assertRaises(ADMISSION.Refusal): ADMISSION.decode('{"schema":1,"schema":2}')
        with self.assertRaises(ADMISSION.Refusal): ADMISSION.decode('{"schema":NaN}')
        api = ADMISSION.PublicGitHub()
        api.get = lambda suffix: [{}] * 100
        with self.assertRaisesRegex(ADMISSION.Refusal, "pagination"): api.pages("/pulls/1502/reviews")
        for source in WORKFLOWS.values():
            self.assertIn(hashlib.sha256(SOURCE.read_bytes()).hexdigest(), source)
            self.assertIn('echo "$ADAPTER_SHA256  $script" | sha256sum --check --status', source)
            self.assertIn('https://raw.githubusercontent.com/EffortlessMetrics/ub-review/$POLICY_BASE_SHA/', source)
            self.assertNotIn('pull-requests:', source)
            self.assertNotIn('checks: write', source)
            self.assertIn('cancel-in-progress: false', source)
            self.assertIn("format('observe-{0}', github.run_id)", source)
            self.assertIn("format('request-{0}', github.event.pull_request.number)", source)


if __name__ == "__main__":
    unittest.main(verbosity=2)
