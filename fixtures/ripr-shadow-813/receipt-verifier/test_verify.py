# Owner: ub-review/tests, #813. Negative controls for comparison-only claims.
import copy
import unittest
import verify


class ComparisonBoundaryTests(unittest.TestCase):
    def setUp(self):
        self.contract = {
            'schema': 'ub-review.ripr_frozen_receipt.v1',
            'authority': 'shadow-only',
            'decision': 'comparison_only_no_promotion',
        }
        self.raw = {
            'summary': {'infection_unknown': 0, 'propagation_unknown': 1,
                        'static_unknown': 0},
            'findings': [{'classification': 'propagation_unknown',
                          'source_currentness': 'candidate_current'}],
        }
        self.badge = {'counts': {'unknowns': 1, 'unsuppressed_exposure_gaps': 0}}

    def test_zero_gaps_retains_unknown_and_no_promotion(self):
        verify.verify_comparison_contract(self.contract)
        result = verify.verify_unknowns(self.raw, self.badge, 'candidate')
        self.assertEqual(result, {'infection_unknown': 0, 'propagation_unknown': 1,
                                  'static_unknown': 0})
        self.assertEqual(self.badge['counts']['unsuppressed_exposure_gaps'], 0)
        self.assertEqual(self.contract['decision'], 'comparison_only_no_promotion')

    def test_missing_native_unknown_is_rejected(self):
        del self.badge['counts']['unknowns']
        with self.assertRaisesRegex(ValueError, 'native unknown count'):
            verify.verify_unknowns(self.raw, self.badge, 'candidate')

    def test_zeroing_native_unknown_is_rejected(self):
        self.badge['counts']['unknowns'] = 0
        with self.assertRaisesRegex(ValueError, 'native unknown count'):
            verify.verify_unknowns(self.raw, self.badge, 'candidate')

    def test_missing_raw_unknown_category_is_rejected(self):
        del self.raw['summary']['propagation_unknown']
        with self.assertRaisesRegex(ValueError, 'must remain explicit'):
            verify.verify_unknowns(self.raw, self.badge, 'candidate')

    def test_zeroing_raw_unknown_category_is_rejected(self):
        self.raw['summary']['propagation_unknown'] = 0
        with self.assertRaisesRegex(ValueError, 'must remain explicit'):
            verify.verify_unknowns(self.raw, self.badge, 'candidate')

    def test_boolean_unknown_count_is_rejected(self):
        self.badge['counts']['unknowns'] = True
        with self.assertRaisesRegex(ValueError, 'native unknown count'):
            verify.verify_unknowns(self.raw, self.badge, 'candidate')

    def test_zero_unknown_category_still_must_be_present(self):
        del self.raw['summary']['static_unknown']
        with self.assertRaisesRegex(ValueError, 'must remain explicit'):
            verify.verify_unknowns(self.raw, self.badge, 'candidate')

    def test_base_deleted_unknown_remains_in_raw_breakdown(self):
        self.raw['findings'][0]['source_currentness'] = 'base_deleted'
        self.badge['counts']['unknowns'] = 0
        result = verify.verify_unknowns(self.raw, self.badge, 'candidate')
        self.assertEqual(result['propagation_unknown'], 1)

    def test_stable_unknown_is_not_removed_by_candidate_currentness(self):
        self.raw['findings'][0]['source_currentness'] = 'base_deleted'
        self.assertEqual(verify.verify_unknowns(self.raw, self.badge, 'stable')
                         ['propagation_unknown'], 1)

    def test_promotion_decision_is_rejected_even_with_zero_gaps(self):
        self.contract['decision'] = 'promote'
        with self.assertRaisesRegex(ValueError, 'unsupported promotion decision'):
            verify.verify_comparison_contract(self.contract)

    def test_missing_decision_is_rejected(self):
        del self.contract['decision']
        with self.assertRaisesRegex(ValueError, 'unsupported promotion decision'):
            verify.verify_comparison_contract(self.contract)

    def test_required_authority_is_rejected(self):
        self.contract['authority'] = 'required'
        with self.assertRaisesRegex(ValueError, 'required authority'):
            verify.verify_comparison_contract(self.contract)

    def test_unsupported_qualification_flags_are_rejected(self):
        for flag in ('qualified', 'promotion_approved', 'required_gate_passed'):
            with self.subTest(flag=flag):
                contract = copy.deepcopy(self.contract)
                contract[flag] = True
                with self.assertRaisesRegex(ValueError, 'unsupported qualification claim'):
                    verify.verify_comparison_contract(contract)

    def test_false_flags_do_not_claim_qualification(self):
        self.contract.update(qualified=False, promotion_approved=False,
                             required_gate_passed=False)
        verify.verify_comparison_contract(self.contract)


if __name__ == '__main__':
    unittest.main()
