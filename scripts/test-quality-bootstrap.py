#!/usr/bin/env python3
"""Offline transport, filesystem, and secret-retention controls for #1345."""
import importlib.util
import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('quality_bootstrap', Path(__file__).with_name('quality-bootstrap.py'))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
RUNS = [dict(databaseId=12, headSha='a'*40, conclusion='success',
             createdAt='2026-10-10T00:00:00Z', displayTitle='fixture', url='https://example.invalid/run')]
PULLS = [dict(number=7, title='fixture', state='OPEN', isDraft=False, mergedAt=None,
              reviewDecision=None, headRefOid='a'*40, comments=[], reviews=[], url='https://example.invalid/pr')]
def ok(value):
    return 0, json.dumps(value).encode(), b''

class Clock:
    def __init__(self): self.value, self.delays = 0, []
    def now(self): return self.value
    def sleep(self, seconds): self.delays.append(seconds); self.value += seconds

class BootstrapTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.out, self.diag = self.root/'source'/'github', self.root/'diagnostics'/'bootstrap.json'
        self.out.mkdir(parents=True)
        (self.out/'prior.json').write_bytes(b'previous-good-source')
        self.clock, self.calls = Clock(), []

    def collect(self, replies):
        items = iter(replies)
        def runner(argv, timeout):
            self.calls.append((argv, timeout))
            reply = next(items)
            if isinstance(reply, Exception): raise reply
            return reply
        return module.collect(self.out, self.diag, runner, self.clock.now, self.clock.sleep)

    def unchanged(self):
        self.assertEqual([p.name for p in self.out.iterdir()], ['prior.json'])
        self.assertEqual((self.out/'prior.json').read_bytes(), b'previous-good-source')

    def test_502_then_success_atomically_promotes_both_queries(self):
        r = self.collect([(1,b'',b'HTTP 502: Bad Gateway'), ok(RUNS), ok(PULLS)])
        self.assertTrue(r['collection_complete'])
        self.assertEqual(self.clock.delays, [1])
        self.assertEqual(len(self.calls), 3)
        self.assertEqual(json.loads((self.out/'actions-runs.json').read_text()), RUNS)
        self.assertEqual((self.out/'pr-numbers.txt').read_text(), '7\n')
        self.assertEqual((Path(r['previous_snapshot'])/'prior.json').read_bytes(), b'previous-good-source')

    def test_repeated_503_exhausts_without_empty_success_or_secrets(self):
        secret = b'HTTP 503 token=DO-NOT-RETAIN-THIS'
        r = self.collect([(1,b'',secret)]*4)
        self.assertFalse(r['collection_complete'])
        self.assertEqual(r['status'], 'upstream_unavailable')
        self.assertEqual(self.clock.delays, [1,3,7])
        self.assertEqual(len(self.calls), 4)
        self.assertNotIn('DO-NOT-RETAIN-THIS', self.diag.read_text())
        self.unchanged()

    def test_permanent_failures_do_not_retry(self):
        for message, expected in [(b'HTTP 401 Unauthorized','authorization_failed'),
          (b'HTTP 403 Forbidden','authorization_failed'),
          (b'HTTP 403 API rate limit exceeded','rate_limit_exhausted'),
          (b'GraphQL: Could not resolve','graphql_error'), (b'other error','client_read_failed')]:
            with self.subTest(message=message):
                before = len(self.calls)
                r = self.collect([(1,b'',message)])
                self.assertEqual(r['status'], expected)
                self.assertEqual(len(self.calls), before+1)
                self.unchanged()

    def test_bad_second_response_preserves_entire_previous_snapshot(self):
        for data in (b'{', b'null', b'{}', b'[{"number":7}]', b'[{"number":true}]'):
            with self.subTest(data=data):
                r = self.collect([ok(RUNS), (0,data,b'')])
                self.assertEqual(r['status'], 'malformed_response')
                self.unchanged()

    def test_duplicate_keys_rows_and_nonfinite_values_are_rejected(self):
        for data in (b'[{"databaseId":12,"databaseId":13}]',
                     b'[{"databaseId":12},{"databaseId":12}]',
                     b'[{"databaseId":12,"other":NaN}]'):
            with self.subTest(data=data):
                self.assertEqual(self.collect([(0,data,b'')])['status'], 'malformed_response')
                self.unchanged()

    def test_graphql_success_status_with_error_body_is_not_empty_data(self):
        r = self.collect([(0,b'{"errors":[{"message":"secret"}]}',b'')])
        self.assertEqual(r['status'], 'graphql_error')
        self.assertNotIn('secret', self.diag.read_text())
        self.unchanged()

    def test_timeout_is_retried_but_response_limit_is_not(self):
        r = self.collect([module.ReadFailure('transport_timeout', True),ok(RUNS),ok(PULLS)])
        self.assertTrue(r['collection_complete'])
        before = len(self.calls)
        r = self.collect([module.ReadFailure('response_too_large')])
        self.assertFalse(r['collection_complete'])
        self.assertEqual(len(self.calls), before+1)

    def test_whole_operation_deadline_is_enforced(self):
        def slow(argv, timeout):
            self.clock.value += 90
            return 1,b'',b'HTTP 502'
        r = module.collect(self.out,self.diag,slow,self.clock.now,self.clock.sleep)
        self.assertEqual(r['status'],'deadline_exhausted')
        self.assertFalse(r['collection_complete'])
        self.unchanged()

    def test_failed_promotion_restores_previous_directory(self):
        original = module.os.replace
        def replace(src,dst):
            if Path(src).name.startswith('.quality-bootstrap-'):
                raise OSError('injected promotion failure')
            return original(src,dst)
        with patch.object(module.os,'replace',side_effect=replace):
            r = self.collect([ok(RUNS),ok(PULLS)])
        self.assertEqual(r['status'],'local_io_failed')
        self.unchanged()

    def test_empty_valid_queries_are_explicit_zero_rows_not_calibration(self):
        r = self.collect([ok([]),ok([])])
        self.assertTrue(r['collection_complete'])
        self.assertEqual([x['rows'] for x in r['attempts']], [0,0])
        self.assertEqual(r['coverage'],'two bounded bootstrap queries only')

    def test_actual_transport_caps_streams_and_terminates_timeout(self):
        with self.assertRaises(module.ReadFailure) as caught:
            module.transport([sys.executable,'-c','import time; time.sleep(5)'], .05)
        self.assertEqual(caught.exception.reason,'transport_timeout')
        with self.assertRaises(module.ReadFailure) as caught:
            module.transport([sys.executable,'-c','import sys; sys.stderr.write("x"*70000)'], 2)
        self.assertEqual(caught.exception.reason,'response_too_large')
        self.assertEqual(module.transport([sys.executable,'-c','print("[]")'], 2)[0],0)

if __name__ == '__main__':
    unittest.main()
