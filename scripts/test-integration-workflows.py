#!/usr/bin/env python3
"""Exercise the pinned-baseline receipt and finalization boundary (#1465)."""
import copy
import io
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
import textwrap
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
BASELINE = (ROOT / '.github/workflows/independent-baseline.yml').read_text()
CODE = textwrap.dedent(BASELINE.split("python3 - <<'PY_RECEIPT'\n", 1)[1].split('          PY_RECEIPT', 1)[0])
B, G, P = '1' * 40, '2' * 40, '3' * 40
REPO = 'EffortlessMetrics/ub-review'
REFERENCE = {'path': REPO + '/.github/workflows/independent-baseline.yml@' + P, 'sha': P}


class BaselineReceipt(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.env = dict(EVENT_NAME='merge_group', BASE_SHA=B, EVENT_BASE_SHA=B,
                        EVENT_GROUP_SHA=G, CANDIDATE_SHA=G, DEFINITION_SHA=P,
                        TRUSTED_WORKFLOW_SHA=G, CANDIDATE_REPOSITORY=REPO,
                        REPOSITORY=REPO, RUN_ID='127', RUN_ATTEMPT='2',
                        EVIDENCE_RESULT='success', RUNNER_TEMP=self.tmp.name,
                        METADATA_TOKEN='read-only-fixture')
        self.run = dict(id=127, run_attempt=2, head_sha=G, event='merge_group',
                        repository={'full_name': REPO}, referenced_workflows=[REFERENCE])

    def evaluate(self):
        def response(request, timeout):
            self.assertEqual(request.full_url, 'https://api.github.com/repos/' + REPO + '/actions/runs/127/attempts/2')
            self.assertEqual(timeout, 30)
            self.assertEqual(request.get_method(), 'GET')
            return io.BytesIO(json.dumps(self.run).encode())
        with patch.dict(os.environ, self.env, clear=True), patch('urllib.request.urlopen', response):
            exec(compile(CODE, '<immutable baseline finalizer>', 'exec'), {})
        return json.loads((Path(self.tmp.name) / 'ub-review-independent-baseline/receipt.json').read_text())

    def test_real_integration_receipt_keeps_definition_separate_from_g(self):
        receipt = self.evaluate()
        self.assertEqual(receipt['candidate_sha'], G)
        self.assertEqual(receipt['group_sha'], G)
        self.assertEqual(receipt['base_sha'], B)
        self.assertEqual(receipt['trusted_workflow_sha'], P)
        self.assertEqual(receipt['run_attempt'], 2)
        self.assertEqual(receipt['referenced_workflow'], REFERENCE)
        self.assertIn('run-level reusable reference alone', receipt['authority_join'])
        self.assertEqual(len(receipt['commands']), 7)

    def test_event_or_subject_substitution_is_refused(self):
        original = dict(self.env)
        for field, value in [('EVENT_NAME', 'workflow_dispatch'), ('EVENT_BASE_SHA', G),
                             ('EVENT_GROUP_SHA', B), ('TRUSTED_WORKFLOW_SHA', P),
                             ('DEFINITION_SHA', 'main'), ('BASE_SHA', 'origin/main'),
                             ('CANDIDATE_SHA', B)]:
            with self.subTest(field=field), self.assertRaises(SystemExit):
                self.env = dict(original, **{field: value})
                self.evaluate()
        self.env = original

    def test_native_run_or_attempt_substitution_is_refused(self):
        original = copy.deepcopy(self.run)
        for field, value in [('id', 128), ('run_attempt', 1), ('head_sha', B),
                             ('event', 'pull_request'), ('repository', {'full_name': 'wrong/repo'})]:
            with self.subTest(field=field), self.assertRaises(SystemExit):
                self.run = dict(original, **{field: value})
                self.evaluate()
        self.run = original

    def test_missing_mutable_duplicated_or_wrong_definition_is_refused(self):
        for references in [[], [dict(REFERENCE, sha=G)], [dict(REFERENCE, path=REFERENCE['path'].replace(P, 'main'))], [REFERENCE, REFERENCE]]:
            with self.subTest(references=references), self.assertRaises(SystemExit):
                self.run['referenced_workflows'] = references
                self.evaluate()

    def test_legacy_protected_base_does_not_claim_g_or_make_api_request(self):
        self.env.update(EVENT_NAME='pull_request_target', TRUSTED_WORKFLOW_SHA=B)
        with patch('urllib.request.urlopen', side_effect=AssertionError('legacy receipt must not query a run')):
            with patch.dict(os.environ, self.env, clear=True):
                exec(compile(CODE, '<baseline legacy finalizer>', 'exec'), {})
        receipt = json.loads((Path(self.tmp.name) / 'ub-review-independent-baseline/receipt.json').read_text())
        self.assertEqual(receipt['workflow_source'], 'protected-base')
        self.assertEqual(receipt['trusted_workflow_sha'], B)
        self.assertNotIn('group_sha', receipt)

    def test_failure_is_recorded_without_becoming_success(self):
        self.env['EVIDENCE_RESULT'] = 'failure'
        self.assertEqual(self.evaluate()['evidence_result'], 'failure')

    def test_fresh_finalizer_enforces_every_non_success(self):
        tail = BASELINE.split('- name: Enforce independent baseline', 1)[1]
        code = textwrap.dedent(tail.split('run: |\n', 1)[1])
        for result in ['success', 'failure', 'skipped', 'cancelled', 'timed_out', '']:
            completed = subprocess.run(['bash', '-c', code], env=dict(os.environ, EVIDENCE_RESULT=result, CANDIDATE_SHA=G), capture_output=True)
            self.assertEqual(completed.returncode == 0, result == 'success', result)

    def test_candidate_and_finalizer_authority_stay_separate(self):
        evidence, finalize = BASELINE.split('\n  evidence:', 1)[1].split('\n  finalize:', 1)
        self.assertNotIn('github.token', evidence)
        self.assertNotIn('actions: read', evidence)
        self.assertIn('actions: read', finalize)
        for forbidden in ['actions/checkout', 'uses: ./', 'checks: write', 'pull-requests: write', 'id-token: write']:
            self.assertNotIn(forbidden, finalize)
        self.assertIn('check: [fmt, check, clippy, test, doc, policy, verifier]', evidence)


class QueuePortfolio(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.workflow = (ROOT / '.github/workflows/queue-qualification.yml').read_text()
        cls.code = textwrap.dedent(cls.workflow.split("python3 - <<'PY_RESULT'\n", 1)[1].split('          PY_RESULT', 1)[0])

    def evaluate(self, needs=None, **changed):
        env = dict(NEEDS_JSON=json.dumps(needs if needs is not None else {k: {'result': 'success'} for k in ('candidate', 'baseline', 'bootstrap')}),
                   EVENT_NAME='merge_group', BASE_SHA=B, GROUP_SHA=G, SUBJECT_SHA=G)
        env.update(changed)
        with patch.dict(os.environ, env, clear=True), patch('sys.stdout', io.StringIO()):
            exec(compile(self.code, '<queue result>', 'exec'), {})

    def test_complete_actual_g_portfolio(self):
        self.evaluate()

    def test_every_missing_skipped_or_failed_family_blocks(self):
        for family in ['candidate', 'baseline', 'bootstrap']:
            for result in [None, 'failure', 'skipped', 'cancelled', 'timed_out', 'neutral', '']:
                needs = {k: {'result': 'success'} for k in ('candidate', 'baseline', 'bootstrap')}
                if result is None:
                    del needs[family]
                else:
                    needs[family]['result'] = result
                with self.subTest(family=family, result=result), self.assertRaises(SystemExit):
                    self.evaluate(needs)

    def test_wrong_or_symbolic_integration_subject_cannot_pass(self):
        for field, value in [('EVENT_NAME', 'pull_request'), ('BASE_SHA', G), ('BASE_SHA', 'origin/main'), ('GROUP_SHA', B), ('SUBJECT_SHA', P)]:
            with self.subTest(field=field), self.assertRaises(SystemExit):
                self.evaluate(**{field: value})

    def test_full_baseline_reference_and_no_candidate_checkout_in_result(self):
        expected = 'bf077a0178c3fe159927a6ea39ee274050a92e10'
        self.assertIn('uses: EffortlessMetrics/ub-review/.github/workflows/independent-baseline.yml@' + expected, self.workflow)
        self.assertIn('definition_sha: ' + expected, self.workflow)
        result = self.workflow.split('\n  result:', 1)[1]
        self.assertIn('needs: [candidate, baseline, bootstrap]', result)
        self.assertIn('if: ${{ always() }}', result)
        self.assertIn('permissions: {}', result)
        self.assertNotIn('actions/checkout', result)
        self.assertNotIn('continue-on-error', result)


if __name__ == '__main__':
    unittest.main()
