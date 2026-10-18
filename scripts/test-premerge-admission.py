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
import sys
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
                        "review_ids": [55], "review_generation": ADMISSION.generation(self.reviews, [55]), "retry": None}
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
        self.comments = [{"id": 77, "user": self.owner, "body": self.body, "author_association": "MEMBER",
                          "created_at": "2026-10-18T00:00:01Z"}]
        self.anchor = self.row(self.floor, self.anchor_id, "legacy", "opened")
        self.own = self.row(self.floor + 1, self.run_id)
        self.rows = [self.own, self.anchor]
        self.events = [{"event": "labeled", "label": {"name": self.label}, "actor": self.owner, "created_at": "2026-10-18T00:00:10Z"}]
        self.extra = {}

    def row(self, number, run_id, title=None, action="labeled"):
        return {"id": run_id, "workflow_id": self.workflow_id, "run_number": number, "path": self.path,
                "repository": {"full_name": ADMISSION.REPO}, "actor": self.owner,
                "head_sha": "a" * 40, "event": self.event_name,
                "display_title": title or f"premerge-v1 1502 {'a'*40} {'b'*40} main {action} 30 {self.label}",
                "pull_requests": [{"number": 1502, "head": {"sha": "a" * 40},
                                   "base": {"sha": "b" * 40, "ref": "main"}}],
                "conclusion": None, "status": "queued", "run_attempt": 1, "updated_at": "2026-10-18T00:01:00Z"}

    def data(self):
        return {"/pulls/1502": self.live,
                "/issues/1502/comments?per_page=100&page=1": self.comments,
                "/issues/1465": {"state": "open"},
                "/collaborators/assigned-lead/permission": {"user": {"id": 30}, "permission": "write"},
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
               admission_result="success", attempt=1, bootstrap_failure=False, metadata_token="fixture-job-token"):
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
            (work / "python3").write_text('#!' + sys.executable + '\n' + textwrap.dedent('''
                import io, json, os, urllib.request, sys, runpy
                from pathlib import Path
                class Transport:
                    def open(self, request, timeout):
                        prefix = "https://api.github.com/repos/EffortlessMetrics/ub-review"
                        assert request.full_url.startswith(prefix) and timeout == 10
                        suffix = request.full_url[len(prefix):]
                        if suffix.startswith("/collaborators/"):
                            assert request.headers.get("Authorization") == "Bearer fixture-job-token"
                        else:
                            assert "Authorization" not in request.headers
                        with open(os.environ["TEST_CALLS"], "a") as log: log.write(suffix + "\\n")
                        data = json.loads(Path(os.environ["TEST_FIXTURE"]).read_text())
                        if suffix not in data: raise OSError("fixture metadata unavailable")
                        return io.BytesIO(json.dumps(data[suffix]).encode())
                urllib.request.build_opener = lambda *args: Transport()
                assert sys.argv[1] == "-I", "production protected helper must isolate Python imports"
                sys.argv = sys.argv[2:]
                runpy.run_path(sys.argv[0], run_name="__main__")
            '''))
            (work / "python3").chmod(0o700)
            (work / "curl").write_text('#!' + sys.executable + '\n' + textwrap.dedent('''
                import os, shutil, sys
                from pathlib import Path
                with open(os.environ["TEST_CALLS"], "a") as log: log.write("bootstrap\\n")
                if os.environ["TEST_BOOTSTRAP_FAILURE"] == "true": sys.exit(22)
                assert "https://raw.githubusercontent.com/EffortlessMetrics/ub-review/" + os.environ["ADAPTER_SOURCE_SHA"] + "/scripts/premerge-admission.py" in sys.argv
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
                   "ADMISSION_METADATA_TOKEN": metadata_token,
                   "POLICY_BASE_SHA": "b" * 40, "NATIVE_SHA": "c" * 40,
                   "ADAPTER_SOURCE_SHA": "b" * 40,
                   "ADAPTER_REPOSITORY": ADMISSION.REPO,
                   "ADAPTER_SHA256": re.search(r"^  ADAPTER_SHA256: (.+)$", source, re.M).group(1),
                   "POSSIBLE_ADMISSION": str(bool(expression(possible, self.context()))).lower()}
            result = subprocess.run(["bash", "-euo", "pipefail", "-c", shell(source, name)],
                                    env=env, capture_output=True, text=True, timeout=10, check=False)
            outputs = dict(line.split("=", 1) for line in output.read_text().splitlines()) if output.exists() else {}
            return result.returncode, outputs, calls.read_text().splitlines() if calls.exists() else [], result.stderr

    def retry(self, conclusions=None):
        conclusions = conclusions or {"gate": "failure", "baseline": "cancelled"}
        previous = copy.deepcopy(self.request)
        previous_row = self.own
        runs = {self.lane: previous_row["id"]}
        self.prior_rows = {self.lane: previous_row}
        other_lane = "baseline" if self.lane == "gate" else "gate"
        generation = previous["retry"]["generation"] + 1 if previous["retry"] else 1
        if generation == 1:
            self.other_fixture = Fixture(other_lane)
            self.other_fixture.own["id"] = 888001
            self.other_current = self.other_fixture.own
        other = self.other_fixture
        runs[other_lane] = self.other_current["id"]
        self.prior_rows[other_lane] = self.other_current
        for lane, row in self.prior_rows.items():
            row.update(status="completed", conclusion=conclusions[lane],
                       updated_at=f"2026-10-18T00:{2*generation-1:02d}:00Z")
        self.extra.update({f"/actions/runs/{other.anchor_id}": other.anchor,
                           f"/actions/workflows/{other.workflow_id}/runs?per_page=100&page=1": {"workflow_runs": other.rows}})
        self.request["retry"] = {"generation": generation, "prior_request": ADMISSION.digest(previous), "runs": runs}
        self.body = ADMISSION.canonical(self.request)
        old_label = self.label
        self.label = ADMISSION.label_for(self.body)
        self.comments.append({"id": 78, "user": self.owner, "body": self.body, "author_association": "MEMBER",
                              "created_at": f"2026-10-18T00:{2*generation:02d}:00Z"})
        self.live["labels"] = [{"name": self.label}]
        self.event["label"] = {"name": self.label}
        self.events.extend([{"event": "unlabeled", "label": {"name": old_label}, "actor": self.owner,
                             "created_at": f"2026-10-18T00:{2*generation:02d}:05Z"},
                            {"event": "labeled", "label": {"name": self.label}, "actor": self.owner,
                             "created_at": f"2026-10-18T00:{2*generation:02d}:10Z"}])
        self.run_id = previous_row["id"] + 1
        self.own = self.row(previous_row["run_number"] + 1, self.run_id)
        self.rows.insert(0, self.own)
        self.other_current = other.row(self.other_current["run_number"] + 1, self.other_current["id"] + 1,
                                       title=self.own["display_title"])
        other.rows.insert(0, self.other_current)

    def reseal(self):
        old_label = self.label
        self.body = ADMISSION.canonical(self.request)
        self.label = ADMISSION.label_for(self.body)
        self.comments[-1]["body"] = self.body
        self.live["labels"] = [{"name": self.label}]
        self.event["label"] = {"name": self.label}
        self.own["display_title"] = self.own["display_title"].replace(old_label, self.label)
        for edge in self.events:
            if edge.get("label", {}).get("name") == old_label:
                edge["label"]["name"] = self.label


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

    def test_two_valid_assignees_cannot_each_admit_same_subject(self):
        for lane in WORKFLOWS:
            for actor in (30, 32):
                f = Fixture(lane)
                second = {"id": 32, "login": "second-lead", "type": "User"}
                f.live["assignees"].append(second)
                request = {**f.request, "owner_id": 32}
                body = ADMISSION.canonical(request)
                label = ADMISSION.label_for(body)
                f.comments.append({"id": 79, "user": second, "body": body, "author_association": "MEMBER"})
                f.live["labels"].append({"name": label})
                f.extra["/collaborators/second-lead/permission"] = {"user": {"id": 32}, "permission": "write"}
                if actor == 32:
                    f.event.update(sender=second, label={"name": label})
                self.assert_suppressed(f)

    def test_explicit_terminal_retry_admits_both_populations_and_deduplicates(self):
        for lane, source in WORKFLOWS.items():
            for result in ("failure", "cancelled", "timed_out"):
                f = Fixture(lane)
                f.retry({"gate": result, "baseline": "success"})
                code, output, _, error = f.invoke(source, "Admit one explicit native request", "admit")
                self.assertEqual((code, output.get("mode")), (0, "admitted"), error)
                code, _, _, error = f.invoke(source, "Revalidate request before candidate checkout", "recheck", "admitted")
                self.assertEqual(code, 0, error)
                # Distinct run for the SAME explicit retry still returns its first winner.
                duplicate = f.row(f.floor+3, 999003)
                f.rows.insert(0, duplicate)
                f.run_id = 999003
                output, _ = self.assert_suppressed(f, "duplicate")
                self.assertEqual(output["winner"], "999002")
                code, output, _, _ = f.invoke(source, "Admit one explicit native request", "admit", attempt=2)
                self.assertEqual((code, output.get("mode")), (0, "replay"))

    def test_invalid_or_unauthorized_retry_never_allocates(self):
        def later_duplicate_predecessor(f):
            other = f.other_fixture
            row = copy.deepcopy(f.prior_rows[other.lane])
            row.update(id=777001, run_number=f.other_current['run_number']+1)
            other.rows.insert(0, row)
            f.request['retry']['runs'][other.lane] = row['id']
        mutations = {
            "queued": lambda f: f.prior_rows['gate'].update(status="queued", conclusion=None),
            "running": lambda f: f.prior_rows['baseline'].update(status="in_progress", conclusion=None),
            "success only": lambda f: [row.update(conclusion="success") for row in f.prior_rows.values()],
            "unknown terminal": lambda f: f.prior_rows['gate'].update(conclusion="neutral"),
            "prior rerun": lambda f: f.prior_rows['baseline'].update(run_attempt=2),
            "unknown prior": lambda f: f.request['retry'].update(prior_request='f'*64),
            "missing lane": lambda f: f.request['retry']['runs'].pop('gate'),
            "wrong winner": lambda f: f.request['retry']['runs'].update(gate=123),
            "later duplicate predecessor": later_duplicate_predecessor,
            "skipped generation": lambda f: f.request['retry'].update(generation=2),
            "over bound": lambda f: f.request['retry'].update(generation=4),
            "arbitrary nonce": lambda f: f.request['retry'].update(nonce='retry-again'),
            "changed subject": lambda f: f.request.update(objective='https://github.com/'+ADMISSION.REPO+'/issues/123'),
            "changed owner": lambda f: f.request.update(owner_id=99),
            "early comment": lambda f: f.comments[-1].update(created_at="2026-10-18T00:00:59Z"),
            "early edge": lambda f: f.events[-1].update(created_at="2026-10-18T00:00:59Z"),
            "other actor": lambda f: f.event.update(sender={"id": 99, "type": "User"}),
        }
        for lane in WORKFLOWS:
            for name, mutate in mutations.items():
                with self.subTest(lane=lane, condition=name):
                    f = Fixture(lane)
                    f.retry()
                    mutate(f)
                    f.reseal()
                    self.assert_suppressed(f)

    def test_retry_chain_is_consecutive_bounded_and_preserves_native_history(self):
        for lane, source in WORKFLOWS.items():
            f = Fixture(lane)
            for generation in (1, 2, 3):
                f.retry()
                self.assertEqual(f.request['retry']['generation'], generation)
                code, output, _, error = f.invoke(source, 'Admit one explicit native request', 'admit')
                self.assertEqual((code, output.get('mode')), (0, 'admitted'), error)
            # The protocol refuses a fourth successor even when prior runs failed.
            f.retry()
            self.assert_suppressed(f)

    def test_retry_serialization_cannot_create_another_transaction(self):
        for lane in WORKFLOWS:
            f = Fixture(lane)
            f.retry()
            alternate = json.dumps(f.request, indent=2)
            label = ADMISSION.label_for(alternate)
            f.comments.append({**f.comments[-1], 'id': 79, 'body': alternate})
            f.live['labels'] = [{'name': label}]
            f.event['label'] = {'name': label}
            f.events.append({'event': 'labeled', 'label': {'name': label}, 'actor': f.owner,
                             'created_at': '2026-10-18T00:02:11Z'})
            second = f.row(f.floor+3, 999003)
            second['display_title'] = second['display_title'].replace(f.label, label)
            f.rows.insert(0, second)
            f.run_id = 999003
            output, _ = self.assert_suppressed(f, 'duplicate')
            self.assertEqual(output['winner'], '999002')

    def test_protected_helper_token_is_isolated_and_never_sent_to_public_reads(self):
        for lane, source in WORKFLOWS.items():
            self.assertIn('ADAPTER_SOURCE_SHA: ${{ github.event.pull_request.base.sha }}', source)
            self.assertIn('ADAPTER_REPOSITORY: EffortlessMetrics/ub-review', source)
            for name in ("Admit one explicit native request", "Revalidate request before candidate checkout",
                         "Require current complete uncancelled qualification"):
                self.assertIn('python3 -I "$script"', shell(source, name))
            f = Fixture(lane)
            code, output, _, error = f.invoke(source, "Admit one explicit native request", "admit", metadata_token="")
            self.assertNotEqual(code, 0)
            self.assertEqual(output['mode'], 'rejected')
            code, output, _, error = f.invoke(source, "Admit one explicit native request", "admit")
            self.assertEqual(code, 0, error)
            self.assertNotIn('fixture-job-token', json.dumps(output)+error)
            worker = job(source, "gate" if lane == "gate" else "evidence")
            checkout = '- uses: actions/checkout@' if lane == 'gate' else '- name: Checkout exact candidate'
            self.assertNotIn('ADMISSION_METADATA_TOKEN', worker.split(checkout, 1)[1])
        with tempfile.TemporaryDirectory() as directory:
            work = Path(directory)
            marker = work/'candidate-imported'
            (work/'sitecustomize.py').write_text('from pathlib import Path\nPath('+repr(str(marker))+').touch()\n')
            f = Fixture('gate')
            f.event['action'] = 'opened'
            (work/'event').write_text(json.dumps(f.event))
            env = {**os.environ, 'PYTHONPATH': str(work), 'GITHUB_EVENT_PATH': str(work/'event'),
                   'GITHUB_OUTPUT': str(work/'out'), 'GITHUB_EVENT_NAME': 'pull_request', 'ADMISSION_LANE': 'gate',
                   'GITHUB_RUN_ID': '999001', 'GITHUB_RUN_ATTEMPT': '1', 'ADMISSION_PHASE': 'admit'}
            result = subprocess.run([sys.executable, '-I', str(SOURCE)], env=env, capture_output=True, timeout=5)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertFalse(marker.exists(), 'candidate import path reached protected helper')

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
            "permission lost": lambda f: f.extra.update({"/collaborators/assigned-lead/permission": {"user": {"id": 30}, "permission": "none"}}),
            "permission read only": lambda f: f.extra.update({"/collaborators/assigned-lead/permission": {"user": {"id": 30}, "permission": "read"}}),
            "permission unknown": lambda f: f.extra.update({"/collaborators/assigned-lead/permission": None}),
            "permission wrong user": lambda f: f.extra.update({"/collaborators/assigned-lead/permission": {"user": {"id": 99}, "permission": "write"}}),
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
                identity_start = actual.index('      - name: Verify exact native merge checkout\n')
                identity_end = actual.index('      - name: Verify task projection regressions\n', identity_start)
                actual = actual[:identity_start] + actual[identity_end:]
                actual = actual.replace("base: ${{ github.event.pull_request.base.sha }}", "base: ${{ github.event_name == 'pull_request' && format('origin/{0}', github.event.pull_request.base.ref) || 'origin/main' }}")
                actual = actual.replace("run-pass: manual", "run-pass: auto")
                actual = actual.replace("depth: standard", "depth: ${{ github.event_name == 'workflow_dispatch' && 'deep' || 'standard' }}")
            self.assertEqual(actual, job(baseline, worker), "proof command, matrix, permission or artifact drift")

    def test_actual_checkout_identity_and_immutable_diff_base(self):
        source = WORKFLOWS["gate"]
        self.assertIn("base: ${{ github.event.pull_request.base.sha }}", job(source, "gate"))
        command = shell(source, "Verify exact native merge checkout")
        for mutate in ("", "HEAD", "HEAD^1", "HEAD^2"):
            with tempfile.TemporaryDirectory(prefix="premerge-git-control-") as directory:
                work = Path(directory)
                (work / "git").write_text('#!/usr/bin/env python3\nimport os,sys\nkey=sys.argv[-1]\nvalue={"HEAD":"c"*40,"HEAD^1":"b"*40,"HEAD^2":"a"*40}[key]\nprint("e"*40 if key==os.environ["MUTATE_REF"] else value)\n')
                (work / "git").chmod(0o700)
                result = subprocess.run(["bash", "-euo", "pipefail", "-c", command],
                                        env={**os.environ, "PATH": directory + os.pathsep + os.environ["PATH"],
                                             "NATIVE_SHA": "c"*40, "EXPECTED_BASE": "b"*40,
                                             "EXPECTED_HEAD": "a"*40, "MUTATE_REF": mutate},
                                        check=False, capture_output=True)
                self.assertEqual(result.returncode == 0, mutate == "")

    def test_admission_failure_cannot_allocate_even_with_an_admitted_output(self):
        for lane, source in WORKFLOWS.items():
            f = Fixture(lane)
            worker = "gate" if lane == "gate" else "evidence"
            guard = re.search(r"^    if: (.+)$", job(source, worker), re.M).group(1)
            self.assertFalse(expression(guard, f.context({"mode": "admitted"}, "failure")))

    def test_missing_gate_artifact_cannot_finalize_required_success(self):
        f = Fixture("gate")
        f.extra[f"/actions/runs/{f.run_id}/artifacts?per_page=100&page=1"] = {"artifacts": []}
        code, _, _, _ = f.invoke(WORKFLOWS["gate"], "Require current complete uncancelled qualification", "finalize", "admitted")
        self.assertNotEqual(code, 0)

    def test_review_comment_does_not_erase_native_changes_requested(self):
        for lane, source in WORKFLOWS.items():
            f = Fixture(lane)
            f.reviews.insert(0, {**f.reviews[0], "id": 54, "state": "CHANGES_REQUESTED"})
            f.request["review_generation"] = ADMISSION.generation(f.reviews, f.request["review_ids"])
            f.body = ADMISSION.canonical(f.request)
            f.label = ADMISSION.label_for(f.body)
            f.comments[0]["body"] = f.body
            f.event["label"]["name"] = f.label
            f.live["labels"] = [{"name": f.label}]
            f.events[0]["label"]["name"] = f.label
            f.own["display_title"] = f.row(f.floor+1, f.run_id)["display_title"]
            self.assert_suppressed(f)

    def test_different_request_encoding_still_elects_original_transaction(self):
        for lane in WORKFLOWS:
            f = Fixture(lane)
            new_body = json.dumps(f.request, indent=2)
            new_label = ADMISSION.label_for(new_body)
            f.comments.append({**f.comments[0], "id": 78, "body": new_body})
            f.event["label"]["name"] = new_label
            f.live["labels"] = [{"name": new_label}]
            f.events = [{"event": "labeled", "label": {"name": new_label}, "actor": f.owner, "created_at": "2026-10-18T00:00:10Z"}]
            second = f.row(f.floor+2, 999002)
            second["display_title"] = second["display_title"].replace(f.label, new_label)
            f.rows.insert(0, second)
            f.run_id = 999002
            outputs, _ = self.assert_suppressed(f, "duplicate")
            self.assertEqual(outputs["winner"], "999001")

    def test_other_actors_labels_do_not_withdraw_owner_selection(self):
        for lane, source in WORKFLOWS.items():
            f = Fixture(lane)
            f.live["labels"].append({"name": "ci:" + "z"*43})
            code, outputs, _, error = f.invoke(source, "Admit one explicit native request", "admit")
            self.assertEqual((code, outputs.get("mode")), (0, "admitted"), error)

    def test_native_submitted_review_order_overrides_allocated_id(self):
        for lane in WORKFLOWS:
            f = Fixture(lane)
            f.reviews.extend([
                {**f.reviews[0], "id": 54, "state": "CHANGES_REQUESTED", "submitted_at": "2026-10-18T00:00:02Z"},
                {**f.reviews[0], "id": 56, "state": "APPROVED", "submitted_at": "2026-10-18T00:00:01Z"}])
            f.request["review_generation"] = ADMISSION.generation(f.reviews, f.request["review_ids"])
            f.body = ADMISSION.canonical(f.request)
            f.label = ADMISSION.label_for(f.body)
            f.comments[0]["body"] = f.body
            f.event["label"]["name"] = f.label
            f.live["labels"] = [{"name": f.label}]
            f.events[0]["label"]["name"] = f.label
            f.own["display_title"] = f.row(f.floor+1, f.run_id)["display_title"]
            self.assert_suppressed(f)

    def test_harmless_review_traffic_preserves_selected_generation(self):
        source = WORKFLOWS["gate"]
        for state in ("approved", "commented"):
            f = Fixture("gate")
            f.event_name = "pull_request_review"
            f.event.update(action="submitted", review={"id": 99, "state": state})
            outputs, calls = self.assert_suppressed(f, "observation")
            self.assertEqual(calls, [])
        f = Fixture("gate")
        f.reviews.append({**f.reviews[0], "id": 99, "state": "COMMENTED", "body": "Thanks"})
        code, outputs, _, error = f.invoke(source, "Admit one explicit native request", "admit")
        self.assertEqual((code, outputs.get("mode")), (0, "admitted"), error)
        for review_id, expected in ((99, "observation"), (55, "invalidated")):
            for action in ("dismissed", "edited"):
                f = Fixture("gate")
                f.event_name = "pull_request_review"
                f.event.update(action=action, review={"id": review_id, "state": "dismissed"})
                self.assert_suppressed(f, expected)
        f = Fixture("gate")
        blocked = f.row(f.floor+2, 999002)
        blocked["event"] = "pull_request_review"
        blocked["display_title"] = blocked["display_title"].replace("labeled 30 " + f.label,
                                                                    "submitted 30 review:99:changes_requested")
        f.rows.insert(0, blocked)
        code, _, _, _ = f.invoke(source, "Revalidate request before candidate checkout", "recheck", "admitted")
        self.assertNotEqual(code, 0, "withdrawn blocking state restored stale request")

    def test_native_review_retirement_is_independent_of_gate_run_history(self):
        for lane, source in WORKFLOWS.items():
            for dismissed in (False, True):
                with self.subTest(lane=lane, dismissed=dismissed):
                    f = Fixture(lane)
                    f.reviews.extend([
                        {**f.reviews[0], "id": 56, "state": "DISMISSED" if dismissed else "CHANGES_REQUESTED", "submitted_at": "2026-10-18T00:00:11Z"},
                        {**f.reviews[0], "id": 57, "state": "APPROVED", "submitted_at": "2026-10-18T00:00:12Z"}])
                    if dismissed:
                        f.events.append({"event": "review_dismissed", "created_at": "2026-10-18T00:00:13Z",
                                         "dismissed_review": {"review_id": "56", "state": "changes_requested"}})
                    self.assert_suppressed(f)

    def test_public_transport_refuses_byte_and_request_overflow_and_redirect(self):
        api = ADMISSION.PublicGitHub()
        api.calls = 40
        with self.assertRaisesRegex(ADMISSION.Refusal, "request bound"):
            api.get("/pulls/1502")
        api = ADMISSION.PublicGitHub()
        class Transport:
            def open(self, _request, timeout):
                return io.BytesIO(b" " * 2_000_001)
        api.opener = Transport()
        with self.assertRaisesRegex(ADMISSION.Refusal, "byte bound"):
            api.get("/pulls/1502")
        redirect = next(handler for handler in ADMISSION.PublicGitHub().opener.handlers
                        if hasattr(handler, "redirect_request"))
        with self.assertRaisesRegex(ADMISSION.Refusal, "redirect"):
            redirect.redirect_request(None, None, 302, None, None, "https://other.example/")

    def test_parsing_bounds_and_digest_custody(self):
        with self.assertRaises(ADMISSION.Refusal): ADMISSION.decode('{"schema":1,"schema":2}')
        with self.assertRaises(ADMISSION.Refusal): ADMISSION.decode('{"schema":NaN}')
        api = ADMISSION.PublicGitHub()
        api.get = lambda suffix: [{}] * 100
        with self.assertRaisesRegex(ADMISSION.Refusal, "pagination"): api.pages("/pulls/1502/reviews")
        self.assertIn('ADAPTER_SOURCE_SHA: ${{ github.event.pull_request.base.sha }}', WORKFLOWS['baseline'])
        self.assertIn('ADAPTER_REPOSITORY: EffortlessMetrics/ub-review', WORKFLOWS['baseline'])
        self.assertIn('ADAPTER_SOURCE_SHA: ${{ github.event.pull_request.base.sha }}', WORKFLOWS['gate'])
        for source in WORKFLOWS.values():
            self.assertIn(hashlib.sha256(SOURCE.read_bytes()).hexdigest(), source)
            self.assertIn('echo "$ADAPTER_SHA256  $script" | sha256sum --check --status', source)
            self.assertIn('https://raw.githubusercontent.com/$ADAPTER_REPOSITORY/$ADAPTER_SOURCE_SHA/', source)
            self.assertNotIn('pull-requests:', source)
            self.assertNotIn('checks: write', source)
            self.assertIn('cancel-in-progress: false', source)
            self.assertIn("format('observe-{0}', github.run_id)", source)
            self.assertIn("format('request-{0}', github.event.pull_request.number)", source)


if __name__ == "__main__":
    unittest.main(verbosity=2)
