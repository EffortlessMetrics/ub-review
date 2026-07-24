#[test]
fn adoption_docs_match_setup_ci_current_surface() {
    let docs = [
        (
            "SPEC-0007",
            include_str!("../docs/specs/UB-REVIEW-SPEC-0007-audit-ci.md"),
        ),
        (
            "SPEC-0008",
            include_str!("../docs/specs/UB-REVIEW-SPEC-0008-setup-ci.md"),
        ),
        (
            "CI_AUDIT_WIZARD",
            include_str!("../docs/CI_AUDIT_WIZARD.md"),
        ),
        (
            "ADR-0002",
            include_str!("../docs/adr/0002-single-gate-and-ci-audit-wizard.md"),
        ),
        ("ROADMAP", include_str!("../docs/ROADMAP.md")),
        (
            "SPEC-0001",
            include_str!("../docs/specs/UB-REVIEW-SPEC-0001-release-surface.md"),
        ),
        ("README", include_str!("../README.md")),
    ];
    for (name, text) in &docs {
        for stale in [
            "spec 0008, unimplemented",
            "the (future) `setup-ci` migration PR generator",
            "Honest answer today: no.",
            "Until it ships",
            "the PR you write yourself",
            "`setup-ci` ships only after",
            "three new files",
            "none of this exists",
            "Contract intent (nothing here is implemented)",
            "must not present setup-ci as available until the slices below land",
        ] {
            assert!(
                !text.contains(stale),
                "{name} leaked stale setup-ci adoption claim `{stale}`"
            );
        }
    }

    let spec_0007 = docs[0].1;
    assert!(spec_0007.contains("`setup-ci` migration PR generator"));
    assert!(spec_0007.contains("`--open-pr` opens the new-files-only migration PR"));

    let spec_0008 = docs[1].1;
    assert!(spec_0008.contains("Honest answer today: yes, within the v0 boundary."));
    assert!(spec_0008.contains("`setup-ci --open-pr` opens one new-files-only migration PR"));

    let wizard = docs[2].1;
    assert!(wizard.contains("`setup-ci --print-pr`"));
    assert!(wizard.contains("new-files-only `setup-ci --open-pr`"));
    assert!(wizard.contains("Never mutates branch protection itself."));

    let readme = docs[6].1;
    assert!(readme.contains("four new files"));

    for (name, text) in &docs {
        if text.contains("--apply-branch-protection") {
            let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
            assert!(
                normalized.contains("not implemented in the current CLI"),
                "{name} mentions branch-protection mutation without current CLI boundary"
            );
            assert!(
                normalized.contains("not part of the adoption path"),
                "{name} mentions branch-protection mutation without adoption-path boundary"
            );
        }
    }
}

#[test]
fn handoff_docs_cover_current_product_gate_surfaces() {
    let handoff = include_str!("../docs/REPO_OPERATING_HANDOFF.md");
    let porting = include_str!("../docs/PORTING_BASELINE.md");

    for required in [
        "review/fill-ledger.json",
        "selected and skipped optional proof",
        "review/proof_receipts.json#<receipt-id>",
        "review/resource_leases.json#<lease-id>",
        "sensors/ripr/exposure-gaps.json",
        "setup-ci --print-pr",
        "setup-ci --open-pr",
        "new-files-only migration PR",
        "never mutates branch protection",
    ] {
        assert!(
            handoff.contains(required),
            "handoff must keep current product gate surface `{required}` visible"
        );
    }

    for required in [
        "Receipt routes must carry exact source anchors",
        "review/proof_receipts.json#<receipt-id>",
        "review/resource_leases.json#<lease-id>",
    ] {
        assert!(
            porting.contains(required),
            "porting baseline must keep receipt-route source anchor `{required}` visible"
        );
    }
}

#[test]
fn artifact_contract_docs_match_ci_audit_verifier_coverage() {
    let spec_0004 = include_str!("../docs/specs/UB-REVIEW-SPEC-0004-artifact-contract.md");
    let verifier = include_str!("../scripts/verify-bun-review-artifacts.py");

    assert!(
        verifier.contains("def require_ci_audit_core_artifacts"),
        "ci-audit core receipt verifier disappeared"
    );
    assert!(
        spec_0004.contains("require_ci_audit_core_artifacts"),
        "SPEC-0004 must name the executable ci-audit verifier"
    );
    assert!(
        spec_0004.contains("ci-audit/audit-report.md"),
        "SPEC-0004 must keep the human audit report separate from JSON receipts"
    );
    assert!(
        verifier.contains("def require_ci_audit_report"),
        "ci-audit report verifier disappeared"
    );
    assert!(
        spec_0004.contains("require_ci_audit_report"),
        "SPEC-0004 must name the executable ci-audit report verifier"
    );
    assert!(
        verifier.contains("def require_setup_ci_terminal_receipts"),
        "setup-ci terminal receipt verifier disappeared"
    );
    assert!(
        spec_0004.contains("require_setup_ci_terminal_receipts"),
        "SPEC-0004 must name the executable setup-ci terminal receipt verifier"
    );
    assert!(
        spec_0004.contains("ci-audit/setup-pr-result.json XOR setup-pr-error.json"),
        "SPEC-0004 must document setup-ci result/error as an XOR terminal receipt"
    );
    for stale in [
        "ci-audit/*                              audit-ci output; contract pending",
        "give ci-audit/* its own contract spec before anyone builds on it",
        "`ci-audit/*` has a contract yet",
        "ci-audit/* pending spec 0007",
    ] {
        assert!(
            !spec_0004.contains(stale),
            "SPEC-0004 leaked stale ci-audit contract claim `{stale}`"
        );
    }
}

#[test]
fn artifact_contract_docs_pin_receipt_route_source_anchors() {
    let spec_0004 = include_str!("../docs/specs/UB-REVIEW-SPEC-0004-artifact-contract.md");
    let verifier = include_str!("../scripts/verify-bun-review-artifacts.py");

    assert!(
        verifier.contains("def receipt_route_source_artifacts"),
        "receipt route source-anchor verifier disappeared"
    );
    assert!(
        verifier.contains("receipt route missing exact source anchors"),
        "receipt route self-test must fail old artifact-only route sources"
    );
    for required in [
        "review/proof_receipts.json#<receipt-id>",
        "review/resource_leases.json#<lease-id>",
        "route entries carry exact proof receipt and matching lease anchors",
    ] {
        assert!(
            spec_0004.contains(required),
            "SPEC-0004 must document receipt route source anchor contract `{required}`"
        );
    }
}
