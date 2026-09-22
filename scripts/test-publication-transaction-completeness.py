#!/usr/bin/env python3
"""Regression for complete prepared-to-terminal delivery accounting."""
from __future__ import annotations

import importlib.util
from pathlib import Path
import tempfile
import unittest

HERE = Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location(
    "publication_boundary_regressions",
    HERE / "test-publication-boundaries.py",
)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError("publication-boundary regression helpers could not be loaded")
fixtures = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(fixtures)


class PublicationTransactionCompleteness(unittest.TestCase):
    def test_subset_transaction_cannot_confirm_all_prepared_comments(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            comments = [
                {
                    "path": "src/first.rs",
                    "line": 7,
                    "side": "RIGHT",
                    "body": "[tests] Verify the first contract.",
                    "suggestion": None,
                },
                {
                    "path": "src/second.rs",
                    "line": 11,
                    "side": "RIGHT",
                    "body": "[tests] Verify the second contract.",
                    "suggestion": None,
                },
            ]
            identity = fixtures.base_packet(root, prepared=True, comments=comments)
            fixtures.post_result(root, identity)
            transaction = fixtures.read_json(root, "review/delivery-transaction.json")
            transaction["planned"] = transaction["planned"][:1]
            fixtures.write_json(root, "review/delivery-transaction.json", transaction)

            report = fixtures.subject.reconcile(root)
            codes = {row["code"] for row in report["issues"]}
            self.assertEqual(report["delivery_state"], "unverifiable")
            self.assertIn("delivery_transaction_incomplete", codes)
            self.assertTrue(report["input_unavailable"])


if __name__ == "__main__":
    unittest.main()
