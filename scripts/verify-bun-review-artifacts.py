#!/usr/bin/env python3
"""Verify that a UB review packet satisfies the Bun v0 artifact contract."""

from __future__ import annotations

import argparse
import contextlib
import hashlib
import io
import json
import pathlib
import re
import sys
import tempfile
from typing import Any, Callable


SENSORS = ["tokmd", "cargo-allow", "ripr", "unsafe-review", "ast-grep", "actionlint"]
RUN_MODE_VALUES = {"review-byok", "intelligent-ci"}
RUN_PASS_VALUES = {
    "opened",
    "reopened",
    "ready_for_review",
    "synchronize",
    "pull_request_other",
    "manual",
}
SKIPPED_REVIEW_PAYLOAD_STATUSES = {
    "skipped_empty_smoke",
    "skipped_artifact_only_body",
    "skipped_pass_policy",
}
BOX_FROM_ALLOCATION_FALSE_PREMISE_DEDUPE_KEY = "rust-box-from-allocation-failure"
SIBLING_COMPLETENESS_OVERCLAIM_DEDUPE_KEY = "sibling-path-completeness-overclaim"
APPROVAL_LINES = {
    "lgtm",
    "looks good",
    "clean",
    "solid",
    "no issues found",
    "no actionable findings",
    "no actionable",
}
MAX_PR_REVIEW_BODY_BYTES = 6_000
MAX_PR_REVIEW_BODY_BULLETS = 12
ARTIFACT_NAME_MAX_CHARS = 96
ARTIFACT_NAME_HASH_CHARS = 16
SECRET_VALUE_NAMES = [
    "FACTORY_API_KEY",
    "github_token",
    "GITHUB_TOKEN",
    "MINIMAX_API_KEY",
    "OPENCODE",
    "OPENCODE_API_KEY",
    "UB_REVIEW_GITHUB_TOKEN",
    "UB_REVIEW_MINIMAX_API_KEY",
    "UB_REVIEW_OPENCODE_API_KEY",
]
SECRET_VALUE_PREFIXES = (
    "ghp_",
    "gho_",
    "ghu_",
    "ghs_",
    "ghr_",
    "github_pat_",
    "sk-",
    "sk_",
)
SECRET_HEADER_PATTERNS = [
    (
        "Authorization header",
        re.compile(
            r"(?im)\bAuthorization\s*:\s*(?:Bearer\s+)?"
            r"(?!redacted\b|\[redacted\]|\*\*\*)[A-Za-z0-9][A-Za-z0-9._~+/=-]{7,}"
        ),
    ),
    (
        "Bearer token",
        re.compile(
            r"(?im)\bBearer\s+"
            r"(?!redacted\b|\[redacted\]|\*\*\*)[A-Za-z0-9][A-Za-z0-9._~+/=-]{7,}"
        ),
    ),
    (
        "API key header",
        re.compile(
            r"(?im)\bX-API-Key\s*:\s*"
            r"(?!redacted\b|\[redacted\]|\*\*\*)[A-Za-z0-9][A-Za-z0-9._~+/=-]{7,}"
        ),
    ),
]
SECRET_ASSIGNMENT_PATTERN = re.compile(
    r"(?im)\b("
    + "|".join(re.escape(name) for name in SECRET_VALUE_NAMES)
    + r")\b\s*[:=]\s*([\"']?)([^\s,;}\])]+)"
)
SAFE_SECRET_VALUE_WORDS = {
    "",
    "false",
    "masked",
    "missing",
    "none",
    "null",
    "present",
    "redacted",
    "true",
    "unset",
}


def fail(message: str) -> None:
    print(f"verify-bun-review-artifacts: {message}", file=sys.stderr)
    raise SystemExit(1)


def require_run_pass(value: Any, label: str) -> str:
    if value not in RUN_PASS_VALUES:
        fail(
            f"{label} expected one of {sorted(RUN_PASS_VALUES)!r}, got {value!r}"
        )
    return value


def require_run_mode(value: Any, label: str) -> str:
    if value not in RUN_MODE_VALUES:
        fail(
            f"{label} expected one of {sorted(RUN_MODE_VALUES)!r}, got {value!r}"
        )
    return value


def expect_self_test_failure(
    label: str, expected_message: str, callback: Callable[[], None]
) -> None:
    stderr = io.StringIO()
    try:
        with contextlib.redirect_stderr(stderr):
            callback()
    except SystemExit as error:
        if error.code != 1:
            fail(f"self-test {label} exited with unexpected code {error.code!r}")
        message = stderr.getvalue()
        if expected_message not in message:
            fail(
                f"self-test {label} failed for the wrong reason: "
                f"expected {expected_message!r}, got {message!r}"
            )
        return
    fail(f"self-test {label} unexpectedly passed")


def read_text(path: pathlib.Path) -> str:
    try:
        return path.read_text(encoding="utf-8")
    except FileNotFoundError:
        fail(f"missing {path}")
    except UnicodeDecodeError as error:
        fail(f"invalid UTF-8 in {path}: {error}")


def looks_like_secret_assignment_value(value: str) -> bool:
    value = value.strip().strip("\"'")
    if value.lower() in SAFE_SECRET_VALUE_WORDS:
        return False
    if value.startswith(("$", "${{", "%", "<", "[", "`")):
        return False
    lowered = value.lower()
    if lowered.startswith(("\\u003c", "\\x3c")) or lowered.lstrip("\\").startswith(
        ("u003c", "x3c")
    ):
        return False
    if lowered.startswith(SECRET_VALUE_PREFIXES):
        return True
    compact = re.sub(r"[^A-Za-z0-9]", "", value)
    return (
        len(compact) >= 16
        and len(set(compact.lower())) >= 5
        and any(character.isalpha() for character in compact)
        and any(character.isdigit() for character in compact)
    )


def secret_leak_marker(text: str) -> str | None:
    for label, pattern in SECRET_HEADER_PATTERNS:
        if pattern.search(text):
            return label
    for match in SECRET_ASSIGNMENT_PATTERN.finditer(text):
        if looks_like_secret_assignment_value(match.group(3)):
            return match.group(1)
    return None


def load_json(path: pathlib.Path) -> Any:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError:
        fail(f"missing {path}")
    except json.JSONDecodeError as error:
        fail(f"invalid JSON in {path}: {error}")


def require_file(path: pathlib.Path) -> pathlib.Path:
    if not path.is_file():
        fail(f"missing file {path}")
    return path


def no_standalone_approval_line(text: str, path: pathlib.Path) -> None:
    for line_number, line in enumerate(text.splitlines(), start=1):
        normalized = (
            line.strip()
            .removeprefix("- ")
            .removeprefix("* ")
            .strip()
            .lower()
        )
        if normalized in APPROVAL_LINES:
            fail(f"standalone approval line in {path}:{line_number}: {line!r}")


def has_reviewer_value_heading(body: str) -> bool:
    return any(
        heading in body
        for heading in [
            "## Decision",
            "## Findings",
            "## Confirmed findings",
            "## Verification questions",
            "## Test proof",
            "## Proof results",
            "## Refuted",
            "## Parked follow-ups",
            "## Evidence gaps",
            "## Missing evidence",
        ]
    )


def require_pr_review_body_policy(
    body: str, path: pathlib.Path, waive_suppressible: bool = False
) -> None:
    """Enforce the PR body contract.

    `waive_suppressible` mirrors the Rust suppressor waiver: when the
    effective `[review_body].summary_only_body` is a posting posture
    (`post_substantive`/`post_all`), the run deliberately posted a body the
    suppressor would have withheld, so the suppressible classes (conciseness,
    boilerplate phrases and noise classifiers, refuted-only notes) are not
    re-litigated here. The structural walls (status-section headings and
    execution-summary labels, which rendered PR bodies never carry) stay in
    force.
    """
    lowered = body.lower()
    if not waive_suppressible:
        body_bytes = len(body.strip().encode("utf-8"))
        if body_bytes > MAX_PR_REVIEW_BODY_BYTES:
            fail(
                f"{path} is not concise enough: "
                f"{body_bytes} bytes over max {MAX_PR_REVIEW_BODY_BYTES}"
            )
        bullet_count = pr_body_bullet_count(body)
        if bullet_count > MAX_PR_REVIEW_BODY_BULLETS:
            fail(
                f"{path} is not concise enough: "
                f"{bullet_count} bullets over max {MAX_PR_REVIEW_BODY_BULLETS}"
            )
        if is_workflow_trust_posture_review_noise(lowered):
            fail(f"{path} contains artifact-only workflow trust posture prose")
        if is_refuted_only_pr_body(lowered):
            fail(f"{path} contains refuted-only artifact note")
        for phrase in [
            "no blocking finding after",
            "no blocking ub finding",
            "no actionable findings",
            "a human should still inspect",
            "human should still review",
            "residual risk remains for human review",
            "bounded review",
            "cached prior observation",
            "refuter demoted inline candidate",
            "gate proof is pending",
            "cannot perform from cached context",
            "commit-existence/ancestry proof",
            "upstream commit-existence",
            "general bot output",
            "pr-body contract hardening",
            "actionlint ran ok",
            "pre-existing, not a diff target",
            "identical to prior pin",
            "no widened attack surface",
            "standing-repo concern",
            "lane transcript",
            "lane roster",
            "model lane roster",
            "raw observations",
            "provider preflight",
            "provider status",
            "sensor status",
            "shared context hash",
            "cache manifest",
            "runtime profile",
            "review payload status",
            "terminal state",
            "github-review-skip",
        ]:
            if phrase in lowered:
                fail(f"{path} contains artifact-only boilerplate: {phrase!r}")
        if "## Residual risk" in body:
            fail(
                f"{path} contains artifact-only status section: '## Residual risk'"
            )
        if is_unsupported_sibling_completeness_overclaim(body):
            fail(
                f"unsupported sibling completeness claim leaked into {path}; "
                "report scan coverage as a verification question instead"
            )
    for heading in [
        "## Model lanes",
        "## Model lane status",
        "## Lane status",
        "## Lane roster",
        "## Provider preflights",
        "## Provider status",
        "## Model provider status",
        "## Sensors",
        "## Sensor status",
        "## Sensor receipts",
    ]:
        if heading in body:
            fail(f"{path} contains artifact-only status section: {heading!r}")
    for label in [
        "- Shared context:",
        "- Profile:",
        "- Base:",
        "- Head:",
        "- Changed files:",
        "- Inline comments:",
        "## Review efficiency",
        "Runtime:",
        "Terminal state:",
        "Review payload:",
        "Follow-up results:",
    ]:
        if label in body:
            fail(f"{path} contains execution summary boilerplate: {label!r}")


SUMMARY_ONLY_BODY_VALUES = {"suppress", "post_substantive", "post_all"}


def effective_summary_only_body(root: pathlib.Path) -> str:
    """`[review_body].summary_only_body` from the run's effective-config.json.

    Missing, unreadable, or unknown values fall back to the conservative
    `suppress` posture (the Rust loader receipts unknown values as policy
    errors and runs with the same default).
    """
    path = root / "effective-config.json"
    if not path.is_file():
        return "suppress"
    try:
        config = json.loads(path.read_text(encoding="utf-8"))
    except (json.JSONDecodeError, UnicodeDecodeError, OSError):
        return "suppress"
    if not isinstance(config, dict):
        return "suppress"
    review_body = config.get("review_body")
    if not isinstance(review_body, dict):
        return "suppress"
    value = review_body.get("summary_only_body")
    return value if value in SUMMARY_ONLY_BODY_VALUES else "suppress"


def pr_body_bullet_count(body: str) -> int:
    return sum(
        1
        for line in body.splitlines()
        if line.lstrip().startswith("- ") or line.lstrip().startswith("* ")
    )


def is_workflow_trust_posture_review_noise(text: str) -> bool:
    return is_unchanged_workflow_trust_posture_noise(text) or (
        (
            "does not eliminate upstream trust" in text
            or "trust in upstream tag" in text
        )
        and (
            "secrets.minimax" in text
            or "github.token" in text
            or "malicious or compromised" in text
        )
    ) or is_no_finding_workflow_pin_summary_noise(
        text
    ) or is_stale_external_bot_objection_noise(
        text
    ) or is_workflow_tool_status_artifact_gap_noise(
        text
    ) or is_workflow_paths_ignore_no_posture_noise(
        text
    ) or is_actionlint_semantic_skip_proof_noise(
        text
    ) or is_current_pin_consistency_followup_noise(
        text
    ) or is_workflow_pin_lockstep_no_value_summary_noise(
        text
    )


def is_refuted_only_pr_body(text: str) -> bool:
    return "## refuted" in text and not any(
        heading in text
        for heading in [
            "## decision",
            "## confirmed findings",
            "## verification questions",
            "## test proof",
            "## proof results",
            "## parked follow-ups",
            "## evidence gaps",
            "## missing evidence",
        ]
    )


def is_unchanged_workflow_trust_posture_noise(text: str) -> bool:
    mentions_workflow_trust = (
        "upstream trust" in text
        or "upstream sha trust" in text
        or "trust in upstream" in text
        or "malicious or compromised" in text
        or "would exfiltrate" in text
        or "reproducibly verified" in text
        or "repo-level policy item" in text
        or "secrets.minimax" in text
        or "github.token" in text
        or "workflow-level permissions" in text
        or "permissions block" in text
        or "exposure surface" in text
    )
    says_unchanged_or_out_of_scope = (
        "not introduced by this" in text
        or "pre-existing" in text
        or "not a diff target" in text
        or "identical to prior" in text
        or "identical in posture" in text
        or "no widened attack surface" in text
        or "zero new secret" in text
        or "zero new" in text
        or "not a diff finding" in text
        or "not a diff-introduced" in text
        or "no permission/trigger/pinning posture change" in text
        or "no permission" in text
        or "no permissions" in text
        or "unchanged" in text
        or "standing-repo concern" in text
        or "standing repo concern" in text
    )
    return mentions_workflow_trust and says_unchanged_or_out_of_scope


def is_no_finding_workflow_pin_summary_noise(text: str) -> bool:
    mentions_pin = (
        "pinning" in text
        or "sha-pinning" in text
        or "sha bump" in text
        or "sha swap" in text
        or "mechanical sha" in text
        or "action uses" in text
        or "uses: ref" in text
        or "cache key" in text
        or "per-action full-sha" in text
        or "40-hex" in text
        or "all-zero" in text
    )
    says_no_defect = (
        "no pinning defect introduced" in text
        or "pinning posture preserved" in text
        or "sha-pinning control remains effective" in text
        or "sha-pinning control is effective" in text
        or "old pin fully absent" in text
        or "pin is 40-hex non-zero" in text
        or "matches expected sha-1 shape" in text
        or "pin shape valid 40-hex" in text
    )
    says_not_current_diff = (
        "not a diff finding" in text
        or "not a diff-introduced" in text
        or "not introduced by this" in text
        or "identical in posture" in text
        or "byte-identical" in text
        or "repo-level policy item" in text
        or "unchanged from prior pin" in text
        or "net new secret/permission surface" in text
        or "net new secret surface" in text
        or "no new permission" in text
        or "no permission, token-scope" in text
        or "no blocker introduced" in text
    )
    return mentions_pin and (says_no_defect or says_not_current_diff)


def is_stale_external_bot_objection_noise(text: str) -> bool:
    mentions_bots = (
        "cursor[bot]" in text
        or "coderabbit" in text
        or "stale-bot" in text
    )
    says_stale_false_positive = ("stale" in text or "false positive" in text) and (
        "false positive" in text
        or "reopens nothing" in text
        or "not real findings" in text
        or "current diff" in text
        or "live diff" in text
    )
    contradicted_target_advice = (
        "different sha" in text
        or "targeting a different sha" in text
        or "0 references to" in text
        or "scripted check showing 0 references" in text
        or "not match to gate target" in text
    ) and ("used in the diff" in text or "current diff" in text)
    mentions_pin_ref_mismatch = (
        "claim target" in text
        or "pin mismatch" in text
        or "target sha" in text
        or "current head sha" in text
        or "pr title" in text
        or "pr body" in text
    )
    return (
        mentions_bots
        and mentions_pin_ref_mismatch
        and (says_stale_false_positive or contradicted_target_advice)
    )


def is_workflow_tool_status_artifact_gap_noise(text: str) -> bool:
    actionlint_ok = "actionlint" in text and (
        "receipt is 'ok'" in text
        or "actionlint=ok" in text
        or "actionlint receipt ok" in text
        or "sensor table" in text
    )
    not_inlined = (
        "no per-line output" in text
        or "not inlined" in text
        or "central proof broker artifact" in text
        or "sensors/actionlint" in text
    )
    yaml_pin = (
        "4-line workflow pin" in text
        or "4-line sha-swap" in text
        or "yaml-only" in text
        or "pin/uses ref consistent" in text
    )
    skipped_heavy = (
        "build/test skipped" in text
        or "--allow-heavy" in text
        or "no fresh pr-build smoke" in text
        or "heavy smoke adds limited value" in text
    )
    disabled_workflow_tools = (
        "zizmor" in text
        or "gitleaks" in text
        or "osv-scanner" in text
        or "cargo-audit" in text
        or "cargo-deny" in text
        or "shellcheck" in text
        or "semgrep" in text
        or "coverage" in text
    ) and (
        "disabled by config" in text or "trigger-mismatched" in text
    ) and ("workflow file" in text or "security/pinning tool" in text)
    local_actionlint_gap = (
        "actionlint" in text
        and "not installed locally" in text
        and ("local pre-push run" in text or "ub-review gate" in text)
    )
    non_workflow_lint_skip = (
        "actionlint" in text
        and ("zizmor" in text or "shellcheck" in text)
        and ("skipped" in text or "disabled" in text)
        and (
            "no .github diff" in text
            or "no github actions yaml" in text
            or "no workflow" in text
            or "consumer workflow" in text
            or "invokes this script" in text
            or "no yaml in diff" in text
        )
    )
    return (
        (actionlint_ok and (not_inlined or yaml_pin))
        or (skipped_heavy and yaml_pin)
        or disabled_workflow_tools
        or local_actionlint_gap
        or non_workflow_lint_skip
        or (
            ("parked follow-up" in text or "not a blocker" in text)
            and actionlint_ok
            and yaml_pin
        )
    )


def is_gap_noise_meta_review_noise(text: str) -> bool:
    mentions_gap_noise = (
        "gap-noise" in text
        or "is_workflow_tool_status_artifact_gap_noise" in text
    )
    mentions_meta_surface = (
        "observation text" in text
        or "observation string" in text
        or "string literal" in text
        or "trust_language_softening" in text
        or "trust-language softening" in text
        or "substring-based matching" in text
    )
    mentions_softening = (
        "softened" in text
        or "softening" in text
        or "not trust-affecting" in text
        or "absence of proof" in text
    )
    return mentions_gap_noise and mentions_meta_surface and mentions_softening


def is_workflow_paths_ignore_no_posture_noise(text: str) -> bool:
    mentions_paths_ignore = "paths-ignore" in text or "path-ignore" in text
    mentions_workflow_posture = (
        "token scopes" in text
        or "permissions block" in text
        or "permission expansion" in text
        or "job-level security context" in text
        or "trigger activation" in text
        or "pull_request_target" in text
        or "checkout" in text
        or "semantic skip behavior" in text
        or "focused smoke proof" in text
        or "workflow_run" in text
        or "droid noise" in text
    )
    says_no_posture_change = (
        "only filters trigger activation" in text
        or "does not alter" in text
        or "no new trigger" in text
        or "no new persistence vector" in text
        or "not modified in this pr" in text
        or "diff only mutates a paths-ignore" in text
        or "not proven by sensors" in text
        or "trust rests on actionlint parse" in text
        or ("future pr" in text and "re-trigger droid" in text)
        or "ub gate is the authoritative review" in text
        or ("future rename" in text and "re-enable" in text)
    )
    return mentions_paths_ignore and mentions_workflow_posture and says_no_posture_change


def is_actionlint_semantic_skip_proof_noise(text: str) -> bool:
    mentions_actionlint_skip = "actionlint" in text and (
        "semantic skip behavior" in text
        or ("skip behavior" in text and "droid" in text)
    )
    says_proof_is_not_decisive = (
        "no semantic proof" in text
        or "trust rests on actionlint parse" in text
        or "unproven beyond actionlint parse" in text
        or "not proven by sensors" in text
        or "no focused smoke proof" in text
    )
    scoped_to_auxiliary_lane = (
        "droid lane" in text
        or "droid" in text
        or "auxiliary/non-blocking" in text
        or "ub gate is authoritative" in text
        or "ub gate is the authoritative" in text
    )
    return (
        mentions_actionlint_skip
        and says_proof_is_not_decisive
        and scoped_to_auxiliary_lane
    )


def is_current_pin_consistency_followup_noise(text: str) -> bool:
    mentions_cache_pin = (
        "cache key" in text
        or "restore-keys" in text
        or "cache restore" in text
    ) and ("action sha" in text or "repin" in text or "uses:" in text)
    says_future_or_parked = (
        "future repin" in text
        or "future pin" in text
        or "partial repin" in text
        or "parked for follow-up" in text
        or "parked for lint-rule" in text
        or "lint-rule follow-up" in text
        or "follow-up lint rule" in text
        or "follow-up lint" in text
        or "lint rule or script" in text
    )
    says_currently_consistent = (
        "current state is consistent" in text
        or "current state consistent" in text
        or "current pr state is consistent" in text
        or "not actionable in this pr" in text
    )
    return mentions_cache_pin and says_future_or_parked and says_currently_consistent


def is_workflow_pin_lockstep_no_value_summary_noise(text: str) -> bool:
    workflow_scope = (
        "workflow" in text
        or "ub-review" in text
        or "actionlint" in text
        or "paths-ignore" in text
        or "droid" in text
    )
    mentions_lockstep_pin = (
        "pin lockstep" in text
        or "lockstep sha pin" in text
        or "pin bump is lockstep" in text
        or "pin/uses ref consistent" in text
        or (
            "cache key/restore-keys" in text
            and (
                "prefix match" in text
                or "prefix is coupled" in text
                or "updated in lockstep" in text
                or "must be updated in lockstep" in text
            )
        )
        or (
            "cache key" in text
            and "restore-keys" in text
            and "uses:" in text
            and "lockstep" in text
        )
    )
    says_no_current_issue = (
        "old pin absent" in text
        or "current state is consistent" in text
        or "current state consistent" in text
        or "current pr state is consistent" in text
        or "no blocker" in text
        or "not a blocker" in text
        or "no other third-party actions changed" in text
        or "no syntactic regression" in text
        or "no source, no permissions, no token, no checkout changes" in text
        or (
            "no new" in text
            and (
                "permission" in text
                or "token" in text
                or "third-party action" in text
                or "checkout" in text
            )
        )
    )
    return workflow_scope and mentions_lockstep_pin and says_no_current_issue


def is_unsupported_sibling_completeness_overclaim(text: str) -> bool:
    lowered = text.lower()
    if not ("sibling" in lowered or "analogous" in lowered):
        return False
    if has_broad_sibling_coverage_claim(lowered):
        return False
    negative_scan = contains_any(
        lowered,
        [
            "no sibling",
            "no siblings",
            "no analogous",
            "none widen",
            "none of the sibling",
            "not found",
            "no match",
            "no matches",
            "nothing else",
        ],
    )
    completeness_claim = contains_any(
        lowered,
        [
            "correctly scoped",
            "need not be broadened",
            "does not need to be broadened",
            "no need to broaden",
            "complete fix",
            "fix is complete",
            "scope is complete",
            "no siblings exist",
            "no sibling paths exist",
            "no sibling concern",
            "no sibling gap",
        ],
    )
    if has_honest_limited_sibling_scope(lowered) and not completeness_claim:
        return False
    return (negative_scan and completeness_claim) or contains_any(
        lowered,
        [
            "no siblings exist",
            "no sibling paths exist",
            "no analogous sibling",
        ],
    )


def has_broad_sibling_coverage_claim(text: str) -> bool:
    return contains_any(
        text,
        [
            "across all",
            "all ffi entry",
            "all entry point",
            "all public route",
            "all sibling",
            "every sibling",
            "every ffi",
            "exhaustive",
            "meta-class",
        ],
    )


def has_honest_limited_sibling_scope(text: str) -> bool:
    return contains_any(
        text,
        [
            "checked scope",
            "scan scope",
            "scanned scope",
            "limited to",
            "did not scan",
            "not scanned",
            "unscanned",
            "only checked",
            "only scanned",
        ],
    )


def contains_any(value: str, needles: list[str]) -> bool:
    return any(needle in value for needle in needles)


FOLLOW_UP_RESULT_STATUSES = {
    "ok",
    "degraded",
    "skipped",
    "skipped_budget",
    "missing_key",
    "preflight_failed",
    "failed",
    "timed_out",
    "rate_limited",
    "auth_failed",
    "invalid_json",
    "bad_envelope",
}
FOLLOW_UP_OUTPUT_COUNT_FIELDS = [
    "observations",
    "candidate_findings",
    "summary_only_findings",
    "failed_objections",
    "proof_requests",
]
MODEL_CALL_ATTEMPTED_STATUSES = {
    "ok",
    "failed",
    "degraded",
    "invalid_json",
    "timed_out",
    "rate_limited",
    "auth_failed",
    "bad_envelope",
}


def require_common_tree(root: pathlib.Path) -> None:
    for path in [
        "input/changed-files.txt",
        "input/diff.patch",
        "input/diff-context.json",
        "events.ndjson",
        "work_queue.json",
        "work_events.ndjson",
        "plan.json",
        "resolved-profile.json",
        "resolved-plan.json",
        "resolved-tools.json",
        "tool-status.json",
        "tool-gate-outcomes.json",
        "running-summary.md",
        "review/shared_context.md",
        "review/shared_context_cache_block.md",
        "review/shared_context_hash.txt",
        "review/cache_manifest.json",
        "review/cache_events.ndjson",
        "review/pr_thread_context.json",
        "review/terminal_state.json",
        "review/resolved-tools.json",
        "review/tool-status.json",
        "review/tool-gate-outcomes.json",
        "review/provider-preflight-status.json",
        "review/model_stages.json",
        "review/metrics.json",
        "review/scheduler.json",
        "review/review.json",
        "review/review.md",
        "review/observations.json",
        "review/unique_observations.json",
        "review/merged_observations.json",
        "review/dropped_observations.json",
        "review/orchestrator_plan.json",
        "review/final_orchestrator_plan.json",
        "review/follow_up_results.json",
        "review/follow_up_outputs.json",
        "review/follow_up_evidence.json",
        "review/resolved_candidates.json",
        "review/final_compiler_input.json",
        "review/witnesses.json",
        "review/witness_registry.json",
        "review/proof_requests.json",
        "review/proof_planner_input.json",
        "review/proof_planner_output.json",
        "review/proof_request_groups.json",
        "review/proof_receipts.json",
        "review/receipt_routes.json",
        "review/proof_plan.md",
        "review/resource_leases.json",
        "review/resource_plan.md",
        "follow_up_questions.ndjson",
        "follow_up_results.ndjson",
        "follow_up_outputs.ndjson",
        "model_stages.ndjson",
        "resolved_candidates.ndjson",
        "witnesses.ndjson",
        "proof_requests.ndjson",
        "proof_tasks.ndjson",
        "proof_receipts.ndjson",
        "receipt_routes.ndjson",
        "tool_gate_outcomes.ndjson",
        "resource_leases.ndjson",
    ]:
        require_file(root / path)
    require_cache_artifacts(root)
    require_events(root)
    if not (root / "review/github-review.json").exists() and not (
        root / "review/github-review-skip.json"
    ).exists():
        fail("neither github-review.json nor github-review-skip.json exists")
    if (root / "review/github-review.json").exists() and (
        root / "review/github-review-skip.json"
    ).exists():
        fail("both github-review.json and github-review-skip.json exist")

    for sensor in SENSORS:
        require_file(root / "sensors" / sensor / "ub-review-sensor-status.json")

    resolved_plan = load_json(root / "resolved-plan.json")
    if not isinstance(resolved_plan, dict):
        fail("resolved-plan.json is not an object")
    selectors = resolved_plan.get("selectors")
    if not isinstance(selectors, dict):
        fail("resolved-plan.json selectors is not an object")
    lanes = selectors.get("effective_model_lanes")
    if not isinstance(lanes, list):
        fail("resolved-plan.json selectors.effective_model_lanes is not an array")
    expected_lane_packets = set()
    for lane in lanes:
        if not isinstance(lane, str) or not lane:
            fail(f"effective model lane is invalid: {lane!r}")
        expected_lane_packets.add(f"{sanitize_artifact_name(lane)}.md")
        lane_path = require_file(root / "lanes" / f"{sanitize_artifact_name(lane)}.md")
        lane_text = read_text(lane_path)
        if f"[{lane}]" not in lane_text:
            fail(f"lane packet {lane_path} does not include [{lane}] prefix")
        no_standalone_approval_line(lane_text, lane_path)
    actual_lane_packets = {
        path.name for path in (root / "lanes").glob("*.md") if path.is_file()
    }
    if actual_lane_packets != expected_lane_packets:
        fail(
            "lane packet files do not match effective_model_lanes: "
            f"expected {sorted(expected_lane_packets)!r}, got {sorted(actual_lane_packets)!r}"
        )


def require_events(root: pathlib.Path) -> None:
    events_path = root / "events.ndjson"
    kinds: list[str] = []
    for index, line in enumerate(read_text(events_path).splitlines(), start=1):
        if not line.strip():
            continue
        try:
            event = json.loads(line)
        except json.JSONDecodeError as error:
            fail(f"invalid events.ndjson line {index}: {error}")
        if not isinstance(event, dict):
            fail(f"events.ndjson line {index} is not an object")
        if not isinstance(event.get("ts"), str) or not event["ts"]:
            fail(f"events.ndjson line {index} missing string ts")
        if not isinstance(event.get("kind"), str) or not event["kind"]:
            fail(f"events.ndjson line {index} missing string kind")
        if "payload" not in event:
            fail(f"events.ndjson line {index} missing payload")
        kinds.append(event["kind"])
    for required in [
        "run_started",
        "evidence_stream_started",
        "evidence_stream_completed",
        "model_stream_started",
        "model_stream_completed",
        "proof_stream_started",
        "proof_stream_completed",
        "run_finished",
    ]:
        if required not in kinds:
            fail(f"events.ndjson missing {required}")


def require_cache_artifacts(root: pathlib.Path) -> None:
    shared_context = read_text(root / "review/shared_context.md")
    cache_block = read_text(root / "review/shared_context_cache_block.md")
    if cache_block != shared_context:
        fail("shared_context_cache_block.md does not match shared_context.md")
    shared_context_hash = read_text(root / "review/shared_context_hash.txt").strip()
    if not shared_context_hash:
        fail("shared_context_hash.txt is empty")
    manifest = load_json(root / "review/cache_manifest.json")
    if manifest.get("schema") != "ub-review.cache_manifest.v1":
        fail("cache_manifest.json has wrong schema")
    if manifest.get("shared_context_hash") != shared_context_hash:
        fail("cache_manifest shared_context_hash does not match shared_context_hash.txt")
    if manifest.get("cache_block_path") != "review/shared_context_cache_block.md":
        fail("cache_manifest cache_block_path is invalid")
    if manifest.get("hash_path") != "review/shared_context_hash.txt":
        fail("cache_manifest hash_path is invalid")
    if manifest.get("events_path") != "review/cache_events.ndjson":
        fail("cache_manifest events_path is invalid")
    if manifest.get("explicit_cache_provider") != "minimax":
        fail("cache_manifest explicit_cache_provider is invalid")
    if manifest.get("explicit_cache_endpoint") != "anthropic-messages":
        fail("cache_manifest explicit_cache_endpoint is invalid")
    lanes = manifest.get("lanes")
    if not isinstance(lanes, list):
        fail("cache_manifest lanes is not an array")
    for lane in lanes:
        if not isinstance(lane, dict):
            fail(f"cache_manifest lane is not an object: {lane!r}")
        for field in [
            "lane",
            "provider",
            "model",
            "endpoint_kind",
            "cache_mode",
            "shared_context_hash",
        ]:
            if not isinstance(lane.get(field), str) or not lane[field]:
                fail(f"cache_manifest lane missing string field {field}: {lane!r}")
        if lane["shared_context_hash"] != shared_context_hash:
            fail(f"cache_manifest lane hash does not match shared context hash: {lane!r}")
    cache_events = []
    for index, line in enumerate(
        read_text(root / "review/cache_events.ndjson").splitlines(), start=1
    ):
        if not line.strip():
            continue
        try:
            event = json.loads(line)
        except json.JSONDecodeError as error:
            fail(f"invalid cache_events.ndjson line {index}: {error}")
        if not isinstance(event, dict):
            fail(f"cache event line {index} is not an object")
        if event.get("schema") != "ub-review.cache_event.v1":
            fail(f"cache event has wrong schema: {event!r}")
        if event.get("shared_context_hash") != shared_context_hash:
            fail(f"cache event shared_context_hash mismatch: {event!r}")
        if not isinstance(event.get("kind"), str) or not event["kind"]:
            fail(f"cache event missing kind: {event!r}")
        cache_events.append(event)
    if not cache_events:
        fail("cache_events.ndjson is empty")
    if not any(event.get("kind") == "shared_context_prepared" for event in cache_events):
        fail("cache_events.ndjson missing shared_context_prepared")


def require_summary(root: pathlib.Path) -> None:
    summary_path = root / "running-summary.md"
    summary = read_text(summary_path)
    for heading in [
        "## Missing evidence",
        "## Provider preflights",
        "## Model lane status",
        "## Lane packets",
        "## Review efficiency",
    ]:
        if heading not in summary:
            fail(f"{summary_path} missing {heading}")
    if "Follow-up results:" not in summary:
        fail(f"{summary_path} missing follow-up result efficiency line")
    no_standalone_approval_line(summary, summary_path)


def require_profile_artifacts(
    root: pathlib.Path, expected_review_profile: str, expected_repo_kind: str
) -> tuple[dict, dict]:
    resolved_profile = load_json(root / "resolved-profile.json")
    resolved_plan = load_json(root / "resolved-plan.json")
    if not isinstance(resolved_profile, dict):
        fail("resolved-profile.json is not an object")
    if not isinstance(resolved_plan, dict):
        fail("resolved-plan.json is not an object")
    if resolved_profile.get("schema") != "ub-review.resolved_profile.v1":
        fail("resolved-profile.json has wrong schema")
    if resolved_plan.get("schema") != "ub-review.resolved_plan.v1":
        fail("resolved-plan.json has wrong schema")
    if resolved_profile.get("selected_review_profile") != expected_review_profile:
        fail(
            "resolved-profile.json selected_review_profile expected "
            f"{expected_review_profile}, got {resolved_profile.get('selected_review_profile')!r}"
        )
    review_profile = resolved_profile.get("review_profile")
    if not isinstance(review_profile, dict):
        fail("resolved-profile.json review_profile is not an object")
    if review_profile.get("name") != expected_review_profile:
        fail(
            "resolved-profile.json review_profile.name expected "
            f"{expected_review_profile}, got {review_profile.get('name')!r}"
        )
    if review_profile.get("repo_kind") != expected_repo_kind:
        fail(
            "resolved-profile.json review_profile.repo_kind expected "
            f"{expected_repo_kind}, got {review_profile.get('repo_kind')!r}"
        )
    runtime_profile = resolved_profile.get("selected_runtime_profile")
    if not isinstance(runtime_profile, str) or not runtime_profile:
        fail("resolved-profile.json selected_runtime_profile is invalid")
    if resolved_plan.get("review_profile") != expected_review_profile:
        fail(
            "resolved-plan.json review_profile expected "
            f"{expected_review_profile}, got {resolved_plan.get('review_profile')!r}"
        )
    if resolved_plan.get("runtime_profile") != runtime_profile:
        fail("resolved-plan.json runtime_profile does not match resolved-profile.json")
    require_gate_config("resolved-profile.json", resolved_profile.get("gate"))
    require_gate_config("resolved-plan.json", resolved_plan.get("gate"))
    if resolved_profile.get("gate") != resolved_plan.get("gate"):
        fail("resolved-plan.json gate does not match resolved-profile.json")
    return resolved_profile, resolved_plan


def require_gate_config(path: str, gate: object) -> None:
    if not isinstance(gate, dict):
        fail(f"{path} gate is not an object")
    for field in ["required_check", "synchronize_mode"]:
        if not isinstance(gate.get(field), str) or not gate[field]:
            fail(f"{path} gate.{field} is invalid: {gate!r}")
    for field in ["target_minutes", "hard_timeout_minutes"]:
        value = gate.get(field)
        if not isinstance(value, int) or isinstance(value, bool) or value <= 0:
            fail(f"{path} gate.{field} is invalid: {gate!r}")
    if gate["hard_timeout_minutes"] < gate["target_minutes"]:
        fail(f"{path} gate hard_timeout_minutes is below target_minutes: {gate!r}")
    post_review_on = gate.get("post_review_on")
    if not isinstance(post_review_on, list) or not all(
        isinstance(item, str) and item for item in post_review_on
    ):
        fail(f"{path} gate.post_review_on is not a string array: {gate!r}")


def require_review(
    root: pathlib.Path,
    max_inline_comments: int | None,
    expected_review_profile: str,
) -> dict:
    review = load_json(root / "review/review.json")
    review_body = read_text(root / "review/review.md")
    shared_context = read_text(root / "review/shared_context.md")

    shared_context_id = review.get("shared_context_id")
    if not isinstance(shared_context_id, str) or not re.fullmatch(
        r"[0-9a-f]{64}", shared_context_id
    ):
        fail("review.json shared_context_id is not a 64-character hex digest")
    require_run_mode(review.get("mode"), "review.json mode")
    if review.get("review_profile") != expected_review_profile:
        fail(
            f"review.json review_profile expected {expected_review_profile}, "
            f"got {review.get('review_profile')!r}"
        )
    if review.get("posting") not in {"review", "artifact-only"}:
        fail(f"review.json posting has unexpected value {review.get('posting')!r}")
    review_run_pass = require_run_pass(review.get("run_pass"), "review.json run_pass")
    if not isinstance(review.get("model_lanes"), list):
        fail("review.json model_lanes is not an array")
    if "## UB Ledger Context" not in shared_context:
        fail("shared_context.md missing UB ledger context section")
    if "## PR Thread Context" not in shared_context:
        fail("shared_context.md missing PR thread context section")
    if "## Initial Work Queue" not in shared_context:
        fail("shared_context.md missing initial work queue section")
    if "pending work is unfinished, not missing evidence" not in shared_context:
        fail("shared_context.md missing pending-work packet rule")
    pr_thread_context = load_json(root / "review/pr_thread_context.json")
    if not isinstance(pr_thread_context, dict):
        fail("pr_thread_context.json is not an object")
    if pr_thread_context.get("schema") != "ub-review.pr_thread_context.v1":
        fail(
            "pr_thread_context.json schema expected ub-review.pr_thread_context.v1, "
            f"got {pr_thread_context.get('schema')!r}"
        )
    if pr_thread_context.get("status") not in {"seeded", "absent", "unavailable"}:
        fail(
            "pr_thread_context.json status expected seeded/absent/unavailable, "
            f"got {pr_thread_context.get('status')!r}"
        )
    if review.get("pr_thread_context") != pr_thread_context:
        fail("review.json pr_thread_context does not match pr_thread_context.json")
    terminal_state = load_json(root / "review/terminal_state.json")
    if not isinstance(terminal_state, dict):
        fail("terminal_state.json is not an object")
    if terminal_state.get("schema") != "ub-review.terminal_state.v1":
        fail(
            "terminal_state.json schema expected ub-review.terminal_state.v1, "
            f"got {terminal_state.get('schema')!r}"
        )
    if terminal_state.get("status") not in {
        "needs-reviewer-attention",
        "sufficient",
        "artifact-only",
        "failed-to-review",
    }:
        fail(
            "terminal_state.json status expected a known terminal state, "
            f"got {terminal_state.get('status')!r}"
        )
    if review.get("terminal_state") != terminal_state:
        fail("review.json terminal_state does not match terminal_state.json")

    for heading in [
        "## Decision",
        "## Confirmed findings",
        "## Summary-only findings",
        "## Failed objections",
        "## Residual risk",
        "## Parked follow-ups",
        "## Missing or failed evidence",
    ]:
        if heading not in review_body:
            fail(f"review.md missing {heading}")
    no_standalone_approval_line(review_body, root / "review/review.md")

    github_review_path = root / "review/github-review.json"
    github_skip_path = root / "review/github-review-skip.json"
    if github_review_path.exists():
        github_review = load_json(github_review_path)
        if github_review.get("event") != "COMMENT":
            fail(f"github-review.json event expected COMMENT, got {github_review.get('event')!r}")
        body = github_review.get("body")
        if not isinstance(body, str) or not has_reviewer_value_heading(body):
            fail("github-review.json body is missing reviewer-value content")
        no_standalone_approval_line(body, github_review_path)
        require_pr_review_body_policy(
            body,
            github_review_path,
            waive_suppressible=effective_summary_only_body(root) != "suppress",
        )
        comments = github_review.get("comments")
        if not isinstance(comments, list):
            fail("github-review.json comments is not an array")
        if max_inline_comments is None:
            max_inline_comments = int(review.get("max_inline_comments", 8))
        if len(comments) > max_inline_comments:
            fail(
                f"github-review.json has {len(comments)} comments, "
                f"over max {max_inline_comments}"
            )
        for index, comment in enumerate(comments):
            require_github_comment(comment, index)
    else:
        skip = load_json(github_skip_path)
        if skip.get("status") != "skipped":
            fail(f"github-review-skip.json status expected skipped, got {skip.get('status')!r}")
        if skip.get("review_payload_status") not in SKIPPED_REVIEW_PAYLOAD_STATUSES:
            fail(
                "github-review-skip.json review_payload_status expected skipped status"
            )
        if skip.get("terminal_state") != review.get("terminal_state", {}).get("status"):
            fail("github-review-skip.json terminal_state does not match review.json")
        if skip.get("run_pass") != review_run_pass:
            fail("github-review-skip.json run_pass does not match review.json")
        require_skipped_payload_contract(skip, root, github_skip_path)

    return review


def require_skipped_payload_contract(receipt: dict, root: pathlib.Path, path: pathlib.Path) -> None:
    declared = receipt.get("github_review_json")
    if declared is None:
        return
    if not isinstance(declared, str) or not declared:
        fail(f"{path} github_review_json must be null or a non-empty string")
    if "\\" in declared:
        fail(f"{path} github_review_json must use artifact-relative POSIX paths")
    declared_path = pathlib.PurePosixPath(declared)
    if declared_path.is_absolute() or ".." in declared_path.parts:
        fail(f"{path} github_review_json is not artifact-relative: {declared!r}")
    if not (root / declared_path).exists():
        fail(f"{path} github_review_json points at missing artifact: {declared}")


def require_github_comment(comment: dict, index: int) -> None:
    path = comment.get("path")
    if not isinstance(path, str) or not path or path.startswith(("/", "\\")) or ".." in pathlib.PurePosixPath(path).parts:
        fail(f"github review comment {index} path is not repo-relative: {path!r}")
    if comment.get("side") != "RIGHT":
        fail(f"github review comment {index} side expected RIGHT, got {comment.get('side')!r}")
    line = comment.get("line")
    if not isinstance(line, int) or line <= 0:
        fail(f"github review comment {index} line is invalid: {line!r}")
    body = comment.get("body")
    if not isinstance(body, str) or not re.match(r"^\[[a-z0-9-]+\]", body):
        fail(f"github review comment {index} body lacks lane prefix")
    if len(body) > 1_200:
        fail(f"github review comment {index} body exceeds 1200 characters")
    no_standalone_approval_line(body, pathlib.Path("review/github-review.json"))
    require_pr_review_body_policy(
        body, pathlib.Path(f"review/github-review.json comments[{index}].body")
    )


def require_metrics(root: pathlib.Path, review: dict) -> dict:
    metrics = load_json(root / "review/metrics.json")
    if metrics.get("schema_version") != 1:
        fail(f"metrics schema_version expected 1, got {metrics.get('schema_version')!r}")
    if metrics.get("shared_context_id") != review.get("shared_context_id"):
        fail("metrics shared_context_id does not match review.json")
    require_run_loop_metrics(metrics)
    require_scheduler_artifact(root, metrics)
    if metrics.get("mode") != review.get("mode"):
        fail("metrics mode does not match review.json")
    review_run_pass = require_run_pass(review.get("run_pass"), "review.json run_pass")
    if metrics.get("run_pass") != review_run_pass:
        fail("metrics run_pass does not match review.json")
    if metrics.get("review_profile") != review.get("review_profile"):
        fail("metrics review_profile does not match review.json")
    if metrics.get("provider_policy") != review.get("provider_policy"):
        fail("metrics provider_policy does not match review.json")
    if metrics.get("inline_comments") != len(review.get("inline_comments", [])):
        fail("metrics inline_comments does not match review.json")
    if metrics.get("summary_only_findings") != len(review.get("summary_only_findings", [])):
        fail("metrics summary_only_findings does not match review.json")
    resolved_plan = load_json(root / "resolved-plan.json")
    if resolved_plan.get("run_pass") != review_run_pass:
        fail("resolved-plan.json run_pass does not match review.json")
    selectors = resolved_plan.get("selectors", {})
    if selectors.get("run_pass") != review_run_pass:
        fail("resolved-plan.json selectors.run_pass does not match review.json")
    effective_lanes = selectors.get("effective_model_lanes", [])
    if not isinstance(effective_lanes, list):
        fail("resolved-plan.json selectors.effective_model_lanes is not an array")
    if metrics.get("lane_packets") != len(effective_lanes):
        fail("metrics lane_packets does not match effective_model_lanes")
    require_candidate_artifacts(root, review)
    require_orchestrator_plan(root)
    if metrics.get("terminal_state") != review.get("terminal_state", {}).get("status"):
        fail("metrics terminal_state does not match review.json terminal_state")
    if not isinstance(metrics.get("observations"), int):
        fail("metrics observations is not an integer")
    if not isinstance(metrics.get("proof_requests"), int):
        fail("metrics proof_requests is not an integer")
    if not isinstance(metrics.get("proof_receipts"), int):
        fail("metrics proof_receipts is not an integer")
    if not isinstance(metrics.get("resource_leases"), int):
        fail("metrics resource_leases is not an integer")
    observations = load_json(root / "review/observations.json")
    if not isinstance(observations, list):
        fail("review/observations.json is not an array")
    if metrics.get("observations") != len(observations):
        fail("metrics observations does not match review/observations.json")
    require_observation_summary_artifacts(root, observations)
    proof_requests = load_json(root / "review/proof_requests.json")
    if not isinstance(proof_requests, list):
        fail("review/proof_requests.json is not an array")
    if metrics.get("proof_requests") != len(proof_requests):
        fail("metrics proof_requests does not match review/proof_requests.json")
    if review.get("proof_requests", []) != proof_requests:
        fail("review proof_requests does not match review/proof_requests.json")
    require_proof_request_ndjson(root, proof_requests)
    require_proof_planner_artifacts(root)
    proof_receipts = load_json(root / "review/proof_receipts.json")
    if not isinstance(proof_receipts, list):
        fail("review/proof_receipts.json is not an array")
    if metrics.get("proof_receipts") != len(proof_receipts):
        fail("metrics proof_receipts does not match review/proof_receipts.json")
    if review.get("proof_receipts", []) != proof_receipts:
        fail("review proof_receipts does not match review/proof_receipts.json")
    require_proof_receipt_ndjson(root, proof_receipts)
    resource_leases = load_json(root / "review/resource_leases.json")
    if not isinstance(resource_leases, list):
        fail("review/resource_leases.json is not an array")
    if metrics.get("resource_leases") != len(resource_leases):
        fail("metrics resource_leases does not match review/resource_leases.json")
    if review.get("resource_leases", []) != resource_leases:
        fail("review resource_leases does not match review/resource_leases.json")
    require_resource_lease_artifacts(root, proof_receipts, resource_leases)
    require_receipt_route_artifacts(root, proof_receipts, resource_leases)
    orchestrator_plan = load_json(root / "review/orchestrator_plan.json")
    final_orchestrator_plan = load_json(root / "review/final_orchestrator_plan.json")
    final_follow_up_tasks = final_orchestrator_plan.get("follow_up_tasks")
    if not isinstance(final_follow_up_tasks, list):
        fail("review/final_orchestrator_plan.json follow_up_tasks is not an array")
    final_follow_up_metric = require_non_negative_int(
        metrics, "metrics.final_follow_up_tasks", "final_follow_up_tasks"
    )
    if final_follow_up_metric != len(final_follow_up_tasks):
        fail("metrics final_follow_up_tasks does not match final_orchestrator_plan")
    terminal_final_follow_up_metric = require_non_negative_int(
        review.get("terminal_state", {}),
        "terminal_state.final_follow_up_tasks",
        "final_follow_up_tasks",
    )
    if terminal_final_follow_up_metric != len(final_follow_up_tasks):
        fail("terminal_state final_follow_up_tasks does not match final_orchestrator_plan")
    if terminal_final_follow_up_metric != final_follow_up_metric:
        fail("terminal_state final_follow_up_tasks does not match metrics")
    follow_up_results = require_follow_up_results(root, orchestrator_plan["follow_up_tasks"])
    require_model_stage_artifacts(root, review, follow_up_results)
    follow_up_outputs = require_follow_up_outputs(root, follow_up_results)
    follow_up_evidence = require_follow_up_evidence(root, follow_up_outputs)
    require_resolved_candidate_artifacts(root, follow_up_results, follow_up_outputs)
    require_final_compiler_input(root, review, follow_up_evidence)
    require_witness_artifacts(root, follow_up_evidence)
    require_follow_up_result_metrics(metrics, follow_up_results)
    require_observation_files(root, observations, orchestrator_plan["follow_up_tasks"])
    if (root / "review/github-review-skip.json").exists():
        if metrics.get("review_payload_status") not in SKIPPED_REVIEW_PAYLOAD_STATUSES:
            fail("metrics review_payload_status does not match github-review-skip.json")
        if metrics.get("github_review_body_bytes") != 0:
            fail("metrics github_review_body_bytes must be 0 for skipped review payloads")
        if metrics.get("github_review_comments") != 0:
            fail("metrics github_review_comments must be 0 for skipped review payloads")
    models = metrics.get("models")
    if not isinstance(models, dict):
        fail("metrics.models is missing")
    require_model_cache_metrics(models)
    return metrics


def require_model_cache_metrics(models: dict) -> None:
    for field in [
        "prompt_cache_creation_input_tokens",
        "prompt_cache_read_input_tokens",
        "prompt_cache_lane_hits",
        "prompt_cache_lane_misses",
        "prompt_cache_lane_unknown",
    ]:
        require_non_negative_int(models, f"metrics.models.{field}", field)


def require_run_loop_metrics(metrics: dict) -> None:
    run = metrics.get("run")
    if not isinstance(run, dict):
        fail("metrics.run is missing")
    if run.get("concurrency_model") != "profiled-stream-scheduler-v0":
        fail(f"metrics.run.concurrency_model is invalid: {run.get('concurrency_model')!r}")
    if run.get("scheduler_profile") != "default-three-stream-v0":
        fail(f"metrics.run.scheduler_profile is invalid: {run.get('scheduler_profile')!r}")
    if run.get("local_proof_wall_excludes_model_wait") is not True:
        fail("metrics.run.local_proof_wall_excludes_model_wait must be true")
    for field in [
        "elapsed_wall_ms",
        "coordination_wall_ms",
        "investigation_wall_ms",
        "proof_wall_ms",
        "evidence_wall_ms",
        "model_wall_ms",
        "local_proof_wall_ms",
        "compiler_wall_ms",
        "model_call_duration_ms_sum",
        "proof_command_duration_ms_sum",
        "investigation_proof_overlap_ms",
        "model_proof_overlap_ms",
        "proof_overlap_ms",
    ]:
        require_non_negative_int(run, f"metrics.run.{field}", field)
    streams = run.get("streams")
    if not isinstance(streams, dict):
        fail("metrics.run.streams is missing")
    for stream_name in ["coordination", "investigation", "proof"]:
        require_timing(streams, f"metrics.run.streams.{stream_name}", stream_name)
    if run.get("coordination_wall_ms") != streams["coordination"].get("wall_ms"):
        fail("metrics.run.coordination_wall_ms does not match metrics.run.streams.coordination.wall_ms")
    if run.get("investigation_wall_ms") != streams["investigation"].get("wall_ms"):
        fail("metrics.run.investigation_wall_ms does not match metrics.run.streams.investigation.wall_ms")
    if run.get("proof_wall_ms") != streams["proof"].get("wall_ms"):
        fail("metrics.run.proof_wall_ms does not match metrics.run.streams.proof.wall_ms")
    scheduler_roles = run.get("scheduler_roles")
    if not isinstance(scheduler_roles, dict):
        fail("metrics.run.scheduler_roles is missing")
    for role_name in ["evidence", "model", "proof"]:
        require_timing(scheduler_roles, f"metrics.run.scheduler_roles.{role_name}", role_name)
    if run.get("evidence_wall_ms") != scheduler_roles["evidence"].get("wall_ms"):
        fail("metrics.run.evidence_wall_ms does not match metrics.run.scheduler_roles.evidence.wall_ms")
    if run.get("model_wall_ms") != scheduler_roles["model"].get("wall_ms"):
        fail("metrics.run.model_wall_ms does not match metrics.run.scheduler_roles.model.wall_ms")
    if run.get("local_proof_wall_ms") != scheduler_roles["proof"].get("wall_ms"):
        fail("metrics.run.local_proof_wall_ms does not match metrics.run.scheduler_roles.proof.wall_ms")
    loops = run.get("loops")
    if not isinstance(loops, dict):
        fail("metrics.run.loops is missing")
    for loop_name in ["evidence", "model", "proof", "compiler"]:
        require_timing(loops, f"metrics.run.loops.{loop_name}", loop_name)
    phases = run.get("phases")
    if not isinstance(phases, list):
        fail("metrics.run.phases is missing")
    if not phases:
        fail("metrics.run.phases is empty")
    for index, phase in enumerate(phases):
        require_scheduler_phase(phase, f"metrics.run.phases[{index}]")


def require_scheduler_artifact(root: pathlib.Path, metrics: dict) -> None:
    scheduler = load_json(root / "review/scheduler.json")
    if scheduler.get("schema") != "ub-review.scheduler.v1":
        fail("review/scheduler.json has wrong schema")
    run = metrics.get("run", {})
    for field in [
        "concurrency_model",
        "scheduler_profile",
        "local_proof_wall_excludes_model_wait",
        "elapsed_wall_ms",
    ]:
        if scheduler.get(field) != run.get(field):
            fail(f"review/scheduler.json {field} does not match metrics.run")
    if scheduler.get("streams") != run.get("streams"):
        fail("review/scheduler.json streams do not match metrics.run.streams")
    if scheduler.get("scheduler_roles") != run.get("scheduler_roles"):
        fail("review/scheduler.json scheduler_roles do not match metrics.run.scheduler_roles")
    if scheduler.get("loops") != run.get("loops"):
        fail("review/scheduler.json loops do not match metrics.run.loops")
    overlaps = scheduler.get("overlaps")
    if not isinstance(overlaps, dict):
        fail("review/scheduler.json overlaps is missing")
    for field in [
        "investigation_proof_overlap_ms",
        "model_proof_overlap_ms",
        "proof_overlap_ms",
    ]:
        if overlaps.get(field) != run.get(field):
            fail(f"review/scheduler.json overlaps.{field} does not match metrics.run")
    if scheduler.get("phases") != run.get("phases"):
        fail("review/scheduler.json phases do not match metrics.run.phases")
    stages = {
        (phase.get("loop_id"), phase.get("stage"))
        for phase in scheduler.get("phases", [])
        if isinstance(phase, dict)
    }
    for expected in [
        ("evidence", "sensors-and-packet"),
        ("proof", "initial-diff-broker"),
        ("compiler", "final"),
    ]:
        if expected not in stages:
            fail(f"review/scheduler.json missing scheduler phase {expected}")


def require_scheduler_phase(phase: dict, label: str) -> None:
    if not isinstance(phase, dict):
        fail(f"{label} is not an object")
    for field in ["loop_id", "stream_id", "stage", "status"]:
        if not isinstance(phase.get(field), str) or not phase[field]:
            fail(f"{label}.{field} is missing")
    for field in ["started_at_offset_ms", "finished_at_offset_ms", "duration_ms"]:
        require_non_negative_int(phase, f"{label}.{field}", field)
    if phase["finished_at_offset_ms"] < phase["started_at_offset_ms"]:
        fail(f"{label} finished before it started")
    span = phase["finished_at_offset_ms"] - phase["started_at_offset_ms"]
    if phase["duration_ms"] > span and phase["finished_at_offset_ms"] > phase["started_at_offset_ms"]:
        fail(f"{label} duration exceeds observed span")


def require_timing(container: dict, label: str, field: str) -> None:
    timing = container.get(field)
    if not isinstance(timing, dict):
        fail(f"{label} is missing")
    started = require_non_negative_int(timing, f"{label}.started_at_offset_ms", "started_at_offset_ms")
    finished = require_non_negative_int(timing, f"{label}.finished_at_offset_ms", "finished_at_offset_ms")
    wall = require_non_negative_int(timing, f"{label}.wall_ms", "wall_ms")
    if finished < started:
        fail(f"{label} finished before it started")
    if wall > finished - started and finished > started:
        fail(f"{label} wall exceeds observed span")


def require_non_negative_int(container: dict, label: str, field: str) -> int:
    value = container.get(field)
    if isinstance(value, bool) or not isinstance(value, int) or value < 0:
        fail(f"{label} is not a non-negative integer: {value!r}")
    return value


def require_candidate_artifacts(root: pathlib.Path, review: dict) -> None:
    candidates = load_json(root / "review/candidates.json")
    if not isinstance(candidates, list):
        fail("review/candidates.json is not an array")
    expected = expected_candidate_records(review)
    if candidates != expected:
        fail("review/candidates.json does not match review candidate surfaces")

    candidate_dir = root / "candidates"
    expected_files = {
        f"{sanitize_artifact_name(candidate['id'])}.json": candidate
        for candidate in candidates
    }
    if not candidate_dir.exists():
        if expected_files:
            fail("missing candidates directory")
    elif not candidate_dir.is_dir():
        fail("candidates path is not a directory")
    else:
        actual_files = []
        for path in candidate_dir.iterdir():
            if not path.is_file():
                fail(f"unexpected candidates entry: {path.name}")
            actual_files.append(path.name)
        actual_files.sort()
        if actual_files != sorted(expected_files):
            fail("candidates directory entries do not match review/candidates.json")
        for name, expected_candidate in expected_files.items():
            parsed = load_json(candidate_dir / name)
            if parsed != expected_candidate:
                fail(f"candidates/{name} does not match review/candidates.json")

    ndjson_path = root / "candidates.ndjson"
    lines = [line for line in read_text(ndjson_path).splitlines() if line.strip()]
    if len(lines) != len(candidates):
        fail("candidates.ndjson line count does not match review/candidates.json")
    for index, line in enumerate(lines):
        try:
            parsed = json.loads(line)
        except json.JSONDecodeError as error:
            fail(f"invalid candidates.ndjson line {index + 1}: {error}")
        if parsed != candidates[index]:
            fail(f"candidates.ndjson line {index + 1} does not match JSON artifact")
    for candidate in candidates:
        require_candidate_schema(candidate)


def require_orchestrator_plan(root: pathlib.Path) -> None:
    candidates = load_json(root / "review/candidates.json")
    observations = load_json(root / "review/unique_observations.json")
    proof_receipts = load_json(root / "review/proof_receipts.json")
    resource_leases = load_json(root / "review/resource_leases.json")
    receipt_routes = load_json(root / "review/receipt_routes.json")
    routes = receipt_routes.get("routes") if isinstance(receipt_routes, dict) else None
    if not isinstance(routes, list):
        fail("review/receipt_routes.json routes is not an array")
    follow_up_receipt_ids = {
        route.get("receipt_id")
        for route in routes
        if isinstance(route, dict) and route.get("phase") == "follow-up-receipt"
    }
    pre_follow_up_receipts = [
        receipt
        for receipt in proof_receipts
        if receipt.get("id") not in follow_up_receipt_ids
    ]
    pre_follow_up_receipt_ids = {
        receipt["id"] for receipt in pre_follow_up_receipts if isinstance(receipt, dict)
    }
    pre_follow_up_leases = [
        lease
        for lease in resource_leases
        if lease.get("consumer") in pre_follow_up_receipt_ids
    ]
    plan = load_json(root / "review/orchestrator_plan.json")
    expected = expected_orchestrator_plan(
        candidates, observations, pre_follow_up_receipts, pre_follow_up_leases
    )
    if plan != expected:
        fail("review/orchestrator_plan.json does not match pre-follow-up evidence routing")
    final_plan = load_json(root / "review/final_orchestrator_plan.json")
    final_expected = expected_final_orchestrator_plan(
        candidates, observations, proof_receipts, resource_leases
    )
    if final_plan != final_expected:
        fail(
            "review/final_orchestrator_plan.json does not match final candidate/observation evidence routing"
        )

    lines = [line for line in read_text(root / "follow_up_questions.ndjson").splitlines() if line.strip()]
    tasks = plan["follow_up_tasks"]
    if len(lines) != len(tasks):
        fail("follow_up_questions.ndjson line count does not match orchestrator plan")
    for index, line in enumerate(lines):
        try:
            parsed = json.loads(line)
        except json.JSONDecodeError as error:
            fail(f"invalid follow_up_questions.ndjson line {index + 1}: {error}")
        if parsed != tasks[index]:
            fail(f"follow_up_questions.ndjson line {index + 1} does not match orchestrator plan")
    require_orchestrator_plan_schema(plan)
    require_orchestrator_plan_schema(final_plan)


def expected_orchestrator_plan(
    candidates: list[dict],
    observations: list[dict],
    proof_receipts: list[dict],
    resource_leases: list[dict],
) -> dict:
    grouped: dict[tuple[str, str], list[dict]] = {}
    for candidate in candidates:
        key = (candidate["disposition"], candidate_evidence_need(candidate))
        grouped.setdefault(key, []).append(candidate)

    evidence_groups = []
    follow_up_tasks = []
    for disposition, evidence_need in sorted(grouped):
        group_candidates = grouped[(disposition, evidence_need)]
        candidate_ids = [candidate["id"] for candidate in group_candidates]
        lanes = sorted({candidate["lane"] for candidate in group_candidates})
        routed_evidence = routed_evidence_for_group(
            evidence_need, lanes, proof_receipts, resource_leases
        )
        group_id = "evidence-group-" + hashlib.sha256(
            f"{disposition}\n{evidence_need}".encode("utf-8")
        ).hexdigest()[:12]
        group = {
            "schema": "ub-review.orchestrator_evidence_group.v1",
            "id": group_id,
            "evidence_need": evidence_need,
            "disposition": disposition,
            "candidate_ids": candidate_ids,
            "lanes": lanes,
            "routed_evidence": routed_evidence,
            "duplicate_count": max(0, len(candidate_ids) - 1),
            "reason": f"grouped candidate disposition `{disposition}` under evidence need `{evidence_need}`",
        }
        task = follow_up_task_for_group(
            group_id, disposition, evidence_need, candidate_ids, routed_evidence
        )
        if task is not None:
            follow_up_tasks.append(task)
        evidence_groups.append(group)

    observation_groups = []
    for observation in observations:
        evidence_need = observation_evidence_need(observation)
        routed_evidence = routed_evidence_for_group(
            evidence_need, observation["lanes"], proof_receipts, resource_leases
        )
        group_id = f"orchestrator-{observation['id']}"
        group = {
            "schema": "ub-review.orchestrator_observation_group.v1",
            "id": group_id,
            "observation_group_id": observation["id"],
            "dedupe_key": observation["dedupe_key"],
            "evidence_need": evidence_need,
            "claim": observation["claim"],
            "kind": observation["kind"],
            "status": observation["status"],
            "lanes": observation["lanes"],
            "sources": observation["sources"],
            "observation_ids": observation["observation_ids"],
            "duplicate_count": observation["duplicate_count"],
            "routed_evidence": routed_evidence,
            "reason": f"routed unique observation group `{observation['id']}` under evidence need `{evidence_need}`",
        }
        task = follow_up_task_for_observation_group(observation, group, routed_evidence)
        if task is not None:
            follow_up_tasks.append(task)
        observation_groups.append(group)

    return {
        "schema": "ub-review.orchestrator_plan.v1",
        "candidates": len(candidates),
        "observations": len(observations),
        "evidence_groups": evidence_groups,
        "observation_groups": observation_groups,
        "follow_up_tasks": follow_up_tasks,
    }


def expected_final_orchestrator_plan(
    candidates: list[dict],
    observations: list[dict],
    proof_receipts: list[dict],
    resource_leases: list[dict],
) -> dict:
    plan = expected_orchestrator_plan(
        candidates, observations, proof_receipts, resource_leases
    )
    plan["follow_up_tasks"] = [
        task
        for task in plan["follow_up_tasks"]
        if not final_follow_up_task_resolved_by_tool_proof(task)
    ]
    return plan


def final_follow_up_task_resolved_by_tool_proof(task: dict) -> bool:
    return task["evidence_need"] == "proof-confirmation" and any(
        evidence["kind"] == "proof-receipt" and evidence["status"] == "tool-confirmed"
        for evidence in task["routed_evidence"]
    )


def candidate_evidence_need(candidate: dict) -> str:
    disposition = candidate["disposition"]
    if disposition == "inline":
        return "accepted-inline-review"
    if disposition == "parked-follow-up":
        return "parked-follow-up-confirmation"
    if disposition == "refuted":
        return "refutation-confirmation"
    if disposition == "dropped":
        return "dropped-candidate-audit"
    text = f"{candidate['claim']}\n{candidate['evidence']}".lower()
    if "proof" in text or "red" in text or "green" in text:
        return "proof-confirmation"
    if "route" in text or "sibling" in text:
        return "source-route-confirmation"
    if "test" in text or "oracle" in text:
        return "test-oracle-confirmation"
    return "summary-confirmation"


def observation_evidence_need(observation: dict) -> str:
    if is_refutation_confirmation_observation(observation):
        return "refutation-confirmation"
    if is_parked_observation(observation):
        return "parked-follow-up-confirmation"
    if observation["kind"] == "test-gap":
        return "test-oracle-confirmation"
    if observation["kind"] == "source-route-gap":
        return "source-route-confirmation"
    text = f"{observation['claim']}\n{chr(10).join(observation.get('evidence', []))}".lower()
    if "proof" in text or "red" in text or "green" in text or "base+tests" in text:
        return "proof-confirmation"
    if "route" in text or "sibling" in text:
        return "source-route-confirmation"
    if "test" in text or "oracle" in text:
        return "test-oracle-confirmation"
    if is_missing_evidence_observation(observation):
        return "evidence-gap-confirmation"
    if is_residual_risk_observation(observation):
        return "residual-risk-confirmation"
    return "observation-confirmation"


def is_refuted_observation(observation: dict) -> bool:
    return observation["status"] == "refuted" or observation["kind"] in {
        "false-premise",
        "resolved-check",
    }


def is_pr_body_refuted_observation(observation: dict) -> bool:
    if observation["kind"] == "resolved-check":
        return False
    return is_refuted_observation(observation) and not is_global_calibration_refutation(
        observation
    )


def is_refutation_confirmation_observation(observation: dict) -> bool:
    return is_refuted_observation(observation) and not is_global_calibration_refutation(
        observation
    )


def is_global_calibration_refutation(observation: dict) -> bool:
    return (
        observation["dedupe_key"] == BOX_FROM_ALLOCATION_FALSE_PREMISE_DEDUPE_KEY
        and observation.get("path") is None
        and "model-false-premise-guard" in observation["sources"]
    )


def is_pr_body_artifact_only_observation(observation: dict) -> bool:
    text = observation_text(observation)
    return (
        observation["status"] == "covered"
        or observation["kind"] == "resolved-check"
        or observation["dedupe_key"].startswith("lane-output-shape")
        or observation["dedupe_key"].startswith("lane-output-malformed-content")
        or (observation["kind"] == "bug" and "lane model summary" in text)
        or "inline guard rejected " in text
        or "severity_allowed=" in text
        or "confidence_allowed=" in text
        or (
            "no permissions" in text
            and ("no new auth surface" in text or "no new token scope" in text)
        )
        or ("no permissions block" in text and "no pull_request_target" in text)
        or ("supply-chain tightening" in text and "no new scope" in text)
        or (
            "out-of-hunk" in text
            and (
                "cursor" in text
                or "push-not-synchronize" in text
                or "pull_request" in text
            )
        )
        or (
            "full 40-hex" in text
            and ("prefix collision" in text or "short-prefix" in text)
        )
        or ("actionlint" in text and "sensor reports ok" in text)
        or ("actionlint" in text and "status=ok" in text)
        or is_unchanged_workflow_trust_posture_noise(text)
        or is_no_finding_workflow_pin_summary_noise(text)
        or is_stale_external_bot_objection_noise(text)
        or is_workflow_tool_status_artifact_gap_noise(text)
        or is_workflow_paths_ignore_no_posture_noise(text)
        or is_actionlint_semantic_skip_proof_noise(text)
        or is_non_workflow_verifier_scope_noise(text)
        or is_self_test_meta_review_noise(text)
        or is_current_pin_consistency_followup_noise(text)
        or is_workflow_pin_lockstep_no_value_summary_noise(text)
        or is_pr_body_meta_review_noise(text)
        or (
            observation["kind"] == "false-premise"
            and (
                "short-prefix" in text
                or ("cache key" in text and "full 40-hex" in text)
                or ("supply-chain" in text and "sha pin" in text)
                or "floating @v0.1" in text
                or "pinning to a sha" in text
                or "pinning to immutable commit sha" in text
                or ("scope change" in text and "supply-chain tightening" in text)
            )
        )
        or (is_missing_evidence_observation(observation) and is_tool_status_only_gap(text))
    )


def observation_text(observation: dict) -> str:
    return f"{observation['claim']} {' '.join(observation.get('evidence', []))}".lower()


def is_missing_evidence_observation(observation: dict) -> bool:
    return observation["kind"] == "missing-evidence"


def is_tool_status_only_gap(text: str) -> bool:
    return (
        ("sensor `" in text or " sensor " in text or "sensors:" in text)
        and (
            "missing" in text
            or "command not found" in text
            or "disabled" in text
        )
        and "base+tests" not in text
        and "red/green" not in text
        and "regression test" not in text
        and "changed-line coverage" not in text
    )


def is_pr_body_meta_review_noise(text: str) -> bool:
    return (
        "cached prior observation" in text
        or "refuter demoted inline candidate" in text
        or "gate proof is pending" in text
        or "cannot perform from cached context" in text
        or "commit-existence/ancestry proof" in text
        or "upstream commit-existence" in text
        or "general bot output" in text
        or (
            "the refutation claiming" in text
            and "still matches current evidence" in text
        )
        or is_gap_noise_meta_review_noise(text)
        or (
            "pr-body contract hardening" in text
            and "not verifiable from the repo diff" in text
        )
        or ("cache key/uses ref" in text and "40 hex" in text and "non-zero" in text)
        or ("sha were 39-hex" in text and "all-zero" in text)
        or is_checkout_persistence_no_change_noise(text)
        or "actionlint ran ok" in text
    )


def is_non_workflow_verifier_scope_noise(text: str) -> bool:
    verifier_script = (
        "scripts/verify-bun-review-artifacts.py" in text
        or "python verifier" in text
        or "python-only" in text
        or "python script" in text
    )
    workflow_not_changed = (
        "no .github/workflows" in text
        or "no github actions yaml" in text
        or "no workflow yaml" in text
        or "no workflow file" in text
        or "no workflow changes" in text
        or "no workflow files changed" in text
        or "no action versions" in text
        or "no action references" in text
        or "no actions, reusable workflows" in text
        or "no workflow trigger" in text
        or "diff does not modify any github actions yaml" in text
    )
    says_scope_only = (
        "actionlint" in text
        or "zizmor" in text
        or "pinning" in text
        or "permissions" in text
        or "token-scope" in text
        or "out of scope" in text
        or "not applicable" in text
        or "not a trust gap" in text
        or "no actionable finding" in text
        or "nothing to pin-review" in text
        or "surfaces are limited" in text
        or "validator script itself" in text
    )
    return verifier_script and workflow_not_changed and says_scope_only


def is_self_test_meta_review_noise(text: str) -> bool:
    mentions_self_test = (
        "self-test" in text
        or "run_self_tests" in text
        or "--self-test" in text
        or "tempfile.temporarydirectory" in text
    )
    says_meta = (
        "receipt not in seeded thread" in text
        or "pr body asserts" in text
        or "focused smoke proof pattern" in text
        or "suitable for python change verification" in text
        or "confirm new self-tests" in text
        or "if --self-test is not executed in ci" in text
    )
    return mentions_self_test and says_meta


def is_checkout_persistence_no_change_noise(text: str) -> bool:
    return (
        "checkout credential persistence" in text
        or "checkout config" in text
        or "persist-credentials" in text
    ) and (
        "did not change checkout" in text
        or "does not change checkout" in text
        or "no new persistence vector" in text
        or "read-only github_token" in text
    )


def is_residual_risk_observation(observation: dict) -> bool:
    return observation["kind"] == "residual-risk"


def is_parked_observation(observation: dict) -> bool:
    return observation["status"] == "parked" or observation["kind"] == "parked-follow-up"


def follow_up_task_for_group(
    group_id: str,
    disposition: str,
    evidence_need: str,
    candidate_ids: list[str],
    routed_evidence: list[dict],
) -> dict | None:
    if disposition in {"inline", "dropped"}:
        return None
    task_id = "follow-up-" + hashlib.sha256(
        f"{group_id}\n{evidence_need}".encode("utf-8")
    ).hexdigest()[:12]
    stage = follow_up_stage(disposition, evidence_need, routed_evidence)
    return {
        "schema": "ub-review.follow_up_question.v1",
        "id": task_id,
        "group_id": group_id,
        "stage": stage,
        "stage_reason": follow_up_stage_reason(stage),
        "evidence_need": evidence_need,
        "disposition": disposition,
        "candidate_ids": candidate_ids,
        "observation_group_ids": [],
        "routed_evidence": routed_evidence,
        "question": follow_up_question_text(disposition, evidence_need),
        "status": "planned",
        "reason": "deterministic orchestrator skeleton; no shell commands or posting side effects",
    }


def follow_up_task_for_observation_group(
    observation: dict, group: dict, routed_evidence: list[dict]
) -> dict | None:
    if is_pr_body_artifact_only_observation(observation) or observation["status"] in {
        "covered",
        "duplicate",
    }:
        return None
    task_id = "follow-up-" + hashlib.sha256(
        f"{group['id']}\n{group['evidence_need']}".encode("utf-8")
    ).hexdigest()[:12]
    stage = follow_up_stage("observation", group["evidence_need"], routed_evidence)
    return {
        "schema": "ub-review.follow_up_question.v1",
        "id": task_id,
        "group_id": group["id"],
        "stage": stage,
        "stage_reason": follow_up_stage_reason(stage),
        "evidence_need": group["evidence_need"],
        "disposition": "observation",
        "candidate_ids": [],
        "observation_group_ids": [observation["id"]],
        "routed_evidence": routed_evidence,
        "question": observation_follow_up_question_text(group["evidence_need"]),
        "status": "planned",
        "reason": "deterministic observation follow-up; no shell commands or posting side effects",
    }


def follow_up_stage(
    disposition: str, evidence_need: str, routed_evidence: list[dict]
) -> str:
    if (
        routed_evidence
        or disposition in {"refuted", "parked-follow-up"}
        or evidence_need in {"refutation-confirmation", "parked-follow-up-confirmation"}
    ):
        return "tertiary"
    return "secondary"


def follow_up_stage_reason(stage: str) -> str:
    if stage == "tertiary":
        return (
            "routed evidence or prior disposition is available; refine, refute, "
            "drop, or park instead of restating the concern"
        )
    return (
        "no routed proof receipt is available; ask for the smallest remaining "
        "evidence or proof request"
    )


def routed_evidence_for_group(
    evidence_need: str,
    lanes: list[str],
    proof_receipts: list[dict],
    resource_leases: list[dict],
) -> list[dict]:
    if evidence_need not in {"proof-confirmation", "test-oracle-confirmation"}:
        return []
    routed = []
    for receipt in proof_receipts:
        if not proof_receipt_routes_to_lanes(receipt, lanes):
            continue
        routed.append(proof_receipt_routed_evidence(receipt))
        for lease in resource_leases:
            if lease["consumer"] == receipt["id"]:
                routed.append(resource_lease_routed_evidence(lease))
    return routed


def proof_receipt_routes_to_lanes(receipt: dict, lanes: list[str]) -> bool:
    requested_by = receipt["requested_by"]
    return "proof-broker" in requested_by or any(lane in lanes for lane in requested_by)


def proof_receipt_routed_evidence(receipt: dict) -> dict:
    return {
        "schema": "ub-review.orchestrator_routed_evidence.v1",
        "id": receipt["id"],
        "kind": "proof-receipt",
        "artifact": "review/proof_receipts.json",
        "status": routed_status_for_proof_receipt(receipt),
        "result": receipt["result"],
        "reason": receipt["reason"],
    }


def resource_lease_routed_evidence(lease: dict) -> dict:
    return {
        "schema": "ub-review.orchestrator_routed_evidence.v1",
        "id": lease["id"],
        "kind": "resource-lease",
        "artifact": "review/resource_leases.json",
        "status": lease["status"],
        "result": lease["status"],
        "reason": lease["reason"],
    }


def routed_status_for_proof_receipt(receipt: dict) -> str:
    result = receipt["result"]
    if result in {"discriminating", "head_passed", "head_failed"}:
        return "tool-confirmed"
    if result in {
        "non_discriminating",
        "base_patch_failed",
        "timed_out",
        "skipped_budget",
        "skipped_profile",
    }:
        return "missing-evidence"
    return "recorded"


def observation_follow_up_question_text(evidence_need: str) -> str:
    if evidence_need == "proof-confirmation":
        return "Confirm whether routed proof evidence resolves this observation."
    if evidence_need == "source-route-confirmation":
        return "Confirm the changed source route or sibling path before promoting this observation."
    if evidence_need == "test-oracle-confirmation":
        return "Confirm the test oracle strength before promoting this observation."
    if evidence_need == "refutation-confirmation":
        return "Confirm the observation refutation still matches current PR evidence."
    if evidence_need == "parked-follow-up-confirmation":
        return "Confirm whether this observation remains parked outside current PR scope."
    if evidence_need == "evidence-gap-confirmation":
        return "Confirm whether this observation is still trust-affecting missing evidence."
    if evidence_need == "residual-risk-confirmation":
        return "Confirm whether this observation remains specific residual risk."
    return "Confirm whether this observation needs promotion, refutation, or parking."


def follow_up_question_text(disposition: str, evidence_need: str) -> str:
    if disposition == "refuted":
        return "Confirm the refutation still matches the current PR evidence."
    if disposition == "parked-follow-up":
        return "Confirm whether this parked follow-up should remain outside current PR scope."
    if evidence_need == "proof-confirmation":
        return "Confirm whether focused proof can resolve this summary-only candidate."
    if evidence_need == "source-route-confirmation":
        return "Confirm the changed source route or sibling path before promoting this candidate."
    if evidence_need == "test-oracle-confirmation":
        return "Confirm the test oracle strength before promoting this candidate."
    return "Confirm whether additional evidence should promote or keep this candidate summary-only."


def expected_candidate_records(review: dict) -> list[dict]:
    candidates: list[dict] = []
    for comment in review.get("inline_comments", []):
        fingerprint = hashlib.sha256(
            (
                "inline-comment\n"
                f"{comment.get('lane')}\n"
                f"{comment.get('path')}\n"
                f"{comment.get('line')}\n"
                f"{comment.get('body')}\n"
                f"{comment.get('evidence')}"
            ).encode("utf-8")
        ).hexdigest()
        candidates.append(
            {
                "schema": "ub-review.candidate.v1",
                "id": f"candidate-{len(candidates):04}-{fingerprint[:12]}",
                "lane": comment.get("lane"),
                "source": "inline-comment",
                "status": "accepted-inline",
                "disposition": "inline",
                "severity": comment.get("severity"),
                "confidence": comment.get("confidence"),
                "claim": comment.get("body"),
                "evidence": comment.get("evidence"),
                "path": comment.get("path"),
                "line": comment.get("line"),
                "side": comment.get("side"),
            }
        )
    for finding in review.get("summary_only_findings", []):
        fingerprint = hashlib.sha256(
            (
                "summary-only-finding\n"
                f"{finding.get('lane')}\n"
                f"{finding.get('reason')}\n"
                f"{finding.get('evidence')}"
            ).encode("utf-8")
        ).hexdigest()
        candidates.append(
            {
                "schema": "ub-review.candidate.v1",
                "id": f"candidate-{len(candidates):04}-{fingerprint[:12]}",
                "lane": finding.get("lane"),
                "source": "summary-only-finding",
                "status": "summary-only",
                "disposition": candidate_disposition_for_summary_finding(finding),
                "severity": finding.get("severity"),
                "confidence": finding.get("confidence"),
                "claim": finding.get("reason"),
                "evidence": finding.get("evidence"),
            }
        )
    return candidates


def candidate_disposition_for_summary_finding(finding: dict) -> str:
    reason = str(finding.get("reason", "")).lower()
    evidence = str(finding.get("evidence", "")).lower()
    if (
        "parked" in reason
        or "follow-up" in reason
        or "parked" in evidence
        or "follow-up" in evidence
    ):
        return "parked-follow-up"
    if (
        "false premise" in reason
        or "refuted" in reason
        or "false premise" in evidence
        or "refuted" in evidence
    ):
        return "refuted"
    if (
        "duplicate inline candidate merged" in reason
        or "summary-only guard rejected candidate" in reason
    ):
        return "dropped"
    return "summary-only"


def require_proof_request_ndjson(root: pathlib.Path, proof_requests: list[dict]) -> None:
    for request in proof_requests:
        require_proof_request_schema(request)
    require_proof_request_groups(root, proof_requests)
    require_proof_request_files(root, proof_requests)
    ndjson_path = root / "proof_requests.ndjson"
    text = read_text(ndjson_path)
    lines = [line for line in text.splitlines() if line.strip()]
    if len(lines) != len(proof_requests):
        fail("proof_requests.ndjson line count does not match review/proof_requests.json")
    for index, line in enumerate(lines):
        try:
            parsed = json.loads(line)
        except json.JSONDecodeError as error:
            fail(f"invalid proof_requests.ndjson line {index + 1}: {error}")
        if parsed != proof_requests[index]:
            fail(f"proof_requests.ndjson line {index + 1} does not match JSON artifact")
    proof_plan = read_text(root / "review/proof_plan.md")
    if "# Proof request plan" not in proof_plan:
        fail("review/proof_plan.md missing heading")
    if proof_requests and "Grouped proof broker tasks" not in proof_plan:
        fail("review/proof_plan.md missing grouped proof request summary")
    if "## Focused red/green proof plan" in proof_plan and not (
        "No proof broker commands were executed" in proof_plan
        or "Proof broker v0 executed focused HEAD proof only" in proof_plan
        or "Proof broker v0 executed focused proof under the runtime budget" in proof_plan
    ):
        fail("review/proof_plan.md missing proof execution/planner note")
    if not proof_requests and not (
        "No proof requests were emitted" in proof_plan
        or "No model-lane proof requests were emitted" in proof_plan
    ):
        fail("review/proof_plan.md missing empty proof request note")


def require_proof_planner_artifacts(root: pathlib.Path) -> None:
    planner_input = load_json(root / "review/proof_planner_input.json")
    planner_output = load_json(root / "review/proof_planner_output.json")
    if planner_input.get("schema") != "ub-review.proof_planner_input.v1":
        fail("review/proof_planner_input.json has wrong schema")
    if planner_output.get("schema") != "ub-review.proof_planner_output.v1":
        fail("review/proof_planner_output.json has wrong schema")
    proof_tasks = planner_output.get("proof_tasks")
    if not isinstance(proof_tasks, list):
        fail("review/proof_planner_output.json proof_tasks is not an array")
    for task in proof_tasks:
        require_proof_task_schema(task)
    lines = [line for line in read_text(root / "proof_tasks.ndjson").splitlines() if line.strip()]
    if len(lines) != len(proof_tasks):
        fail("proof_tasks.ndjson line count does not match proof planner output")
    for index, line in enumerate(lines):
        try:
            parsed = json.loads(line)
        except json.JSONDecodeError as error:
            fail(f"invalid proof_tasks.ndjson line {index + 1}: {error}")
        if parsed != proof_tasks[index]:
            fail(f"proof_tasks.ndjson line {index + 1} does not match proof planner output")
    require_work_queue_artifacts(root, proof_tasks)


def require_proof_task_schema(task: dict) -> None:
    if not isinstance(task, dict):
        fail(f"proof task is not an object: {task!r}")
    if task.get("schema") != "ub-review.proof_task.v1":
        fail(f"proof task has wrong schema: {task!r}")
    for field in [
        "id",
        "kind",
        "source",
        "priority",
        "packet_policy",
        "gate_policy",
        "mode",
        "command",
        "purpose",
        "value",
        "cost",
        "status",
    ]:
        if not isinstance(task.get(field), str) or not task[field]:
            fail(f"proof task missing string field {field}: {task!r}")
    if task.get("packet_policy") not in {
        "must-run",
        "include-if-ready",
        "late-follow-up",
        "adaptive",
        "artifact-only",
        "gate-only",
    }:
        fail(f"proof task packet_policy is invalid: {task!r}")
    deadline = task.get("deadline_sec")
    if not isinstance(deadline, int) or isinstance(deadline, bool) or deadline < 0:
        fail(f"proof task deadline_sec is invalid: {task!r}")
    timeout = task.get("timeout_sec")
    if not isinstance(timeout, int) or isinstance(timeout, bool) or timeout < 0:
        fail(f"proof task timeout_sec is invalid: {task!r}")
    if timeout != deadline:
        fail(f"proof task timeout_sec does not match deadline_sec: {task!r}")
    consumers = task.get("consumers")
    if not isinstance(consumers, list) or not all(
        isinstance(consumer, str) and consumer for consumer in consumers
    ):
        fail(f"proof task consumers is not a string array: {task!r}")
    lease = task.get("lease")
    if not isinstance(lease, dict):
        fail(f"proof task lease is not an object: {task!r}")
    for field in ["cpu", "memory_mb", "disk_mb"]:
        value = lease.get(field)
        if not isinstance(value, int) or isinstance(value, bool) or value < 0:
            fail(f"proof task lease {field} is invalid: {task!r}")
    if lease.get("network") not in {True, False}:
        fail(f"proof task lease network is not boolean: {task!r}")
    lease_timeout = lease.get("timeout_sec")
    if (
        not isinstance(lease_timeout, int)
        or isinstance(lease_timeout, bool)
        or lease_timeout != deadline
    ):
        fail(f"proof task lease timeout_sec does not match deadline_sec: {task!r}")


def require_work_queue_artifacts(root: pathlib.Path, proof_tasks: list[dict]) -> None:
    tool_status = load_json(root / "tool-status.json")
    tools = tool_status.get("tools")
    if not isinstance(tools, list):
        fail("tool-status.json tools is not an array")
    queue = load_json(root / "work_queue.json")
    if queue.get("schema") != "ub-review.work_queue.v1":
        fail("work_queue.json has wrong schema")
    for field in ["initial_packet_deadline_sec", "follow_up_deadline_sec"]:
        value = queue.get(field)
        if not isinstance(value, int) or isinstance(value, bool) or value <= 0:
            fail(f"work_queue.json {field} is invalid: {value!r}")
    tasks = queue.get("tasks")
    if not isinstance(tasks, list):
        fail("work_queue.json tasks is not an array")
    if len(tasks) != len(tools) + len(proof_tasks):
        fail("work_queue.json task count does not match tool status plus proof planner output")

    sensor_queue_tasks = tasks[: len(tools)]
    sensor_tasks_by_id: dict[str, dict] = {}
    for task in sensor_queue_tasks:
        task_id = task.get("id")
        if not isinstance(task_id, str) or not task_id:
            fail(f"sensor work queue task id is invalid: {task!r}")
        if task_id in sensor_tasks_by_id:
            fail(f"duplicate sensor work queue task id: {task_id}")
        sensor_tasks_by_id[task_id] = task
    for tool in tools:
        tool_id = tool.get("id")
        task = sensor_tasks_by_id.get(f"sensor-{tool_id}")
        if task is None:
            fail(f"missing sensor work queue task for tool {tool_id!r}")
        require_sensor_work_queue_task_schema(root, task, tool)
    proof_queue_tasks = tasks[len(tools) :]
    for index, task in enumerate(proof_queue_tasks):
        require_proof_work_queue_task_schema(task, proof_tasks[index])

    lines = [line for line in read_text(root / "work_events.ndjson").splitlines() if line.strip()]
    if len(lines) != len(tasks):
        fail("work_events.ndjson line count does not match work_queue.json tasks")
    for index, line in enumerate(lines):
        try:
            event = json.loads(line)
        except json.JSONDecodeError as error:
            fail(f"invalid work_events.ndjson line {index + 1}: {error}")
        require_work_event_schema(event, tasks[index])


def require_sensor_work_queue_task_schema(root: pathlib.Path, task: dict, tool: dict) -> None:
    require_work_queue_task_base_schema(task)
    tool_id = tool.get("id")
    if not isinstance(tool_id, str) or not tool_id:
        fail(f"tool status id is invalid: {tool!r}")
    expected = {
        "id": f"sensor-{tool_id}",
        "kind": "sensor",
        "source": "tool-registry",
        "packet_policy": "must-run"
        if tool.get("required") is True
        else "include-if-ready"
        if tool.get("planned_run") is True
        else "artifact-only",
        "deadline_sec": tool.get("timeout_sec") if tool.get("planned_run") is True else 0,
        "receipt_path": f"sensors/{tool_id}/ub-review-sensor-status.json",
        "status": "planned" if tool.get("planned_run") is True else "skipped",
        "task_path": "resolved-tools.json",
    }
    receipt_ready = (root / expected["receipt_path"]).is_file()
    expected["initial_packet_status"] = expected_work_queue_initial_packet_status(
        expected["packet_policy"],
        expected["status"],
        receipt_ready,
    )
    for field, value in expected.items():
        if task.get(field) != value:
            fail(f"sensor work queue task {field} mismatch: task={task!r} tool={tool!r}")
    if tool.get("required") is True:
        expected_priority = "high"
        expected_gate_policy = "gate-required"
    elif tool.get("planned_run") is True:
        expected_priority = "medium"
        expected_gate_policy = "trust-affecting" if tool.get("gate") is not None else "review-context"
    else:
        expected_priority = "low"
        expected_gate_policy = "trust-affecting" if tool.get("gate") is not None else "artifact-only"
    if task.get("priority") != expected_priority:
        fail(f"sensor work queue task priority mismatch: task={task!r} tool={tool!r}")
    if task.get("gate_policy") != expected_gate_policy:
        fail(f"sensor work queue task gate_policy mismatch: task={task!r} tool={tool!r}")
    if task.get("dedupe_key") != f"tool-registry:sensor:{tool_id}":
        fail(f"sensor work queue task dedupe_key mismatch: task={task!r} tool={tool!r}")
    consumers = task.get("consumers")
    if not isinstance(consumers, list) or "compiler" not in consumers:
        fail(f"sensor work queue task consumers missing compiler: {task!r}")
    lease = task.get("lease")
    if not isinstance(lease, dict):
        fail(f"sensor work queue task lease is not an object: {task!r}")
    if lease.get("timeout_sec") != task.get("deadline_sec"):
        fail(f"sensor work queue task lease timeout mismatch: {task!r}")
    if lease.get("network") not in {True, False}:
        fail(f"sensor work queue task lease network is not boolean: {task!r}")
    for field in ["cpu", "memory_mb", "disk_mb"]:
        value = lease.get(field)
        if not isinstance(value, int) or isinstance(value, bool) or value < 0:
            fail(f"sensor work queue task lease {field} is invalid: {task!r}")


def require_proof_work_queue_task_schema(task: dict, proof_task: dict) -> None:
    require_work_queue_task_base_schema(task)
    mirrored = [
        "id",
        "kind",
        "source",
        "priority",
        "packet_policy",
        "deadline_sec",
        "consumers",
        "gate_policy",
        "lease",
        "status",
    ]
    for field in mirrored:
        if task.get(field) != proof_task.get(field):
            fail(f"work queue task {field} does not match proof task: {task!r}")
    if task.get("receipt_path") != "review/proof_receipts.json":
        fail(f"work queue task receipt_path is invalid: {task!r}")
    if task.get("task_path") != "proof_tasks.ndjson":
        fail(f"work queue task task_path is invalid: {task!r}")
    expected_initial_packet_status = expected_work_queue_initial_packet_status(
        proof_task.get("packet_policy"),
        proof_task.get("status"),
        False,
    )
    if task.get("initial_packet_status") != expected_initial_packet_status:
        fail(f"proof work queue task initial_packet_status is invalid: {task!r}")


def require_work_queue_task_base_schema(task: dict) -> None:
    if not isinstance(task, dict):
        fail(f"work queue task is not an object: {task!r}")
    if task.get("schema") != "ub-review.work_queue_task.v1":
        fail(f"work queue task has wrong schema: {task!r}")
    if not isinstance(task.get("dedupe_key"), str) or not task["dedupe_key"]:
        fail(f"work queue task dedupe_key is invalid: {task!r}")
    if task.get("initial_packet_status") not in {
        "ready_for_initial_packet",
        "pending_initial_packet",
        "not_initial_packet",
    }:
        fail(f"work queue task initial_packet_status is invalid: {task!r}")


def expected_work_queue_initial_packet_status(
    packet_policy: object, status: object, receipt_ready: bool
) -> str:
    if (
        packet_policy in {"must-run", "include-if-ready"}
        and status == "planned"
        and receipt_ready
    ):
        return "ready_for_initial_packet"
    if packet_policy in {"must-run", "include-if-ready"} and status == "planned":
        return "pending_initial_packet"
    if packet_policy in {"late-follow-up", "adaptive"} and status == "planned":
        return "pending_initial_packet"
    return "not_initial_packet"


def require_work_event_schema(event: dict, task: dict) -> None:
    if not isinstance(event, dict):
        fail(f"work event is not an object: {event!r}")
    if event.get("schema") != "ub-review.work_event.v1":
        fail(f"work event has wrong schema: {event!r}")
    if event.get("kind") != "task_planned":
        fail(f"work event kind expected task_planned, got {event.get('kind')!r}")
    expected = {
        "task_id": "id",
        "task_kind": "kind",
        "source": "source",
        "packet_policy": "packet_policy",
        "deadline_sec": "deadline_sec",
        "consumers": "consumers",
        "gate_policy": "gate_policy",
        "status": "status",
        "initial_packet_status": "initial_packet_status",
        "receipt_path": "receipt_path",
    }
    for event_field, task_field in expected.items():
        if event.get(event_field) != task.get(task_field):
            fail(
                f"work event {event_field} does not match queue task {task_field}: {event!r}"
            )


def require_proof_request_files(root: pathlib.Path, proof_requests: list[dict]) -> None:
    proof_request_dir = root / "proof_requests"
    if not proof_request_dir.is_dir():
        if not proof_requests:
            return
        fail("missing proof_requests directory")
    expected = {f"{sanitize_artifact_name(request['id'])}.json": request for request in proof_requests}
    actual = sorted(path.name for path in proof_request_dir.glob("*.json"))
    if actual != sorted(expected):
        fail("proof_requests directory entries do not match review/proof_requests.json")
    for name, expected_request in expected.items():
        parsed = load_json(proof_request_dir / name)
        if parsed != expected_request:
            fail(f"proof_requests/{name} does not match review/proof_requests.json")


def require_proof_receipt_ndjson(root: pathlib.Path, proof_receipts: list[dict]) -> None:
    receipt_ids: set[str] = set()
    for receipt in proof_receipts:
        require_proof_receipt_schema(root, receipt)
        receipt_id = receipt["id"]
        if receipt_id in receipt_ids:
            fail(f"duplicate proof receipt id: {receipt_id}")
        receipt_ids.add(receipt_id)
    ndjson_path = root / "proof_receipts.ndjson"
    text = read_text(ndjson_path)
    lines = [line for line in text.splitlines() if line.strip()]
    if len(lines) != len(proof_receipts):
        fail("proof_receipts.ndjson line count does not match review/proof_receipts.json")
    for index, line in enumerate(lines):
        try:
            parsed = json.loads(line)
        except json.JSONDecodeError as error:
            fail(f"invalid proof_receipts.ndjson line {index + 1}: {error}")
        if parsed != proof_receipts[index]:
            fail(f"proof_receipts.ndjson line {index + 1} does not match JSON artifact")


def require_receipt_route_artifacts(
    root: pathlib.Path, proof_receipts: list[dict], resource_leases: list[dict]
) -> None:
    artifact = load_json(root / "review/receipt_routes.json")
    if not isinstance(artifact, dict):
        fail("review/receipt_routes.json is not an object")
    if artifact.get("schema") != "ub-review.receipt_routes.v1":
        fail(f"receipt routes artifact has wrong schema: {artifact!r}")
    if artifact.get("source_artifacts") != [
        "review/proof_receipts.json",
        "review/resource_leases.json",
    ]:
        fail(f"receipt routes source_artifacts are unsupported: {artifact!r}")
    routes = artifact.get("routes")
    if not isinstance(routes, list):
        fail("review/receipt_routes.json routes is not an array")
    expected = expected_receipt_routes(proof_receipts, resource_leases)
    if routes != expected:
        fail("review/receipt_routes.json routes do not match proof receipts and leases")
    lines = [
        line
        for line in read_text(root / "receipt_routes.ndjson").splitlines()
        if line.strip()
    ]
    if len(lines) != len(routes):
        fail("receipt_routes.ndjson line count does not match review/receipt_routes.json")
    for index, line in enumerate(lines):
        try:
            parsed = json.loads(line)
        except json.JSONDecodeError as error:
            fail(f"invalid receipt_routes.ndjson line {index + 1}: {error}")
        if parsed != routes[index]:
            fail(f"receipt_routes.ndjson line {index + 1} does not match JSON artifact")
        require_receipt_route_schema(parsed)


def expected_receipt_routes(
    proof_receipts: list[dict], resource_leases: list[dict]
) -> list[dict]:
    routes = []
    for receipt in proof_receipts:
        lease_ids = [
            lease["id"]
            for lease in resource_leases
            if lease.get("consumer") == receipt["id"]
        ]
        source_artifacts = ["review/proof_receipts.json"]
        if lease_ids:
            source_artifacts.append("review/resource_leases.json")
        routes.append(
            {
                "schema": "ub-review.receipt_route.v1",
                "id": f"receipt-route-{receipt['id']}",
                "receipt_id": receipt["id"],
                "phase": receipt_route_phase(receipt),
                "receipt_kind": receipt["kind"],
                "result": receipt["result"],
                "status": routed_status_for_proof_receipt(receipt),
                "requested_by": receipt["requested_by"],
                "request_ids": receipt["request_ids"],
                "consumers": receipt_route_consumers(receipt),
                "lease_ids": lease_ids,
                "source_artifacts": source_artifacts,
                "reason": receipt["reason"],
            }
        )
    return routes


def receipt_route_phase(receipt: dict) -> str:
    if any(
        isinstance(lane, str) and lane.startswith("orchestrator-follow-up")
        for lane in receipt["requested_by"]
    ) or any("follow-up" in request_id for request_id in receipt["request_ids"]):
        return "follow-up-receipt"
    if "proof-broker" in receipt["requested_by"] and not receipt["request_ids"]:
        return "initial-diff-receipt"
    return "model-request-receipt"


def receipt_route_consumers(receipt: dict) -> list[str]:
    consumers: list[str] = []
    if "proof-broker" in receipt["requested_by"]:
        if receipt["kind"] in {"focused-head", "focused-red-green"}:
            append_unique(consumers, "tests-oracle")
            append_unique(consumers, "opposition")
        elif receipt["kind"] == "focused-build":
            append_unique(consumers, "architecture")
    for lane in receipt["requested_by"]:
        if lane != "proof-broker":
            append_unique(consumers, lane)
    append_unique(consumers, "compiler")
    return consumers


def require_receipt_route_schema(route: dict) -> None:
    if not isinstance(route, dict):
        fail(f"receipt route is not an object: {route!r}")
    if route.get("schema") != "ub-review.receipt_route.v1":
        fail(f"receipt route has wrong schema: {route!r}")
    for field in [
        "id",
        "receipt_id",
        "phase",
        "receipt_kind",
        "result",
        "status",
        "reason",
    ]:
        if not isinstance(route.get(field), str) or not route[field]:
            fail(f"receipt route missing string field {field}: {route!r}")
    if route["phase"] not in {
        "initial-diff-receipt",
        "model-request-receipt",
        "follow-up-receipt",
    }:
        fail(f"receipt route phase is unsupported: {route!r}")
    for field in ["requested_by", "request_ids", "consumers", "lease_ids", "source_artifacts"]:
        values = route.get(field)
        if not isinstance(values, list) or not all(
            isinstance(value, str) and value for value in values
        ):
            fail(f"receipt route {field} is not a string array: {route!r}")


def require_resource_lease_artifacts(
    root: pathlib.Path, proof_receipts: list[dict], resource_leases: list[dict]
) -> None:
    lease_ids: set[str] = set()
    for lease in resource_leases:
        require_resource_lease_schema(lease)
        lease_id = lease["id"]
        if lease_id in lease_ids:
            fail(f"duplicate resource lease id: {lease_id}")
        lease_ids.add(lease_id)
    ndjson_path = root / "resource_leases.ndjson"
    text = read_text(ndjson_path)
    lines = [line for line in text.splitlines() if line.strip()]
    if len(lines) != len(resource_leases):
        fail("resource_leases.ndjson line count does not match review/resource_leases.json")
    for index, line in enumerate(lines):
        try:
            parsed = json.loads(line)
        except json.JSONDecodeError as error:
            fail(f"invalid resource_leases.ndjson line {index + 1}: {error}")
        if parsed != resource_leases[index]:
            fail(f"resource_leases.ndjson line {index + 1} does not match JSON artifact")

    resource_plan = read_text(root / "review/resource_plan.md")
    if "# Resource lease plan" not in resource_plan:
        fail("review/resource_plan.md missing heading")
    if resource_leases and "## Focused proof leases" not in resource_plan:
        fail("review/resource_plan.md missing focused proof lease section")

    focused_leases = {}
    for lease in resource_leases:
        if lease.get("kind") not in {"focused-test", "focused-build"}:
            continue
        consumer = lease["consumer"]
        if consumer in focused_leases:
            fail(f"duplicate focused proof lease consumer: {consumer}")
        focused_leases[consumer] = lease
    focused_receipts = [
        receipt
        for receipt in proof_receipts
        if receipt.get("kind") in {"focused-head", "focused-red-green", "focused-build"}
    ]
    for receipt in focused_receipts:
        lease = focused_leases.get(receipt["id"])
        if lease is None:
            fail(f"focused proof receipt lacks resource lease: {receipt!r}")
        expected_kind = "focused-build" if receipt["kind"] == "focused-build" else "focused-test"
        if lease["kind"] != expected_kind:
            fail(
                "focused proof lease kind does not match receipt kind: "
                f"lease={lease!r} receipt={receipt!r}"
            )
        expected = expected_lease_statuses_for_proof_result(receipt["result"])
        if lease["status"] not in expected:
            fail(
                "focused proof lease status does not match receipt result: "
                f"lease={lease!r} receipt={receipt!r}"
            )

    receipt_ids = {receipt["id"] for receipt in focused_receipts}
    for consumer in focused_leases:
        if consumer not in receipt_ids:
            fail(f"focused proof lease has no matching proof receipt: {consumer}")


def expected_lease_statuses_for_proof_result(result: str) -> set[str]:
    if result == "skipped_budget":
        return {"exhausted"}
    if result == "skipped_profile":
        return {"granted", "skipped_profile"}
    return {"granted"}


def require_proof_request_groups(root: pathlib.Path, proof_requests: list[dict]) -> None:
    groups = load_json(root / "review/proof_request_groups.json")
    if not isinstance(groups, list):
        fail("review/proof_request_groups.json is not an array")
    expected = expected_proof_request_groups(proof_requests)
    if groups != expected:
        fail("review/proof_request_groups.json does not match canonical proof request grouping")
    for group in groups:
        require_proof_request_group_schema(group)


def expected_proof_request_groups(proof_requests: list[dict]) -> list[dict]:
    groups: dict[tuple[str, str, int], dict] = {}
    for request in proof_requests:
        command = request["command"]
        cost = request["cost"]
        timeout_sec = request["timeout_sec"]
        group_command = canonical_proof_request_group_command(command, cost)
        key = (group_command, cost, timeout_sec)
        group = groups.get(key)
        if group is None:
            digest = hashlib.sha256(
                f"{group_command}\n{cost}\n{timeout_sec}".encode()
            ).hexdigest()
            group = {
                "schema": "ub-review.proof_request_group.v1",
                "id": f"proof-group-{digest[:12]}",
                "command": command,
                "cost": cost,
                "timeout_sec": timeout_sec,
                "required": False,
                "status": "invalid",
                "requested_by": [],
                "request_ids": [],
                "reasons": [],
                "duplicate_count": 0,
            }
            groups[key] = group
        group["required"] = bool(group["required"] or request["required"])
        if request["status"] == "requested":
            group["status"] = "requested"
        elif request["status"] == "unsupported" and group["status"] != "requested":
            group["status"] = "unsupported"
        append_unique(group["requested_by"], request["lane"])
        for lane in request["requested_by"]:
            append_unique(group["requested_by"], lane)
        append_unique(group["request_ids"], request["id"])
        append_unique(group["reasons"], request["reason"])
        group["duplicate_count"] += 1
    return [groups[key] for key in sorted(groups)]


def canonical_proof_request_group_command(command: str, cost: str) -> str:
    if cost != "focused-test":
        return command
    parts = command.split()
    target = focused_bun_request_parts(parts)
    if target is None:
        return command
    file, args = target
    return (
        f"focused-bun:{normalize_repo_path(file)}:"
        f"{focused_test_name_arg(args) or ''}"
    )


def focused_bun_request_parts(parts: list[str]) -> tuple[str, list[str]] | None:
    if len(parts) >= 3 and parts[0] == "bun" and parts[1] == "test":
        return parts[2], parts[3:]
    if len(parts) >= 4 and parts[0] == "bun" and parts[1] == "bd" and parts[2] == "test":
        return parts[3], parts[4:]
    if (
        len(parts) >= 4
        and parts[0] == "USE_SYSTEM_BUN=1"
        and parts[1] == "bun"
        and parts[2] == "test"
    ):
        return parts[3], parts[4:]
    return None


def focused_test_name_arg(args: list[str]) -> str | None:
    try:
        index = next(
            index
            for index, arg in enumerate(args)
            if arg in {"-t", "--test-name-pattern"}
        )
    except StopIteration:
        return None
    tokens = []
    for token in args[index + 1 :]:
        if token.startswith("-"):
            break
        tokens.append(token)
    value = strip_matching_quotes(" ".join(tokens).strip())
    return value or None


def strip_matching_quotes(value: str) -> str:
    if len(value) >= 2 and value[0] == value[-1] and value[0] in {"'", '"'}:
        return value[1:-1]
    return value


def normalize_repo_path(value: str) -> str:
    value = value.strip()
    if value.startswith("b/"):
        value = value[2:]
    return value.replace("\\", "/")


def append_unique(values: list[str], value: str) -> None:
    if value not in values:
        values.append(value)


def require_observation_files(
    root: pathlib.Path, observations: list[dict], follow_up_tasks: list[dict]
) -> None:
    observation_dir = root / "observations"
    if not observation_dir.is_dir():
        fail("missing observations directory")
    for observation in observations:
        require_observation_schema(observation)
    lanes = {observation["lane"] for observation in observations}
    for lane in lanes:
        path = observation_dir / f"{sanitize_artifact_name(lane)}.ndjson"
        require_file(path)
        lane_observations = []
        for line_number, line in enumerate(read_text(path).splitlines(), start=1):
            if not line.strip():
                continue
            try:
                parsed = json.loads(line)
            except json.JSONDecodeError as error:
                fail(f"invalid observation NDJSON {path}:{line_number}: {error}")
            if parsed.get("lane") != lane:
                fail(f"observation NDJSON {path}:{line_number} has wrong lane")
            require_observation_schema(parsed)
            lane_observations.append(parsed)
        expected = [observation for observation in observations if observation["lane"] == lane]
        if lane_observations != expected:
            fail(f"observation NDJSON {path} does not match review/observations.json")
    require_question_observation_files(root, observations, follow_up_tasks)


def require_question_observation_files(
    root: pathlib.Path, observations: list[dict], follow_up_tasks: list[dict]
) -> None:
    questions_dir = root / "questions"
    if not questions_dir.is_dir():
        fail("missing questions directory")

    expected: dict[str, dict[str, dict]] = {}
    for observation in observations:
        lane_name = sanitize_artifact_name(observation["lane"])
        question_name = f"{sanitize_artifact_name(observation['question'])}.json"
        lane_questions = expected.setdefault(lane_name, {})
        artifact = lane_questions.setdefault(
            question_name,
            {
                "schema": "ub-review.question_observations.v1",
                "lane": observation["lane"],
                "question": observation["question"],
                "observations": [],
            },
        )
        if (
            artifact["lane"] != observation["lane"]
            or artifact["question"] != observation["question"]
        ):
            fail(
                "questions artifact path collision for "
                f"{observation['lane']}/{observation['question']}"
            )
        artifact["observations"].append(observation)

    actual_lane_dirs = []
    for path in questions_dir.iterdir():
        if not path.is_dir():
            fail(f"unexpected questions entry: {path.name}")
        actual_lane_dirs.append(path.name)
    if follow_up_tasks:
        expected.setdefault("orchestrator-follow-up", {})
    if sorted(actual_lane_dirs) != sorted(expected):
        fail("questions directory entries do not match review/observations.json")

    for lane_name, expected_questions in expected.items():
        if lane_name == "orchestrator-follow-up":
            continue
        lane_dir = questions_dir / lane_name
        actual_question_files = []
        for path in lane_dir.iterdir():
            if not path.is_file():
                fail(f"unexpected questions/{lane_name} entry: {path.name}")
            actual_question_files.append(path.name)
        if sorted(actual_question_files) != sorted(expected_questions):
            fail(
                f"questions/{lane_name} entries do not match review/observations.json"
            )
        for name, expected_artifact in expected_questions.items():
            parsed = load_json(lane_dir / name)
            if parsed != expected_artifact:
                fail(
                    f"questions/{lane_name}/{name} does not match review/observations.json"
                )
    require_follow_up_question_packet_files(root, follow_up_tasks)


def require_follow_up_question_packet_files(
    root: pathlib.Path, follow_up_tasks: list[dict]
) -> None:
    follow_up_dir = root / "questions" / "orchestrator-follow-up"
    if not follow_up_tasks:
        if follow_up_dir.exists():
            fail("questions/orchestrator-follow-up exists without follow-up tasks")
        return
    if not follow_up_dir.is_dir():
        fail("missing questions/orchestrator-follow-up directory")
    expected = {
        f"{sanitize_artifact_name(task['id'])}.json": expected_follow_up_question_packet(task)
        for task in follow_up_tasks
    }
    actual_files = []
    for path in follow_up_dir.iterdir():
        if not path.is_file():
            fail(f"unexpected questions/orchestrator-follow-up entry: {path.name}")
        actual_files.append(path.name)
    if sorted(actual_files) != sorted(expected):
        fail("questions/orchestrator-follow-up entries do not match follow_up_tasks")
    for name, expected_packet in expected.items():
        parsed = load_json(follow_up_dir / name)
        require_follow_up_question_packet_schema(parsed)
        if parsed != expected_packet:
            fail(
                f"questions/orchestrator-follow-up/{name} does not match orchestrator plan"
            )


def require_follow_up_results(
    root: pathlib.Path, follow_up_tasks: list[dict]
) -> list[dict]:
    results = load_json(root / "review/follow_up_results.json")
    if not isinstance(results, list):
        fail("review/follow_up_results.json is not an array")
    if len(results) != len(follow_up_tasks):
        fail("review/follow_up_results.json count does not match follow_up_tasks")
    lines = [
        line
        for line in read_text(root / "follow_up_results.ndjson").splitlines()
        if line.strip()
    ]
    if len(lines) != len(results):
        fail("follow_up_results.ndjson line count does not match review/follow_up_results.json")
    for index, task in enumerate(follow_up_tasks):
        result = results[index]
        if not isinstance(result, dict):
            fail(f"follow-up result {index + 1} is not an object: {result!r}")
        try:
            parsed = json.loads(lines[index])
        except json.JSONDecodeError as error:
            fail(f"invalid follow_up_results.ndjson line {index + 1}: {error}")
        if parsed != result:
            fail(f"follow_up_results.ndjson line {index + 1} does not match JSON artifact")
        require_follow_up_result_schema(root, result, task)
    return results


def require_model_stage_artifacts(
    root: pathlib.Path, review: dict, follow_up_results: list[dict]
) -> None:
    stages = load_json(root / "review/model_stages.json")
    if not isinstance(stages, list):
        fail("review/model_stages.json is not an array")
    lines = [
        line
        for line in read_text(root / "model_stages.ndjson").splitlines()
        if line.strip()
    ]
    if len(lines) != len(stages):
        fail("model_stages.ndjson line count does not match review/model_stages.json")
    for index, stage in enumerate(stages):
        if not isinstance(stage, dict):
            fail(f"model stage {index + 1} is not an object: {stage!r}")
        try:
            parsed = json.loads(lines[index])
        except json.JSONDecodeError as error:
            fail(f"invalid model_stages.ndjson line {index + 1}: {error}")
        if parsed != stage:
            fail(f"model_stages.ndjson line {index + 1} does not match JSON artifact")
        require_model_stage_schema(stage)

    model_lanes = review.get("model_lanes", [])
    if not isinstance(model_lanes, list):
        fail("review.json model_lanes is not an array")
    expected_total = len(model_lanes) + len(follow_up_results)
    if len(stages) != expected_total:
        fail(
            "review/model_stages.json count does not match model_lanes + follow_up_results"
        )
    for index, receipt in enumerate(model_lanes):
        stage = stages[index]
        if stage.get("source") != expected_model_lane_stage_source(receipt.get("lane")):
            fail(f"model stage source does not match model lane receipt: {stage!r}")
        if stage.get("stage") != expected_model_lane_stage(receipt.get("lane")):
            fail(f"model stage value does not match model lane receipt: {stage!r}")
        for field in ["lane", "status", "reason", "provider", "model", "endpoint_kind"]:
            if stage.get(field) != receipt.get(field):
                fail(f"model stage {field} does not match model lane receipt: {stage!r}")
        for field in ["task_id", "group_id", "packet_path"]:
            if field in stage:
                fail(f"primary model stage unexpectedly has {field}: {stage!r}")
    offset = len(model_lanes)
    for index, result in enumerate(follow_up_results):
        stage = stages[offset + index]
        if stage.get("source") != "orchestrator-follow-up":
            fail(f"follow-up model stage has wrong source: {stage!r}")
        if stage.get("stage") != result.get("stage"):
            fail(f"follow-up model stage does not match follow-up result: {stage!r}")
        for field in ["status", "reason"]:
            if stage.get(field) != result.get(field):
                fail(f"follow-up model stage {field} does not match result: {stage!r}")
        if stage.get("lane") != result.get("model_lane"):
            fail(f"follow-up model stage lane does not match result: {stage!r}")
        if stage.get("task_id") != result.get("task_id"):
            fail(f"follow-up model stage task_id does not match result: {stage!r}")
        if stage.get("group_id") != result.get("group_id"):
            fail(f"follow-up model stage group_id does not match result: {stage!r}")
        if stage.get("packet_path") != result.get("packet_path"):
            fail(f"follow-up model stage packet_path does not match result: {stage!r}")


def require_model_stage_schema(stage: dict) -> None:
    if stage.get("schema") != "ub-review.model_stage.v1":
        fail(f"model stage has wrong schema: {stage!r}")
    for field in [
        "lane",
        "source",
        "stage",
        "stage_reason",
        "status",
        "reason",
        "provider",
        "model",
        "endpoint_kind",
    ]:
        if not isinstance(stage.get(field), str) or not stage.get(field):
            fail(f"model stage {field} is missing or empty: {stage!r}")
    if stage.get("source") not in {
        "model-lane",
        "proof-planner",
        "refuter",
        "orchestrator-follow-up",
    }:
        fail(f"model stage source is unsupported: {stage!r}")
    if stage.get("stage") not in {"primary", "secondary", "tertiary"}:
        fail(f"model stage value is unsupported: {stage!r}")
    for field in ["task_id", "group_id", "packet_path", "response_shape"]:
        if field in stage and not isinstance(stage.get(field), str):
            fail(f"model stage {field} is not a string: {stage!r}")
    if "duration_ms" in stage and not isinstance(stage.get("duration_ms"), int):
        fail(f"model stage duration_ms is not an integer: {stage!r}")
    if "http_status" in stage and not isinstance(stage.get("http_status"), int):
        fail(f"model stage http_status is not an integer: {stage!r}")


def expected_model_lane_stage(lane: object) -> str:
    return "tertiary" if lane == "refuter" else "primary"


def expected_model_lane_stage_source(lane: object) -> str:
    if lane == "proof-planner":
        return "proof-planner"
    if lane == "refuter":
        return "refuter"
    return "model-lane"


def require_follow_up_outputs(root: pathlib.Path, results: list[dict]) -> list[dict]:
    outputs = load_json(root / "review/follow_up_outputs.json")
    if not isinstance(outputs, list):
        fail("review/follow_up_outputs.json is not an array")
    if len(outputs) != len(results):
        fail("review/follow_up_outputs.json count does not match follow-up results")
    lines = [
        line
        for line in read_text(root / "follow_up_outputs.ndjson").splitlines()
        if line.strip()
    ]
    if len(lines) != len(outputs):
        fail("follow_up_outputs.ndjson line count does not match review/follow_up_outputs.json")
    for index, result in enumerate(results):
        output = outputs[index]
        if not isinstance(output, dict):
            fail(f"follow-up output {index + 1} is not an object: {output!r}")
        try:
            parsed = json.loads(lines[index])
        except json.JSONDecodeError as error:
            fail(f"invalid follow_up_outputs.ndjson line {index + 1}: {error}")
        if parsed != output:
            fail(f"follow_up_outputs.ndjson line {index + 1} does not match JSON artifact")
        require_follow_up_output_schema(output, result)
    return outputs


def require_follow_up_evidence(root: pathlib.Path, outputs: list[dict]) -> dict:
    evidence = load_json(root / "review/follow_up_evidence.json")
    if not isinstance(evidence, dict):
        fail("review/follow_up_evidence.json is not an object")
    if evidence.get("schema") != "ub-review.follow_up_evidence.v1":
        fail(f"follow-up evidence has wrong schema: {evidence!r}")
    if evidence.get("follow_up_outputs") != len(outputs):
        fail("follow-up evidence output count does not match follow_up_outputs")
    for field in [
        "inline_comments",
        "summary_only_findings",
        "observations",
        "proof_requests",
    ]:
        values = evidence.get(field)
        if not isinstance(values, list):
            fail(f"follow-up evidence {field} is not an array: {evidence!r}")
        expected = []
        for output in outputs:
            expected.extend(output[field])
        if values != expected:
            fail(f"follow-up evidence {field} does not match flattened outputs")
    return evidence


def require_resolved_candidate_artifacts(
    root: pathlib.Path, follow_up_results: list[dict], follow_up_outputs: list[dict]
) -> None:
    candidates = load_json(root / "review/candidates.json")
    resolved = load_json(root / "review/resolved_candidates.json")
    if not isinstance(resolved, list):
        fail("review/resolved_candidates.json is not an array")
    expected = expected_resolved_candidate_records(
        candidates, follow_up_results, follow_up_outputs
    )
    if resolved != expected:
        fail("review/resolved_candidates.json does not match candidates plus follow-up outputs")
    lines = [
        line
        for line in read_text(root / "resolved_candidates.ndjson").splitlines()
        if line.strip()
    ]
    if len(lines) != len(resolved):
        fail("resolved_candidates.ndjson line count does not match review/resolved_candidates.json")
    for index, line in enumerate(lines):
        try:
            parsed = json.loads(line)
        except json.JSONDecodeError as error:
            fail(f"invalid resolved_candidates.ndjson line {index + 1}: {error}")
        if parsed != resolved[index]:
            fail(f"resolved_candidates.ndjson line {index + 1} does not match JSON artifact")
        require_resolved_candidate_schema(parsed)


def expected_resolved_candidate_records(
    candidates: list[dict], follow_up_results: list[dict], follow_up_outputs: list[dict]
) -> list[dict]:
    result_task_ids = {result["task_id"] for result in follow_up_results}
    records = []
    for candidate in candidates:
        linked = [
            output
            for output in follow_up_outputs
            if output["task_id"] in result_task_ids
            and candidate["id"] in output.get("candidate_ids", [])
        ]
        record = resolved_candidate_record(candidate, linked)
        records.append(record)
    return records


def resolved_candidate_record(candidate: dict, follow_up_outputs: list[dict]) -> dict:
    resolved_status, resolved_disposition, resolution_source, reason, evidence = (
        resolve_candidate_from_follow_ups(candidate, follow_up_outputs)
    )
    return {
        "schema": "ub-review.resolved_candidate.v1",
        "candidate_id": candidate["id"],
        "lane": candidate["lane"],
        "source": candidate["source"],
        "original_status": candidate["status"],
        "original_disposition": candidate["disposition"],
        "resolved_status": resolved_status,
        "resolved_disposition": resolved_disposition,
        "resolution_source": resolution_source,
        "source_artifacts": [
            "review/candidates.json",
            "review/follow_up_results.json",
            "review/follow_up_outputs.json",
        ],
        "reason": reason,
        "follow_up_task_ids": unique_output_values(follow_up_outputs, "task_id"),
        "follow_up_stages": unique_output_values(follow_up_outputs, "stage"),
        "follow_up_statuses": unique_output_values(follow_up_outputs, "status"),
        "evidence": evidence,
    }


def unique_output_values(outputs: list[dict], field: str) -> list[str]:
    values = []
    for output in outputs:
        value = output[field]
        if value not in values:
            values.append(value)
    return values


def resolve_candidate_from_follow_ups(
    candidate: dict, follow_up_outputs: list[dict]
) -> tuple[str, str, str, str, list[str]]:
    if not follow_up_outputs:
        return (
            "unchanged",
            candidate["disposition"],
            "candidate",
            "no candidate-targeted follow-up output",
            [f"Original candidate disposition `{candidate['disposition']}`"],
        )
    signals = resolved_candidate_signals(follow_up_outputs)
    if signals:
        dispositions = {signal["disposition"] for signal in signals}
        if len(dispositions) > 1:
            return (
                "conflicting",
                candidate["disposition"],
                "orchestrator-follow-up",
                "candidate-targeted follow-ups produced conflicting disposition signals",
                [
                    item
                    for signal in signals
                    for item in signal["evidence"]
                ],
            )
        signal = signals[0]
        return (
            "resolved",
            signal["disposition"],
            "orchestrator-follow-up",
            signal["reason"],
            signal["evidence"],
        )
    evidence = [
        f"Follow-up task `{output['task_id']}` stage `{output['stage']}` status `{output['status']}`"
        for output in follow_up_outputs
    ]
    if any(output["status"] in {"ok", "degraded"} for output in follow_up_outputs):
        return (
            "unresolved",
            candidate["disposition"],
            "orchestrator-follow-up",
            "candidate-targeted follow-up ran without a refuted, parked, or dropped disposition",
            evidence,
        )
    return (
        "follow-up-unavailable",
        candidate["disposition"],
        "orchestrator-follow-up",
        "candidate-targeted follow-up did not produce usable model output",
        evidence,
    )


def resolved_candidate_signals(follow_up_outputs: list[dict]) -> list[dict]:
    signals = []
    for output in follow_up_outputs:
        evidence = follow_up_refuted_evidence(output)
        if evidence is not None:
            signals.append(
                {
                    "disposition": "refuted",
                    "reason": f"follow-up task `{output['task_id']}` refuted the candidate",
                    "evidence": evidence,
                }
            )
    for output in follow_up_outputs:
        evidence = follow_up_parked_evidence(output)
        if evidence is not None:
            signals.append(
                {
                    "disposition": "parked-follow-up",
                    "reason": f"follow-up task `{output['task_id']}` parked the candidate",
                    "evidence": evidence,
                }
            )
    for output in follow_up_outputs:
        evidence = follow_up_dropped_evidence(output)
        if evidence is not None:
            signals.append(
                {
                    "disposition": "dropped",
                    "reason": f"follow-up task `{output['task_id']}` dropped the candidate",
                    "evidence": evidence,
                }
            )
    return signals


def follow_up_refuted_evidence(output: dict) -> list[str] | None:
    if any(observation_is_refuted(observation) for observation in output["observations"]):
        return [f"Follow-up `{output['task_id']}` emitted a refuted/resolved observation"]
    for finding in output["summary_only_findings"]:
        if candidate_disposition_for_summary_finding(finding) == "refuted":
            return [f"Follow-up summary: {finding['reason']}"]
    return None


def follow_up_parked_evidence(output: dict) -> list[str] | None:
    if any(observation_is_parked(observation) for observation in output["observations"]):
        return [f"Follow-up `{output['task_id']}` emitted a parked observation"]
    for finding in output["summary_only_findings"]:
        if candidate_disposition_for_summary_finding(finding) == "parked-follow-up":
            return [f"Follow-up summary: {finding['reason']}"]
    return None


def follow_up_dropped_evidence(output: dict) -> list[str] | None:
    for finding in output["summary_only_findings"]:
        if candidate_disposition_for_summary_finding(finding) == "dropped":
            return [f"Follow-up summary: {finding['reason']}"]
    return None


def observation_is_refuted(observation: dict) -> bool:
    return observation.get("status") == "refuted"


def observation_is_parked(observation: dict) -> bool:
    return observation.get("status") == "parked"


def require_resolved_candidate_schema(record: dict) -> None:
    if record.get("schema") != "ub-review.resolved_candidate.v1":
        fail(f"resolved candidate has wrong schema: {record!r}")
    for field in [
        "candidate_id",
        "lane",
        "source",
        "original_status",
        "original_disposition",
        "resolved_status",
        "resolved_disposition",
        "resolution_source",
        "reason",
    ]:
        if not isinstance(record.get(field), str) or not record[field]:
            fail(f"resolved candidate {field} is missing or empty: {record!r}")
    if record["resolved_status"] not in {
        "unchanged",
        "resolved",
        "unresolved",
        "follow-up-unavailable",
        "conflicting",
    }:
        fail(f"resolved candidate status is unsupported: {record!r}")
    if record["resolved_disposition"] not in {
        "inline",
        "summary-only",
        "parked-follow-up",
        "refuted",
        "dropped",
    }:
        fail(f"resolved candidate disposition is unsupported: {record!r}")
    if record["resolution_source"] not in {"candidate", "orchestrator-follow-up"}:
        fail(f"resolved candidate source is unsupported: {record!r}")
    if record.get("source_artifacts") != [
        "review/candidates.json",
        "review/follow_up_results.json",
        "review/follow_up_outputs.json",
    ]:
        fail(f"resolved candidate source_artifacts are unsupported: {record!r}")
    for field in [
        "follow_up_task_ids",
        "follow_up_stages",
        "follow_up_statuses",
        "evidence",
    ]:
        values = record.get(field)
        if not isinstance(values, list) or not all(
            isinstance(value, str) and value for value in values
        ):
            fail(f"resolved candidate {field} is not a string array: {record!r}")


def require_final_compiler_input(
    root: pathlib.Path, review: dict, follow_up_evidence: dict
) -> None:
    final_input = load_json(root / "review/final_compiler_input.json")
    if not isinstance(final_input, dict):
        fail("review/final_compiler_input.json is not an object")
    if final_input.get("schema") != "ub-review.final_compiler_input.v1":
        fail(f"final compiler input has wrong schema: {final_input!r}")
    if final_input.get("phase") != "final":
        fail(f"final compiler input phase expected final: {final_input!r}")
    source_artifacts = final_input.get("source_artifacts")
    if not isinstance(source_artifacts, list):
        fail("final compiler input source_artifacts is not an array")
    for source in [
        "review/review.json",
        "review/follow_up_evidence.json",
        "review/proof_receipts.json",
        "review/receipt_routes.json",
        "review/final_orchestrator_plan.json",
    ]:
        if source not in source_artifacts:
            fail(f"final compiler input missing source artifact {source}")
    for field in [
        "model_lanes",
        "missing_or_failed_sensor_evidence",
        "missing_or_failed_model_evidence",
        "inline_comments",
        "summary_only_findings",
        "observations",
        "proof_receipts",
    ]:
        if not isinstance(final_input.get(field), list):
            fail(f"final compiler input {field} is not an array")
    for field in [
        "model_lanes",
        "missing_or_failed_sensor_evidence",
        "missing_or_failed_model_evidence",
        "inline_comments",
        "proof_receipts",
    ]:
        if final_input.get(field) != review.get(field, []):
            fail(f"final compiler input {field} does not match review.json")
    expected_summary = list(review.get("summary_only_findings", [])) + list(
        follow_up_evidence.get("summary_only_findings", [])
    )
    if final_input.get("summary_only_findings") != expected_summary:
        fail(
            "final compiler input summary_only_findings does not match "
            "review.json plus follow_up_evidence"
        )
    expected_observations = list(review.get("observations", [])) + list(
        follow_up_evidence.get("observations", [])
    )
    if final_input.get("observations") != expected_observations:
        fail(
            "final compiler input observations does not match "
            "review.json plus follow_up_evidence"
        )


def require_witness_artifacts(root: pathlib.Path, follow_up_evidence: dict) -> list[dict]:
    witnesses = load_json(root / "review/witnesses.json")
    if not isinstance(witnesses, list):
        fail("review/witnesses.json is not an array")
    registry = load_json(root / "review/witness_registry.json")
    if not isinstance(registry, dict):
        fail("review/witness_registry.json is not an object")
    lines = [
        line
        for line in read_text(root / "witnesses.ndjson").splitlines()
        if line.strip()
    ]
    if len(lines) != len(witnesses):
        fail("witnesses.ndjson line count does not match review/witnesses.json")
    for index, witness in enumerate(witnesses):
        if not isinstance(witness, dict):
            fail(f"witness {index + 1} is not an object: {witness!r}")
        try:
            parsed = json.loads(lines[index])
        except json.JSONDecodeError as error:
            fail(f"invalid witnesses.ndjson line {index + 1}: {error}")
        if parsed != witness:
            fail(f"witnesses.ndjson line {index + 1} does not match JSON artifact")
        require_witness_schema(witness)
    expected_follow_up = sum(
        len(follow_up_evidence[field])
        for field in [
            "inline_comments",
            "summary_only_findings",
            "observations",
            "proof_requests",
        ]
    )
    actual_follow_up = sum(
        1
        for witness in witnesses
        if isinstance(witness.get("source"), str)
        and witness["source"].startswith("follow-up-")
    )
    if actual_follow_up != expected_follow_up:
        fail("follow-up witness count does not match follow_up_evidence")
    require_witness_registry(registry, witnesses)
    return witnesses


def require_witness_registry(registry: dict, witnesses: list[dict]) -> None:
    if registry.get("schema") != "ub-review.witness_registry.v1":
        fail(f"witness registry has wrong schema: {registry!r}")
    expected = expected_witness_registry(witnesses)
    for field, expected_value in expected.items():
        if registry.get(field) != expected_value:
            fail(f"witness registry {field} does not match witnesses.json")


def expected_witness_registry(witnesses: list[dict]) -> dict:
    status_counts: dict[str, int] = {}
    kind_counts: dict[str, int] = {}
    source_counts: dict[str, int] = {}
    follow_up_status_counts: dict[str, int] = {}
    witness_ids_by_status: dict[str, list[str]] = {}
    follow_up_witness_ids_by_status: dict[str, list[str]] = {}
    follow_up_total = 0

    for witness in witnesses:
        status = witness["status"]
        kind = witness["kind"]
        source = witness["source"]
        status_counts[status] = status_counts.get(status, 0) + 1
        kind_counts[kind] = kind_counts.get(kind, 0) + 1
        source_counts[source] = source_counts.get(source, 0) + 1
        witness_ids_by_status.setdefault(status, []).append(witness["id"])
        if source.startswith("follow-up-"):
            follow_up_total += 1
            follow_up_status_counts[status] = follow_up_status_counts.get(status, 0) + 1
            follow_up_witness_ids_by_status.setdefault(status, []).append(witness["id"])

    return {
        "total": len(witnesses),
        "status_counts": dict(sorted(status_counts.items())),
        "kind_counts": dict(sorted(kind_counts.items())),
        "source_counts": dict(sorted(source_counts.items())),
        "follow_up_total": follow_up_total,
        "follow_up_status_counts": dict(sorted(follow_up_status_counts.items())),
        "witness_ids_by_status": dict(sorted(witness_ids_by_status.items())),
        "follow_up_witness_ids_by_status": dict(
            sorted(follow_up_witness_ids_by_status.items())
        ),
    }


def require_follow_up_result_metrics(metrics: dict, results: list[dict]) -> None:
    result_metrics = metrics.get("follow_up_results")
    if not isinstance(result_metrics, dict):
        fail("metrics.follow_up_results is missing")
    if result_metrics.get("total") != len(results):
        fail("metrics.follow_up_results.total does not match follow-up result artifacts")
    expected_status_counts: dict[str, int] = {}
    for result in results:
        status = result["status"]
        expected_status_counts[status] = expected_status_counts.get(status, 0) + 1
    if result_metrics.get("status_counts") != expected_status_counts:
        fail("metrics.follow_up_results.status_counts does not match follow-up result artifacts")
    expected_attempted = sum(
        1 for result in results if result["status"] in MODEL_CALL_ATTEMPTED_STATUSES
    )
    if result_metrics.get("calls_attempted") != expected_attempted:
        fail("metrics.follow_up_results.calls_attempted does not match follow-up result artifacts")


def require_follow_up_result_schema(
    root: pathlib.Path, result: dict, task: dict
) -> None:
    if result.get("schema") != "ub-review.follow_up_result.v1":
        fail(f"follow-up result has wrong schema: {result!r}")
    for field in [
        "task_id",
        "group_id",
        "stage",
        "disposition",
        "evidence_need",
        "packet_path",
        "model_lane",
        "status",
        "reason",
    ]:
        if not isinstance(result.get(field), str) or not result[field]:
            fail(f"follow-up result missing string field {field}: {result!r}")
    expected_packet_path = (
        f"questions/orchestrator-follow-up/{sanitize_artifact_name(task['id'])}.json"
    )
    expected_model_lane = f"orchestrator-follow-up-{sanitize_artifact_name(task['id'])}"
    if result["task_id"] != task["id"] or result["group_id"] != task["group_id"]:
        fail(f"follow-up result does not match task identity: {result!r}")
    if result["stage"] != task["stage"]:
        fail(f"follow-up result stage does not match task: {result!r}")
    if result["disposition"] != task["disposition"]:
        fail(f"follow-up result disposition does not match task: {result!r}")
    if result["evidence_need"] != task["evidence_need"]:
        fail(f"follow-up result evidence_need does not match task: {result!r}")
    for field in ["candidate_ids", "observation_group_ids"]:
        if not isinstance(result.get(field), list) or result[field] != task[field]:
            fail(f"follow-up result {field} does not match task: {result!r}")
    if result["packet_path"] != expected_packet_path:
        fail(f"follow-up result packet_path does not match task: {result!r}")
    if result["model_lane"] != expected_model_lane:
        fail(f"follow-up result model_lane does not match task: {result!r}")
    if result["status"] not in FOLLOW_UP_RESULT_STATUSES:
        fail(f"follow-up result has unsupported status: {result!r}")

    counts = result.get("output_counts")
    if not isinstance(counts, dict):
        fail(f"follow-up result output_counts is not an object: {result!r}")
    if sorted(counts) != sorted(FOLLOW_UP_OUTPUT_COUNT_FIELDS):
        fail(f"follow-up result output_counts has wrong fields: {result!r}")
    for field in FOLLOW_UP_OUTPUT_COUNT_FIELDS:
        if not isinstance(counts.get(field), int) or counts[field] < 0:
            fail(f"follow-up result output_counts field is invalid: {result!r}")

    if "duration_ms" in result and (
        not isinstance(result["duration_ms"], int) or result["duration_ms"] < 0
    ):
        fail(f"follow-up result duration_ms is invalid: {result!r}")
    if "http_status" in result and (
        not isinstance(result["http_status"], int)
        or result["http_status"] < 100
        or result["http_status"] > 599
    ):
        fail(f"follow-up result http_status is invalid: {result!r}")
    if "response_shape" in result and (
        not isinstance(result["response_shape"], str) or not result["response_shape"]
    ):
        fail(f"follow-up result response_shape is invalid: {result!r}")

    required_for_success = {
        "request_path",
        "response_path",
        "content_path",
        "stderr_path",
    }
    for field in required_for_success:
        if result["status"] in {"ok", "degraded"} and field not in result:
            fail(f"follow-up success result missing {field}: {result!r}")

    allowed_artifact_fields = required_for_success | {"normalized_content_path"}
    for field in allowed_artifact_fields:
        value = result.get(field)
        if value is None:
            continue
        if not isinstance(value, str) or not value:
            fail(f"follow-up result {field} is invalid: {result!r}")
        expected_prefix = f"review/model/{result['model_lane']}/"
        if not value.startswith(expected_prefix):
            fail(f"follow-up result {field} is outside its model lane: {result!r}")
        require_file(root / value)


def require_follow_up_output_schema(output: dict, result: dict) -> None:
    if output.get("schema") != "ub-review.follow_up_output.v1":
        fail(f"follow-up output has wrong schema: {output!r}")
    for field in [
        "task_id",
        "group_id",
        "stage",
        "disposition",
        "evidence_need",
        "model_lane",
        "status",
        "reason",
    ]:
        if not isinstance(output.get(field), str) or not output[field]:
            fail(f"follow-up output missing string field {field}: {output!r}")
    for field in [
        "task_id",
        "group_id",
        "stage",
        "disposition",
        "evidence_need",
        "candidate_ids",
        "observation_group_ids",
        "model_lane",
        "status",
        "reason",
    ]:
        if output[field] != result[field]:
            fail(f"follow-up output field {field} does not match result: {output!r}")
    inline_comments = output.get("inline_comments")
    if not isinstance(inline_comments, list):
        fail(f"follow-up output inline_comments is not an array: {output!r}")
    for comment in inline_comments:
        require_follow_up_inline_comment_schema(comment, output["model_lane"])
    summary_only = output.get("summary_only_findings")
    if not isinstance(summary_only, list):
        fail(f"follow-up output summary_only_findings is not an array: {output!r}")
    for finding in summary_only:
        require_follow_up_summary_only_schema(finding, output["model_lane"])
    observations = output.get("observations")
    if not isinstance(observations, list):
        fail(f"follow-up output observations is not an array: {output!r}")
    for observation in observations:
        require_observation_schema(observation)
    proof_requests = output.get("proof_requests")
    if not isinstance(proof_requests, list):
        fail(f"follow-up output proof_requests is not an array: {output!r}")
    for request in proof_requests:
        require_proof_request_schema(request)


def require_follow_up_inline_comment_schema(comment: dict, model_lane: str) -> None:
    if not isinstance(comment, dict):
        fail(f"follow-up inline comment is not an object: {comment!r}")
    for field in ["lane", "severity", "confidence", "path", "side", "body", "evidence"]:
        if not isinstance(comment.get(field), str) or not comment[field]:
            fail(f"follow-up inline comment missing string field {field}: {comment!r}")
    if comment["lane"] != model_lane:
        fail(f"follow-up inline comment lane does not match output lane: {comment!r}")
    if comment["side"] != "RIGHT":
        fail(f"follow-up inline comment side is not RIGHT: {comment!r}")
    if not isinstance(comment.get("line"), int) or comment["line"] <= 0:
        fail(f"follow-up inline comment line is invalid: {comment!r}")


def require_follow_up_summary_only_schema(finding: dict, model_lane: str) -> None:
    if not isinstance(finding, dict):
        fail(f"follow-up summary-only finding is not an object: {finding!r}")
    for field in ["lane", "severity", "confidence", "reason", "evidence"]:
        if not isinstance(finding.get(field), str) or not finding[field]:
            fail(f"follow-up summary-only finding missing string field {field}: {finding!r}")
    if finding["lane"] != model_lane:
        fail(f"follow-up summary-only finding lane does not match output lane: {finding!r}")


def require_witness_schema(witness: dict) -> None:
    if witness.get("schema") != "ub-review.witness.v1":
        fail(f"witness has wrong schema: {witness!r}")
    for field in ["id", "status", "kind", "source", "claim", "dedupe_key"]:
        if not isinstance(witness.get(field), str) or not witness[field]:
            fail(f"witness missing string field {field}: {witness!r}")
    if witness["status"] not in {
        "tool-confirmed",
        "type-confirmed",
        "needs-witness",
        "refuted",
        "parked",
    }:
        fail(f"witness has unsupported status: {witness!r}")
    evidence = witness.get("evidence")
    if not isinstance(evidence, list) or not evidence or not all(
        isinstance(item, str) and item for item in evidence
    ):
        fail(f"witness evidence is not a non-empty string array: {witness!r}")
    lane = witness.get("lane")
    if lane is not None and (not isinstance(lane, str) or not lane):
        fail(f"witness lane is not string/null: {witness!r}")
    path = witness.get("path")
    if path is not None and (
        not isinstance(path, str)
        or path.startswith(("/", "\\"))
        or ".." in pathlib.PurePosixPath(path).parts
    ):
        fail(f"witness path is not repo-relative: {witness!r}")
    line = witness.get("line")
    if line is not None and (not isinstance(line, int) or line <= 0):
        fail(f"witness line is invalid: {witness!r}")
    observation_id = witness.get("observation_id")
    if observation_id is not None and (
        not isinstance(observation_id, str) or not observation_id
    ):
        fail(f"witness observation_id is not string/null: {witness!r}")
    proof_receipt_id = witness.get("proof_receipt_id")
    if proof_receipt_id is not None and (
        not isinstance(proof_receipt_id, str) or not proof_receipt_id
    ):
        fail(f"witness proof_receipt_id is not string/null: {witness!r}")


def expected_follow_up_question_packet(task: dict) -> dict:
    return {
        "schema": "ub-review.follow_up_question_packet.v1",
        "id": task["id"],
        "task_id": task["id"],
        "group_id": task["group_id"],
        "stage": task["stage"],
        "stage_reason": task["stage_reason"],
        "evidence_need": task["evidence_need"],
        "disposition": task["disposition"],
        "candidate_ids": task["candidate_ids"],
        "observation_group_ids": task["observation_group_ids"],
        "routed_evidence": task["routed_evidence"],
        "question": task["question"],
        "status": task["status"],
        "source_artifact": "review/orchestrator_plan.json",
        "prompt": follow_up_question_prompt(task),
    }


def follow_up_question_prompt(task: dict) -> str:
    prompt = "Follow-up question task\n\n"
    prompt += f"- Task: `{task['id']}`\n"
    prompt += f"- Group: `{task['group_id']}`\n"
    prompt += f"- Stage: `{task['stage']}` - {task['stage_reason']}\n"
    prompt += f"- Evidence need: `{task['evidence_need']}`\n"
    prompt += f"- Disposition: `{task['disposition']}`\n"
    if task["candidate_ids"]:
        prompt += f"- Candidate ids: `{'`, `'.join(task['candidate_ids'])}`\n"
    if task["observation_group_ids"]:
        prompt += (
            f"- Observation group ids: `{'`, `'.join(task['observation_group_ids'])}`\n"
        )
    prompt += f"\nQuestion: {task['question']}\n\n"
    if not task["routed_evidence"]:
        prompt += "Routed evidence: none.\n\n"
    else:
        prompt += "Routed evidence:\n"
        for evidence in task["routed_evidence"]:
            prompt += (
                f"- `{evidence['id']}` kind=`{evidence['kind']}` "
                f"status=`{evidence['status']}` result=`{evidence['result']}` "
                f"artifact=`{evidence['artifact']}` reason={evidence['reason']}\n"
            )
        prompt += "\n"
    if task["stage"] == "tertiary":
        prompt += (
            "Stage instruction: use routed evidence to refine, refute, drop, "
            "or park the concern; do not repeat an already-resolved question.\n"
        )
    else:
        prompt += (
            "Stage instruction: identify the smallest remaining evidence or "
            "proof request needed before promotion.\n"
        )
    prompt += (
        "Return strict JSON with observations, summary_only_findings, "
        "failed_objections, and proof_requests. "
        f"Use question `{task['id']}` for observations. "
        "Do not emit candidate_findings or inline_comments. "
        "Do not post, mutate, or run shell commands.\n"
    )
    return prompt


def require_observation_summary_artifacts(
    root: pathlib.Path, observations: list[dict]
) -> None:
    unique = load_json(root / "review/unique_observations.json")
    merged = load_json(root / "review/merged_observations.json")
    dropped = load_json(root / "review/dropped_observations.json")
    if not isinstance(unique, list):
        fail("review/unique_observations.json is not an array")
    if not isinstance(merged, list):
        fail("review/merged_observations.json is not an array")
    if not isinstance(dropped, list):
        fail("review/dropped_observations.json is not an array")

    expected_unique, expected_merged, expected_dropped = expected_observation_groups(
        observations
    )
    if unique != expected_unique:
        fail("unique_observations.json does not match raw observation grouping")
    if merged != expected_merged:
        fail("merged_observations.json does not match raw observation grouping")
    if dropped != expected_dropped:
        fail("dropped_observations.json does not match raw observation grouping")

    observation_ids = {
        observation["id"]
        for observation in observations
        if isinstance(observation.get("id"), str)
    }
    unique_observation_ids: set[str] = set()
    group_ids: set[str] = set()
    duplicate_count = 0
    for group in unique:
        require_observation_group_schema(group)
        group_ids.add(group["id"])
        ids = group["observation_ids"]
        for observation_id in ids:
            if observation_id not in observation_ids:
                fail(f"unique observation group references unknown observation: {group!r}")
        unique_observation_ids.update(ids)
        if group["duplicate_count"] != max(0, len(ids) - 1):
            fail(f"unique observation group duplicate_count is wrong: {group!r}")
        duplicate_count += group["duplicate_count"]
    if unique_observation_ids != observation_ids:
        fail("unique_observations.json does not cover the raw observation ids exactly")

    dropped_ids = set()
    for record in merged:
        require_merged_observation_schema(record, group_ids, observation_ids)
    for record in dropped:
        require_dropped_observation_schema(record, group_ids, observation_ids)
        if record["observation_id"] in dropped_ids:
            fail(f"dropped observation is listed more than once: {record!r}")
        dropped_ids.add(record["observation_id"])
    if len(dropped) != duplicate_count:
        fail("dropped_observations.json count does not match grouped duplicate count")


def expected_observation_groups(
    observations: list[dict],
) -> tuple[list[dict], list[dict], list[dict]]:
    groups: list[dict] = []
    indexes: dict[str, int] = {}
    for observation in observations:
        key = observation.get("dedupe_key") or observation.get("fingerprint")
        if not isinstance(key, str) or not key:
            fail(f"observation cannot be grouped: {observation!r}")
        if key in indexes:
            merge_expected_observation_group(groups[indexes[key]], observation)
        else:
            group_id = expected_observation_group_id(len(groups), key)
            groups.append(
                {
                    "schema": "ub-review.observation_group.v1",
                    "id": group_id,
                    "dedupe_key": key,
                    "claim": observation["claim"],
                    "kind": observation["kind"],
                    "status": observation["status"],
                    "severity": observation["severity"],
                    "confidence": observation["confidence"],
                    "path": observation.get("path"),
                    "line": observation.get("line"),
                    "evidence": observation.get("evidence", [])[:3],
                    "lanes": [observation["lane"]],
                    "sources": [observation["source"]],
                    "observation_ids": [observation["id"]],
                    "duplicate_count": 0,
                }
            )
            indexes[key] = len(groups) - 1

    merged = []
    dropped = []
    by_id = {observation["id"]: observation for observation in observations}
    for group in groups:
        ids = group["observation_ids"]
        if len(ids) <= 1:
            continue
        duplicate_ids = ids[1:]
        merged.append(
            {
                "schema": "ub-review.merged_observation.v1",
                "group_id": group["id"],
                "dedupe_key": group["dedupe_key"],
                "kept_observation_id": ids[0],
                "merged_observation_ids": duplicate_ids,
                "lanes": group["lanes"],
                "reason": "merged_duplicate_dedupe_key",
            }
        )
        for observation_id in duplicate_ids:
            dropped.append(
                {
                    "schema": "ub-review.dropped_observation.v1",
                    "observation_id": observation_id,
                    "group_id": group["id"],
                    "dedupe_key": group["dedupe_key"],
                    "lane": by_id[observation_id]["lane"],
                    "reason": "merged_into_unique_observation",
                }
            )
    return groups, merged, dropped


def merge_expected_observation_group(group: dict, observation: dict) -> None:
    if severity_rank(observation["severity"]) > severity_rank(group["severity"]):
        group["severity"] = observation["severity"]
    if confidence_rank(observation["confidence"]) > confidence_rank(group["confidence"]):
        group["confidence"] = observation["confidence"]
    if observation_status_rank(observation["status"]) > observation_status_rank(group["status"]):
        group["status"] = observation["status"]
    if group.get("path") is None:
        group["path"] = observation.get("path")
    if group.get("line") is None:
        group["line"] = observation.get("line")
    if observation["lane"] not in group["lanes"]:
        group["lanes"].append(observation["lane"])
    if observation["source"] not in group["sources"]:
        group["sources"].append(observation["source"])
    group["observation_ids"].append(observation["id"])
    group["duplicate_count"] = max(0, len(group["observation_ids"]) - 1)
    for evidence in observation.get("evidence", []):
        if len(group["evidence"]) >= 3:
            break
        if evidence not in group["evidence"]:
            group["evidence"].append(evidence)


def expected_observation_group_id(index: int, dedupe_key: str) -> str:
    digest = hashlib.sha256(dedupe_key.encode("utf-8")).hexdigest()
    return f"obsgrp-{index:04d}-{digest[:12]}"


def severity_rank(value: str) -> int:
    return {
        "blocker": 4,
        "high": 3,
        "medium": 2,
        "low": 1,
    }.get(value, 0)


def confidence_rank(value: str) -> int:
    return {
        "high": 3,
        "medium-high": 2,
        "medium": 1,
        "low": 0,
    }.get(value, 0)


def observation_status_rank(value: str) -> int:
    return {
        "refuted": 7,
        "confirmed": 6,
        "parked": 5,
        "demoted": 4,
        "open": 3,
        "covered": 2,
        "duplicate": 1,
    }.get(value, 0)


def require_observation_group_schema(group: dict) -> None:
    if group.get("schema") != "ub-review.observation_group.v1":
        fail(f"observation group has wrong schema: {group!r}")
    for field in [
        "id",
        "dedupe_key",
        "claim",
        "kind",
        "status",
        "severity",
        "confidence",
    ]:
        if not isinstance(group.get(field), str) or not group[field]:
            fail(f"observation group missing string field {field}: {group!r}")
    for field in ["evidence", "lanes", "sources", "observation_ids"]:
        values = group.get(field)
        if not isinstance(values, list) or not all(
            isinstance(item, str) and item for item in values
        ):
            fail(f"observation group {field} is not a non-empty string array: {group!r}")
    if not isinstance(group.get("duplicate_count"), int) or group["duplicate_count"] < 0:
        fail(f"observation group duplicate_count is invalid: {group!r}")
    path = group.get("path")
    if path is not None and (not isinstance(path, str) or path.startswith(("/", "\\"))):
        fail(f"observation group path is not repo-relative: {group!r}")
    line = group.get("line")
    if line is not None and (not isinstance(line, int) or line <= 0):
        fail(f"observation group line is invalid: {group!r}")
    require_sibling_completeness_observation_policy(group)


def require_merged_observation_schema(
    record: dict, group_ids: set[str], observation_ids: set[str]
) -> None:
    if record.get("schema") != "ub-review.merged_observation.v1":
        fail(f"merged observation has wrong schema: {record!r}")
    for field in ["group_id", "dedupe_key", "kept_observation_id", "reason"]:
        if not isinstance(record.get(field), str) or not record[field]:
            fail(f"merged observation missing string field {field}: {record!r}")
    if record["group_id"] not in group_ids:
        fail(f"merged observation references unknown group: {record!r}")
    if record["kept_observation_id"] not in observation_ids:
        fail(f"merged observation references unknown kept observation: {record!r}")
    for field in ["merged_observation_ids", "lanes"]:
        values = record.get(field)
        if not isinstance(values, list) or not all(
            isinstance(item, str) and item for item in values
        ):
            fail(f"merged observation {field} is not a non-empty string array: {record!r}")
    for observation_id in record["merged_observation_ids"]:
        if observation_id not in observation_ids:
            fail(f"merged observation references unknown duplicate: {record!r}")


def require_dropped_observation_schema(
    record: dict, group_ids: set[str], observation_ids: set[str]
) -> None:
    if record.get("schema") != "ub-review.dropped_observation.v1":
        fail(f"dropped observation has wrong schema: {record!r}")
    for field in ["observation_id", "group_id", "dedupe_key", "lane", "reason"]:
        if not isinstance(record.get(field), str) or not record[field]:
            fail(f"dropped observation missing string field {field}: {record!r}")
    if record["group_id"] not in group_ids:
        fail(f"dropped observation references unknown group: {record!r}")
    if record["observation_id"] not in observation_ids:
        fail(f"dropped observation references unknown observation: {record!r}")


def require_observation_schema(observation: dict) -> None:
    if observation.get("schema") != "ub-review.observation.v1":
        fail(f"observation has wrong schema: {observation!r}")
    for field in [
        "id",
        "lane",
        "question",
        "claim",
        "kind",
        "status",
        "severity",
        "confidence",
        "fingerprint",
        "dedupe_key",
        "source",
    ]:
        if not isinstance(observation.get(field), str) or not observation[field]:
            fail(f"observation missing string field {field}: {observation!r}")
    if observation["kind"] not in {
        "bug",
        "verification-question",
        "missing-evidence",
        "test-gap",
        "source-route-gap",
        "residual-risk",
        "security-risk",
        "false-premise",
        "parked-follow-up",
        "resolved-check",
    }:
        fail(f"observation has unsupported kind: {observation!r}")
    if observation["status"] not in {
        "open",
        "covered",
        "confirmed",
        "refuted",
        "demoted",
        "parked",
        "duplicate",
    }:
        fail(f"observation has unsupported status: {observation!r}")
    if observation["severity"] not in {"blocker", "high", "medium", "low"}:
        fail(f"observation has unsupported severity: {observation!r}")
    if observation["confidence"] not in {"high", "medium-high", "medium", "low"}:
        fail(f"observation has unsupported confidence: {observation!r}")
    if not re.fullmatch(r"[0-9a-f]{64}", observation["fingerprint"]):
        fail(f"observation fingerprint is not a SHA-256 hex digest: {observation!r}")
    evidence = observation.get("evidence")
    if not isinstance(evidence, list) or not all(isinstance(item, str) for item in evidence):
        fail(f"observation evidence is not a string array: {observation!r}")
    path = observation.get("path")
    if path is not None and (not isinstance(path, str) or path.startswith(("/", "\\"))):
        fail(f"observation path is not repo-relative: {observation!r}")
    line = observation.get("line")
    if line is not None and (not isinstance(line, int) or line <= 0):
        fail(f"observation line is invalid: {observation!r}")
    require_sibling_completeness_observation_policy(observation)


def require_sibling_completeness_observation_policy(observation: dict) -> None:
    text = "\n".join(
        [
            str(observation.get("claim", "")),
            "\n".join(item for item in observation.get("evidence", []) if isinstance(item, str)),
        ]
    )
    if observation.get("dedupe_key") == SIBLING_COMPLETENESS_OVERCLAIM_DEDUPE_KEY:
        if observation.get("kind") != "source-route-gap" or observation.get("status") != "open":
            fail(
                "sibling completeness guard observation is not an open source-route gap: "
                f"{observation!r}"
            )
        return
    if not is_unsupported_sibling_completeness_overclaim(text):
        return
    if observation.get("status") in {"refuted", "covered", "confirmed"} or observation.get(
        "kind"
    ) in {"resolved-check", "false-premise"}:
        fail(
            "resolved/refuted observation contains unsupported sibling completeness claim: "
            f"{observation!r}"
        )


def require_candidate_schema(candidate: dict) -> None:
    if candidate.get("schema") != "ub-review.candidate.v1":
        fail(f"candidate has wrong schema: {candidate!r}")
    for field in [
        "id",
        "lane",
        "source",
        "status",
        "disposition",
        "severity",
        "confidence",
        "claim",
        "evidence",
    ]:
        if not isinstance(candidate.get(field), str) or not candidate[field]:
            fail(f"candidate missing string field {field}: {candidate!r}")
    if candidate["source"] not in {"inline-comment", "summary-only-finding"}:
        fail(f"candidate has unsupported source: {candidate!r}")
    if candidate["status"] not in {"accepted-inline", "summary-only"}:
        fail(f"candidate has unsupported status: {candidate!r}")
    if candidate["disposition"] not in {
        "inline",
        "summary-only",
        "parked-follow-up",
        "refuted",
        "dropped",
    }:
        fail(f"candidate has unsupported disposition: {candidate!r}")
    if candidate["severity"] not in {"blocker", "high", "medium", "low"}:
        fail(f"candidate has unsupported severity: {candidate!r}")
    if candidate["confidence"] not in {"high", "medium-high", "medium", "low"}:
        fail(f"candidate has unsupported confidence: {candidate!r}")
    path = candidate.get("path")
    line = candidate.get("line")
    side = candidate.get("side")
    if candidate["status"] == "accepted-inline":
        if candidate["disposition"] != "inline":
            fail(f"inline candidate disposition is not inline: {candidate!r}")
        if (
            not isinstance(path, str)
            or not path
            or path.startswith(("/", "\\"))
            or ".." in pathlib.PurePosixPath(path).parts
        ):
            fail(f"inline candidate path is not repo-relative: {candidate!r}")
        if not isinstance(line, int) or line <= 0:
            fail(f"inline candidate line is invalid: {candidate!r}")
        if side != "RIGHT":
            fail(f"inline candidate side is not RIGHT: {candidate!r}")
    else:
        if candidate["disposition"] == "inline":
            fail(f"summary-only candidate disposition is inline: {candidate!r}")
        if path is not None or line is not None or side is not None:
            fail(f"summary-only candidate should not have inline fields: {candidate!r}")


def require_orchestrator_plan_schema(plan: dict) -> None:
    if plan.get("schema") != "ub-review.orchestrator_plan.v1":
        fail(f"orchestrator plan has wrong schema: {plan!r}")
    if not isinstance(plan.get("candidates"), int) or plan["candidates"] < 0:
        fail(f"orchestrator plan candidates is invalid: {plan!r}")
    if not isinstance(plan.get("observations"), int) or plan["observations"] < 0:
        fail(f"orchestrator plan observations is invalid: {plan!r}")
    groups = plan.get("evidence_groups")
    observation_groups = plan.get("observation_groups")
    tasks = plan.get("follow_up_tasks")
    if not isinstance(groups, list):
        fail("orchestrator plan evidence_groups is not an array")
    if not isinstance(observation_groups, list):
        fail("orchestrator plan observation_groups is not an array")
    if not isinstance(tasks, list):
        fail("orchestrator plan follow_up_tasks is not an array")
    group_ids = set()
    for group in groups:
        require_orchestrator_group_schema(group)
        if group["id"] in group_ids:
            fail(f"orchestrator group id is duplicated: {group!r}")
        group_ids.add(group["id"])
    for group in observation_groups:
        require_orchestrator_observation_group_schema(group)
        if group["id"] in group_ids:
            fail(f"orchestrator observation group id is duplicated: {group!r}")
        group_ids.add(group["id"])
    for task in tasks:
        require_follow_up_task_schema(task, group_ids)


def require_orchestrator_group_schema(group: dict) -> None:
    if group.get("schema") != "ub-review.orchestrator_evidence_group.v1":
        fail(f"orchestrator group has wrong schema: {group!r}")
    for field in ["id", "evidence_need", "disposition", "reason"]:
        if not isinstance(group.get(field), str) or not group[field]:
            fail(f"orchestrator group missing string field {field}: {group!r}")
    for field in ["candidate_ids", "lanes"]:
        values = group.get(field)
        if not isinstance(values, list) or not all(isinstance(item, str) and item for item in values):
            fail(f"orchestrator group {field} is not a string array: {group!r}")
    routed_evidence = group.get("routed_evidence")
    if not isinstance(routed_evidence, list):
        fail(f"orchestrator group routed_evidence is not an array: {group!r}")
    for evidence in routed_evidence:
        require_orchestrator_routed_evidence_schema(evidence)
    if not isinstance(group.get("duplicate_count"), int) or group["duplicate_count"] < 0:
        fail(f"orchestrator group duplicate_count is invalid: {group!r}")


def require_orchestrator_observation_group_schema(group: dict) -> None:
    if group.get("schema") != "ub-review.orchestrator_observation_group.v1":
        fail(f"orchestrator observation group has wrong schema: {group!r}")
    for field in [
        "id",
        "observation_group_id",
        "dedupe_key",
        "evidence_need",
        "claim",
        "kind",
        "status",
        "reason",
    ]:
        if not isinstance(group.get(field), str) or not group[field]:
            fail(f"orchestrator observation group missing string field {field}: {group!r}")
    for field in ["lanes", "sources", "observation_ids"]:
        values = group.get(field)
        if not isinstance(values, list) or not all(
            isinstance(item, str) and item for item in values
        ):
            fail(f"orchestrator observation group {field} is not a string array: {group!r}")
    routed_evidence = group.get("routed_evidence")
    if not isinstance(routed_evidence, list):
        fail(f"orchestrator observation group routed_evidence is not an array: {group!r}")
    for evidence in routed_evidence:
        require_orchestrator_routed_evidence_schema(evidence)
    if not isinstance(group.get("duplicate_count"), int) or group["duplicate_count"] < 0:
        fail(f"orchestrator observation group duplicate_count is invalid: {group!r}")


def require_follow_up_task_schema(task: dict, group_ids: set[str]) -> None:
    if task.get("schema") != "ub-review.follow_up_question.v1":
        fail(f"follow-up task has wrong schema: {task!r}")
    for field in [
        "id",
        "group_id",
        "stage",
        "stage_reason",
        "evidence_need",
        "disposition",
        "question",
        "status",
        "reason",
    ]:
        if not isinstance(task.get(field), str) or not task[field]:
            fail(f"follow-up task missing string field {field}: {task!r}")
    if task["group_id"] not in group_ids:
        fail(f"follow-up task references unknown group: {task!r}")
    if task["stage"] not in {"secondary", "tertiary"}:
        fail(f"follow-up task has unsupported stage: {task!r}")
    if task["status"] != "planned":
        fail(f"follow-up task has unsupported status: {task!r}")
    candidate_ids = task.get("candidate_ids")
    if not isinstance(candidate_ids, list) or not all(
        isinstance(item, str) and item for item in candidate_ids
    ):
        fail(f"follow-up task candidate_ids is not a string array: {task!r}")
    observation_group_ids = task.get("observation_group_ids")
    if not isinstance(observation_group_ids, list) or not all(
        isinstance(item, str) and item for item in observation_group_ids
    ):
        fail(f"follow-up task observation_group_ids is not a string array: {task!r}")
    routed_evidence = task.get("routed_evidence")
    if not isinstance(routed_evidence, list):
        fail(f"follow-up task routed_evidence is not an array: {task!r}")
    for evidence in routed_evidence:
        require_orchestrator_routed_evidence_schema(evidence)


def require_follow_up_question_packet_schema(packet: dict) -> None:
    if packet.get("schema") != "ub-review.follow_up_question_packet.v1":
        fail(f"follow-up question packet has wrong schema: {packet!r}")
    for field in [
        "id",
        "task_id",
        "group_id",
        "stage",
        "stage_reason",
        "evidence_need",
        "disposition",
        "question",
        "status",
        "source_artifact",
        "prompt",
    ]:
        if not isinstance(packet.get(field), str) or not packet[field]:
            fail(f"follow-up question packet missing string field {field}: {packet!r}")
    if packet["source_artifact"] != "review/orchestrator_plan.json":
        fail(f"follow-up question packet has unsupported source artifact: {packet!r}")
    if packet["stage"] not in {"secondary", "tertiary"}:
        fail(f"follow-up question packet has unsupported stage: {packet!r}")
    for field in ["candidate_ids", "observation_group_ids"]:
        values = packet.get(field)
        if not isinstance(values, list) or not all(
            isinstance(item, str) and item for item in values
        ):
            fail(f"follow-up question packet {field} is not a string array: {packet!r}")
    routed_evidence = packet.get("routed_evidence")
    if not isinstance(routed_evidence, list):
        fail(f"follow-up question packet routed_evidence is not an array: {packet!r}")
    for evidence in routed_evidence:
        require_orchestrator_routed_evidence_schema(evidence)


def require_orchestrator_routed_evidence_schema(evidence: dict) -> None:
    if evidence.get("schema") != "ub-review.orchestrator_routed_evidence.v1":
        fail(f"routed evidence has wrong schema: {evidence!r}")
    for field in ["id", "kind", "artifact", "status", "result", "reason"]:
        if not isinstance(evidence.get(field), str) or not evidence[field]:
            fail(f"routed evidence missing string field {field}: {evidence!r}")
    if evidence["kind"] not in {"proof-receipt", "resource-lease"}:
        fail(f"routed evidence has unsupported kind: {evidence!r}")
    if evidence["artifact"] not in {"review/proof_receipts.json", "review/resource_leases.json"}:
        fail(f"routed evidence has unsupported artifact path: {evidence!r}")


def require_proof_request_schema(request: dict) -> None:
    if request.get("schema") != "ub-review.proof_request.v1":
        fail(f"proof request has wrong schema: {request!r}")
    for field in ["id", "lane", "command", "reason", "cost", "status"]:
        if not isinstance(request.get(field), str) or not request[field]:
            fail(f"proof request missing string field {field}: {request!r}")
    requested_by = request.get("requested_by")
    if not isinstance(requested_by, list) or not all(
        isinstance(item, str) and item for item in requested_by
    ):
        fail(f"proof request requested_by is not a non-empty string array: {request!r}")
    if request["lane"] not in requested_by:
        fail(f"proof request lane is not listed in requested_by: {request!r}")
    if request["cost"] not in {"focused-test", "focused-build", "manual"}:
        fail(f"proof request has unsupported cost: {request!r}")
    timeout = request.get("timeout_sec")
    if not isinstance(timeout, int) or timeout <= 0 or timeout > 900:
        fail(f"proof request timeout_sec is invalid: {request!r}")
    if not isinstance(request.get("required"), bool):
        fail(f"proof request required is not boolean: {request!r}")
    if request["status"] not in {"requested", "unsupported", "invalid"}:
        fail(f"proof request has unsupported status: {request!r}")


def require_proof_receipt_schema(root: pathlib.Path, receipt: dict) -> None:
    if receipt.get("schema") != "ub-review.proof_receipt.v1":
        fail(f"proof receipt has wrong schema: {receipt!r}")
    for field in ["id", "kind", "base", "head", "test_patch_mode", "result", "reason"]:
        if not isinstance(receipt.get(field), str) or not receipt[field]:
            fail(f"proof receipt missing string field {field}: {receipt!r}")
    if receipt["kind"] not in {"focused-head", "focused-red-green", "focused-build"}:
        fail(f"proof receipt has unsupported kind: {receipt!r}")
    if receipt["test_patch_mode"] not in {"head-only", "base-plus-tests"}:
        fail(f"proof receipt has unsupported test_patch_mode: {receipt!r}")
    if receipt["result"] not in {
        "head_passed",
        "head_failed",
        "timed_out",
        "skipped_budget",
        "skipped_profile",
        "discriminating",
        "non_discriminating",
        "base_patch_failed",
    }:
        fail(f"proof receipt has unsupported result: {receipt!r}")
    for field in ["requested_by", "request_ids"]:
        values = receipt.get(field)
        if not isinstance(values, list) or not all(
            isinstance(item, str) and item for item in values
        ):
            fail(f"proof receipt {field} is not a string array: {receipt!r}")
    commands = receipt.get("commands")
    if not isinstance(commands, list) or not commands:
        fail(f"proof receipt commands is not a non-empty array: {receipt!r}")
    for command in commands:
        require_proof_command_receipt_schema(root, receipt["id"], command)


def require_proof_command_receipt_schema(
    root: pathlib.Path, receipt_id: str, command: dict
) -> None:
    for field in ["side", "command", "status", "stdout", "stderr", "reason"]:
        if not isinstance(command.get(field), str) or not command[field]:
            fail(f"proof command receipt missing string field {field}: {command!r}")
    env = command.get("env")
    if not isinstance(env, dict) or not all(
        isinstance(key, str) and key and isinstance(value, str)
        for key, value in env.items()
    ):
        fail(f"proof command receipt env is not a string map: {command!r}")
    if command["side"] not in {"head", "base-plus-tests"}:
        fail(f"proof command receipt has unsupported side: {command!r}")
    if command["status"] not in {"passed", "failed", "timed_out", "skipped"}:
        fail(f"proof command receipt has unsupported status: {command!r}")
    if command.get("exit_code") is not None and not isinstance(command.get("exit_code"), int):
        fail(f"proof command receipt exit_code is invalid: {command!r}")
    if not isinstance(command.get("timed_out"), bool):
        fail(f"proof command receipt timed_out is not boolean: {command!r}")
    timeout = command.get("timeout_sec")
    if not isinstance(timeout, int) or timeout < 0 or timeout > 900:
        fail(f"proof command receipt timeout_sec is invalid: {command!r}")
    duration_ms = command.get("duration_ms")
    if not isinstance(duration_ms, int) or duration_ms < 0:
        fail(f"proof command receipt duration_ms is invalid: {command!r}")
    for field in ["stdout", "stderr"]:
        rel = command[field]
        path = pathlib.PurePosixPath(rel.replace("\\", "/"))
        if path.is_absolute() or ".." in path.parts:
            fail(f"proof command {field} path is not repo-relative: {command!r}")
        expected = pathlib.PurePosixPath(
            f"proof/{sanitize_artifact_name(receipt_id)}/{command['side']}/{field}.txt"
        )
        if path != expected:
            fail(
                f"proof command {field} path expected {expected}, got {path}: {command!r}"
            )
        if not (root / path).is_file():
            fail(f"proof command {field} artifact missing: {path}")


def require_resource_lease_schema(lease: dict) -> None:
    if lease.get("schema") != "ub-review.resource_lease.v1":
        fail(f"resource lease has wrong schema: {lease!r}")
    for field in ["id", "kind", "consumer", "status", "reason"]:
        if not isinstance(lease.get(field), str) or not lease[field]:
            fail(f"resource lease missing string field {field}: {lease!r}")
    if lease["kind"] not in {"focused-test", "focused-build"}:
        fail(f"resource lease has unsupported kind: {lease!r}")
    if lease["status"] not in {"granted", "exhausted", "skipped_profile"}:
        fail(f"resource lease has unsupported status: {lease!r}")
    for field in ["cpu", "memory_mb", "disk_mb", "timeout_sec"]:
        value = lease.get(field)
        if not isinstance(value, int) or isinstance(value, bool) or value < 0:
            fail(f"resource lease {field} is not a non-negative integer: {lease!r}")
    for field in ["network", "scratch"]:
        if lease.get(field) not in {True, False}:
            fail(f"resource lease {field} is not bool: {lease!r}")
    if lease.get("worktree") is not None and not isinstance(lease.get("worktree"), str):
        fail(f"resource lease worktree is not string/null: {lease!r}")
    if lease.get("command") is not None and not isinstance(lease.get("command"), str):
        fail(f"resource lease command is not string/null: {lease!r}")


def require_proof_request_group_schema(group: dict) -> None:
    if group.get("schema") != "ub-review.proof_request_group.v1":
        fail(f"proof request group has wrong schema: {group!r}")
    for field in ["id", "command", "cost", "status"]:
        if not isinstance(group.get(field), str) or not group[field]:
            fail(f"proof request group missing string field {field}: {group!r}")
    timeout = group.get("timeout_sec")
    if not isinstance(timeout, int) or timeout <= 0 or timeout > 900:
        fail(f"proof request group timeout_sec is invalid: {group!r}")
    if not isinstance(group.get("required"), bool):
        fail(f"proof request group required is not boolean: {group!r}")
    if group["status"] not in {"requested", "unsupported", "invalid"}:
        fail(f"proof request group has unsupported status: {group!r}")
    if group["cost"] not in {"focused-test", "focused-build", "manual"}:
        fail(f"proof request group has unsupported cost: {group!r}")
    duplicate_count = group.get("duplicate_count")
    if not isinstance(duplicate_count, int) or duplicate_count <= 0:
        fail(f"proof request group duplicate_count is invalid: {group!r}")
    for field in ["requested_by", "request_ids", "reasons"]:
        values = group.get(field)
        if not isinstance(values, list) or not all(
            isinstance(item, str) and item for item in values
        ):
            fail(f"proof request group {field} is not a non-empty string array: {group!r}")


def sanitize_artifact_name(value: str) -> str:
    sanitized = "".join(
        ch if (ch.isascii() and ch.isalnum()) or ch in "-_" else "-" for ch in value
    )
    if len(sanitized) <= ARTIFACT_NAME_MAX_CHARS:
        return sanitized
    digest = hashlib.sha256(value.encode("utf-8")).hexdigest()
    prefix_len = ARTIFACT_NAME_MAX_CHARS - ARTIFACT_NAME_HASH_CHARS - 1
    return f"{sanitized[:prefix_len]}-{digest[:ARTIFACT_NAME_HASH_CHARS]}"


def require_sensor_receipts(root: pathlib.Path) -> None:
    for sensor in SENSORS:
        receipt = load_json(root / "sensors" / sensor / "ub-review-sensor-status.json")
        if receipt.get("sensor") != sensor:
            fail(f"{sensor} receipt has wrong sensor id {receipt.get('sensor')!r}")
        if receipt.get("status") not in {"ok", "missing", "skipped", "failed", "timed_out"}:
            fail(f"{sensor} receipt has unsupported status {receipt.get('status')!r}")
        if "reason" not in receipt:
            fail(f"{sensor} receipt missing reason")


def require_tool_registry_artifacts(root: pathlib.Path) -> None:
    resolved_tools = load_json(root / "resolved-tools.json")
    review_resolved_tools = load_json(root / "review/resolved-tools.json")
    if resolved_tools != review_resolved_tools:
        fail("resolved-tools.json does not match review/resolved-tools.json")
    if resolved_tools.get("schema") != "ub-review.resolved_tools.v1":
        fail("resolved-tools.json has wrong schema")

    tool_status = load_json(root / "tool-status.json")
    review_tool_status = load_json(root / "review/tool-status.json")
    if tool_status != review_tool_status:
        fail("tool-status.json does not match review/tool-status.json")
    if tool_status.get("schema") != "ub-review.tool_status.v1":
        fail("tool-status.json has wrong schema")

    runtime_profile = resolved_tools.get("runtime_profile")
    if not isinstance(runtime_profile, str) or not runtime_profile:
        fail("resolved-tools.json runtime_profile is invalid")
    if tool_status.get("runtime_profile") != runtime_profile:
        fail("tool-status.json runtime_profile does not match resolved-tools.json")

    resolved_by_id = require_tool_entries(resolved_tools, "resolved-tools.json")
    status_by_id = require_tool_entries(tool_status, "tool-status.json")
    missing_status = sorted(set(resolved_by_id) - set(status_by_id))
    if missing_status:
        fail(f"tool-status.json missing resolved tools: {', '.join(missing_status)}")
    for tool_id, resolved_entry in resolved_by_id.items():
        status_entry = status_by_id.get(tool_id)
        if status_entry is None:
            continue
        require_tool_status_matches_resolved(tool_id, resolved_entry, status_entry)

    for sensor in SENSORS:
        if sensor not in resolved_by_id:
            fail(f"resolved-tools.json missing core tool {sensor}")
        if sensor not in status_by_id:
            fail(f"tool-status.json missing core tool {sensor}")
        status_entry = status_by_id[sensor]
        receipt = load_json(root / "sensors" / sensor / "ub-review-sensor-status.json")
        if status_entry.get("status") != receipt.get("status"):
            fail(f"tool-status.json status for {sensor} does not match sensor receipt")
        if status_entry.get("reason") != receipt.get("reason"):
            fail(f"tool-status.json reason for {sensor} does not match sensor receipt")
        expected_status_path = f"sensors/{sensor}/ub-review-sensor-status.json"
        paths = status_entry.get("artifact_paths", [])
        if expected_status_path not in paths:
            fail(f"tool-status.json {sensor} missing status artifact path")

    if "coverage" in status_by_id:
        require_coverage_status_artifact(root, status_by_id["coverage"])
    require_tool_gate_outcome_artifacts(root, resolved_by_id, status_by_id, runtime_profile)


def require_tool_gate_outcome_artifacts(
    root: pathlib.Path,
    resolved_by_id: dict[str, dict],
    status_by_id: dict[str, dict],
    runtime_profile: str,
) -> None:
    outcomes = load_json(root / "tool-gate-outcomes.json")
    review_outcomes = load_json(root / "review/tool-gate-outcomes.json")
    if outcomes != review_outcomes:
        fail("tool-gate-outcomes.json does not match review/tool-gate-outcomes.json")
    if outcomes.get("schema") != "ub-review.tool_gate_outcomes.v1":
        fail("tool-gate-outcomes.json has wrong schema")
    if outcomes.get("runtime_profile") != runtime_profile:
        fail("tool-gate-outcomes.json runtime_profile does not match resolved-tools.json")
    outcome_entries = outcomes.get("outcomes")
    if not isinstance(outcome_entries, list):
        fail("tool-gate-outcomes.json outcomes is not an array")
    gated_tools = {
        tool_id: entry
        for tool_id, entry in resolved_by_id.items()
        if entry.get("gate") is not None
    }
    if len(outcome_entries) != len(gated_tools):
        fail("tool-gate-outcomes.json outcome count does not match configured tool gates")
    by_tool: dict[str, dict] = {}
    for entry in outcome_entries:
        require_tool_gate_outcome_entry(entry)
        tool = entry["tool"]
        if tool in by_tool:
            fail(f"duplicate tool gate outcome for {tool}")
        by_tool[tool] = entry
    missing = sorted(set(gated_tools) - set(by_tool))
    extra = sorted(set(by_tool) - set(gated_tools))
    if missing:
        fail(f"tool-gate-outcomes.json missing gated tools: {', '.join(missing)}")
    if extra:
        fail(f"tool-gate-outcomes.json has ungated tools: {', '.join(extra)}")
    for tool_id, resolved_entry in gated_tools.items():
        outcome = by_tool[tool_id]
        status_entry = status_by_id.get(tool_id)
        if status_entry is None:
            fail(f"tool-gate-outcomes.json references missing status tool {tool_id}")
        if outcome.get("policy") != resolved_entry.get("gate"):
            fail(f"tool-gate-outcomes.json policy for {tool_id} does not match resolved-tools.json")
        if outcome.get("required") != status_entry.get("required"):
            fail(f"tool-gate-outcomes.json required for {tool_id} does not match tool-status.json")
        if outcome.get("planned_run") != status_entry.get("planned_run"):
            fail(f"tool-gate-outcomes.json planned_run for {tool_id} does not match tool-status.json")
        if outcome.get("sensor_status") != status_entry.get("status"):
            fail(f"tool-gate-outcomes.json sensor_status for {tool_id} does not match tool-status.json")
        if outcome.get("sensor_reason") != status_entry.get("reason"):
            fail(f"tool-gate-outcomes.json sensor_reason for {tool_id} does not match tool-status.json")
        expected_receipt = f"sensors/{tool_id}/ub-review-sensor-status.json"
        if outcome.get("sensor_receipt_path") != expected_receipt:
            fail(f"tool-gate-outcomes.json sensor receipt path for {tool_id} is invalid")
        source_artifacts = outcome.get("source_artifacts")
        if expected_receipt not in source_artifacts:
            fail(f"tool-gate-outcomes.json source_artifacts missing sensor receipt for {tool_id}")
        if "tool-status.json" not in source_artifacts:
            fail(f"tool-gate-outcomes.json source_artifacts missing tool-status for {tool_id}")
        require_tool_gate_outcome_consistency(outcome)

    lines = [line for line in read_text(root / "tool_gate_outcomes.ndjson").splitlines() if line.strip()]
    if len(lines) != len(outcome_entries):
        fail("tool_gate_outcomes.ndjson line count does not match tool-gate-outcomes.json")
    for index, line in enumerate(lines):
        try:
            parsed = json.loads(line)
        except json.JSONDecodeError as error:
            fail(f"invalid tool_gate_outcomes.ndjson line {index + 1}: {error}")
        if parsed != outcome_entries[index]:
            fail(f"tool_gate_outcomes.ndjson line {index + 1} does not match JSON artifact")


def require_tool_gate_outcome_entry(entry: dict) -> None:
    if not isinstance(entry, dict):
        fail(f"tool gate outcome is not an object: {entry!r}")
    if entry.get("schema") != "ub-review.tool_gate_outcome.v1":
        fail(f"tool gate outcome has wrong schema: {entry!r}")
    for field in [
        "tool",
        "sensor_status",
        "sensor_reason",
        "sensor_receipt_path",
        "status_source",
        "outcome",
        "reason",
        "packet_policy",
        "gate_policy",
    ]:
        if not isinstance(entry.get(field), str) or not entry[field]:
            fail(f"tool gate outcome missing string field {field}: {entry!r}")
    if entry.get("outcome") not in {"passed", "failed", "not_evaluated", "missing_evidence"}:
        fail(f"tool gate outcome has unsupported outcome: {entry!r}")
    if entry.get("evaluated") not in {True, False}:
        fail(f"tool gate outcome evaluated is not boolean: {entry!r}")
    if entry.get("required") not in {True, False}:
        fail(f"tool gate outcome required is not boolean: {entry!r}")
    if entry.get("planned_run") not in {True, False}:
        fail(f"tool gate outcome planned_run is not boolean: {entry!r}")
    if entry.get("status_source") != "tool-status.json":
        fail(f"tool gate outcome status_source is invalid: {entry!r}")
    if entry.get("packet_policy") != "gate-only":
        fail(f"tool gate outcome packet_policy is invalid: {entry!r}")
    if entry.get("gate_policy") != "trust-affecting":
        fail(f"tool gate outcome gate_policy is invalid: {entry!r}")
    source_artifacts = entry.get("source_artifacts")
    if not isinstance(source_artifacts, list) or not all(
        isinstance(item, str) and item for item in source_artifacts
    ):
        fail(f"tool gate outcome source_artifacts is not a string array: {entry!r}")
    metrics = entry.get("metrics")
    if not isinstance(metrics, dict):
        fail(f"tool gate outcome metrics is not an object: {entry!r}")
    new_unsuppressed = metrics.get("new_unsuppressed")
    if new_unsuppressed is not None and (
        not isinstance(new_unsuppressed, int)
        or isinstance(new_unsuppressed, bool)
        or new_unsuppressed < 0
    ):
        fail(f"tool gate outcome new_unsuppressed metric is invalid: {entry!r}")
    require_tool_gate_policy("tool-gate-outcomes.json", entry["tool"], entry.get("policy"))


def require_tool_gate_outcome_consistency(entry: dict) -> None:
    tool_id = entry["tool"]
    outcome = entry["outcome"]
    evaluated = entry["evaluated"]
    sensor_status = entry["sensor_status"]
    policy = entry["policy"]
    metrics = entry["metrics"]
    max_new_unsuppressed = policy.get("max_new_unsuppressed")
    new_unsuppressed = metrics.get("new_unsuppressed")

    if outcome in {"passed", "failed"} and evaluated is not True:
        fail(f"tool-gate-outcomes.json {tool_id} outcome {outcome} must be evaluated")
    if outcome in {"not_evaluated", "missing_evidence"} and evaluated is not False:
        fail(f"tool-gate-outcomes.json {tool_id} outcome {outcome} must not be evaluated")
    # A sensor that crashed or timed out never produced a threshold verdict:
    # that is missing evidence, never an evaluated `failed` threshold.
    if sensor_status in {"failed", "timed_out"} and outcome != "missing_evidence":
        fail(
            f"tool-gate-outcomes.json {tool_id} sensor without verdict must produce "
            "missing_evidence outcome"
        )
    # `passed`/`failed` are evaluated threshold verdicts, which require an ok
    # sensor run that produced a gate-decision receipt.
    if outcome in {"passed", "failed"} and sensor_status != "ok":
        fail(f"tool-gate-outcomes.json {tool_id} evaluated outcome with non-ok sensor status")
    if sensor_status != "ok" or max_new_unsuppressed is None:
        return

    if outcome == "passed":
        if not isinstance(new_unsuppressed, int) or isinstance(new_unsuppressed, bool):
            fail(f"tool-gate-outcomes.json {tool_id} passed without new_unsuppressed metric")
        if new_unsuppressed > max_new_unsuppressed:
            fail(
                f"tool-gate-outcomes.json {tool_id} passed with new_unsuppressed "
                f"{new_unsuppressed} above threshold {max_new_unsuppressed}"
            )
    elif outcome == "failed":
        if not isinstance(new_unsuppressed, int) or isinstance(new_unsuppressed, bool):
            fail(f"tool-gate-outcomes.json {tool_id} failed without new_unsuppressed metric")
        if new_unsuppressed <= max_new_unsuppressed:
            fail(
                f"tool-gate-outcomes.json {tool_id} failed with new_unsuppressed "
                f"{new_unsuppressed} within threshold {max_new_unsuppressed}"
            )
    elif new_unsuppressed is not None:
        fail(
            f"tool-gate-outcomes.json {tool_id} unevaluated outcome carries "
            "new_unsuppressed metric"
        )


def require_tool_entries(artifact: dict, path: str) -> dict[str, dict]:
    tools = artifact.get("tools")
    if not isinstance(tools, list) or not tools:
        fail(f"{path} tools is not a non-empty array")
    by_id: dict[str, dict] = {}
    for entry in tools:
        if not isinstance(entry, dict):
            fail(f"{path} tool entry is not an object: {entry!r}")
        tool_id = entry.get("id")
        if not isinstance(tool_id, str) or not tool_id:
            fail(f"{path} tool entry missing id: {entry!r}")
        if tool_id in by_id:
            fail(f"{path} duplicate tool id {tool_id}")
        for field in ["class", "command", "required_if", "required_reason", "runtime_profile"]:
            if not isinstance(entry.get(field), str) or not entry[field]:
                fail(f"{path} {tool_id} missing string field {field}: {entry!r}")
        for field in ["required", "planned_run", "requires_lease"]:
            if not isinstance(entry.get(field), bool):
                fail(f"{path} {tool_id} missing boolean field {field}: {entry!r}")
        for field in ["timeout_sec", "artifact_budget_mb"]:
            value = entry.get(field)
            if not isinstance(value, int) or isinstance(value, bool) or value < 0:
                fail(f"{path} {tool_id} field {field} is invalid: {entry!r}")
        if path == "tool-status.json":
            for field in ["status", "reason"]:
                if not isinstance(entry.get(field), str) or not entry[field]:
                    fail(f"{path} {tool_id} missing string field {field}: {entry!r}")
            if not isinstance(entry.get("timed_out"), bool):
                fail(f"{path} {tool_id} timed_out is not boolean: {entry!r}")
            exit_code = entry.get("exit_code")
            if exit_code is not None and (
                not isinstance(exit_code, int) or isinstance(exit_code, bool)
            ):
                fail(f"{path} {tool_id} exit_code is invalid: {entry!r}")
        artifact_paths = entry.get("artifact_paths")
        if not isinstance(artifact_paths, list) or not all(
            isinstance(item, str) and item for item in artifact_paths
        ):
            fail(f"{path} {tool_id} artifact_paths is not a string array")
        gate = entry.get("gate")
        if gate is not None:
            require_tool_gate_policy(path, tool_id, gate)
        by_id[tool_id] = entry
    return by_id


def require_tool_status_matches_resolved(
    tool_id: str, resolved_entry: dict, status_entry: dict
) -> None:
    mirrored_fields = [
        "class",
        "command",
        "required_if",
        "required",
        "required_reason",
        "runtime_profile",
        "planned_run",
        "timeout_sec",
        "artifact_budget_mb",
        "requires_lease",
        "gate",
        "artifact_paths",
    ]
    for field in mirrored_fields:
        if status_entry.get(field) != resolved_entry.get(field):
            fail(
                f"tool-status.json {field} for {tool_id} "
                "does not match resolved-tools.json"
            )


def require_tool_gate_policy(path: str, tool_id: str, gate: object) -> None:
    if not isinstance(gate, dict):
        fail(f"{path} {tool_id} gate is not an object")
    scope = gate.get("scope")
    # Allowlist kept in lockstep with KNOWN_TOOL_GATE_SCOPES in src/config.rs:
    # on-diff is the only scope semantics that exist; the loader records any
    # other value as a PolicyError and strips it before artifacts are written.
    if scope is not None and scope != "on-diff":
        fail(f"{path} {tool_id} gate.scope is invalid: {gate!r}")
    max_new_unsuppressed = gate.get("max_new_unsuppressed")
    if max_new_unsuppressed is not None and (
        not isinstance(max_new_unsuppressed, int)
        or isinstance(max_new_unsuppressed, bool)
        or max_new_unsuppressed < 0
    ):
        fail(f"{path} {tool_id} gate.max_new_unsuppressed is invalid: {gate!r}")
    unknown = sorted(set(gate) - {"scope", "max_new_unsuppressed"})
    if unknown:
        fail(f"{path} {tool_id} gate has unsupported field(s): {', '.join(unknown)}")
    if not gate:
        fail(f"{path} {tool_id} gate is empty")


def require_coverage_status_artifact(root: pathlib.Path, tool_status: dict) -> None:
    paths = tool_status.get("artifact_paths", [])
    if "sensors/coverage/status.json" not in paths:
        fail("tool-status.json coverage missing status artifact path")
    for path in [
        "sensors/coverage/coverage-summary.json",
        "sensors/coverage/changed-lines.json",
        "sensors/coverage/upload.json",
    ]:
        if path not in paths:
            fail(f"tool-status.json coverage missing {path}")
    status = load_json(root / "sensors/coverage/status.json")
    if status.get("schema") != "ub-review.coverage_status.v1":
        fail("sensors/coverage/status.json has wrong schema")
    if status.get("status") != tool_status.get("status"):
        fail("coverage status.json status does not match tool-status.json")
    if status.get("reason") != tool_status.get("reason"):
        fail("coverage status.json reason does not match tool-status.json")
    if status.get("execution_surface_only") is not True:
        fail("coverage status.json must mark execution_surface_only true")
    if status.get("correctness_claim") is not False:
        fail("coverage status.json must not claim correctness")

    lcov = status.get("lcov")
    if not isinstance(lcov, dict):
        fail("coverage status.json lcov is not an object")
    if lcov.get("path") != "sensors/coverage/lcov.info":
        fail("coverage status.json lcov path is invalid")
    if lcov.get("present") != (root / "sensors/coverage/lcov.info").is_file():
        fail("coverage status.json lcov present does not match artifact")

    summary_ref = status.get("summary")
    if not isinstance(summary_ref, dict):
        fail("coverage status.json summary is not an object")
    if summary_ref.get("path") != "sensors/coverage/coverage-summary.json":
        fail("coverage status.json summary path is invalid")
    summary = load_json(root / "sensors/coverage/coverage-summary.json")
    if summary.get("schema") != "ub-review.coverage_summary.v1":
        fail("sensors/coverage/coverage-summary.json has wrong schema")
    require_coverage_telemetry_flags(summary, "coverage-summary.json")
    if summary.get("status") != summary_ref.get("status"):
        fail("coverage status.json summary status does not match summary receipt")
    if summary.get("lcov") != lcov:
        fail("coverage summary lcov does not match status.json")
    for field in ["line_totals", "function_totals"]:
        totals = summary.get(field)
        if not isinstance(totals, dict):
            fail(f"coverage summary {field} is not an object")
        for key in ["found", "hit"]:
            if not isinstance(totals.get(key), int) or totals[key] < 0:
                fail(f"coverage summary {field}.{key} is not a non-negative integer")

    changed_lines = status.get("changed_lines")
    if not isinstance(changed_lines, dict) or not isinstance(changed_lines.get("status"), str):
        fail("coverage status.json changed_lines status is invalid")
    if changed_lines.get("path") != "sensors/coverage/changed-lines.json":
        fail("coverage status.json changed_lines path is invalid")
    if changed_lines["status"] not in {"not_collected", "collected", "unknown"}:
        fail("coverage status.json changed_lines status is unsupported")
    changed_line_receipt = load_json(root / "sensors/coverage/changed-lines.json")
    if changed_line_receipt.get("schema") != "ub-review.coverage_changed_lines.v1":
        fail("sensors/coverage/changed-lines.json has wrong schema")
    require_coverage_telemetry_flags(changed_line_receipt, "changed-lines.json")
    if changed_line_receipt.get("status") != changed_lines.get("status"):
        fail("coverage changed-lines status does not match status.json")

    upload = status.get("upload")
    if not isinstance(upload, dict) or not isinstance(upload.get("status"), str):
        fail("coverage status.json upload status is invalid")
    if upload.get("path") != "sensors/coverage/upload.json":
        fail("coverage status.json upload path is invalid")
    upload_receipt = load_json(root / "sensors/coverage/upload.json")
    if upload_receipt.get("schema") != "ub-review.coverage_upload.v1":
        fail("sensors/coverage/upload.json has wrong schema")
    require_coverage_telemetry_flags(upload_receipt, "upload.json")
    if upload_receipt.get("status") != upload.get("status"):
        fail("coverage upload status does not match status.json")


def require_coverage_telemetry_flags(receipt: dict, label: str) -> None:
    if receipt.get("execution_surface_only") is not True:
        fail(f"{label} must mark execution_surface_only true")
    if receipt.get("correctness_claim") is not False:
        fail(f"{label} must not claim correctness")


def require_model_receipts(review: dict, metrics: dict, min_ok_model_lanes: int) -> None:
    if min_ok_model_lanes <= 0:
        return
    preflights = review.get("provider_preflights")
    if not isinstance(preflights, list) or not preflights:
        fail("review.json provider_preflights is empty")
    ok_preflights = [receipt for receipt in preflights if receipt.get("status") == "ok"]
    if not ok_preflights:
        fail("no ok provider preflight receipt")
    usable_lanes = [
        lane
        for lane in review.get("model_lanes", [])
        if lane.get("status") in {"ok", "degraded"} and lane.get("lane") != "refuter"
    ]
    if len(usable_lanes) < min_ok_model_lanes:
        fail(
            "expected at least "
            f"{min_ok_model_lanes} usable ok/degraded model lanes, got {len(usable_lanes)}"
        )
    model_metrics = metrics.get("models", {})
    if model_metrics.get("provider_preflight_calls_attempted", 0) < 1:
        fail("metrics did not record a provider preflight call")
    if model_metrics.get("model_lane_calls_attempted", 0) < min_ok_model_lanes:
        fail("metrics did not record enough model lane calls")


def require_no_model_evidence_failures(review: dict) -> None:
    failures = review.get("missing_or_failed_model_evidence", [])
    if failures:
        fail(f"missing_or_failed_model_evidence is not empty: {failures!r}")


def require_post_receipt(root: pathlib.Path) -> None:
    post_result = root / "review/post-result.json"
    post_error = root / "review/post-error.json"
    if not post_result.exists() and not post_error.exists():
        github_skip = root / "review/github-review-skip.json"
        if github_skip.exists():
            skip = load_json(github_skip)
            if (
                skip.get("status") == "skipped"
                and skip.get("review_payload_status") in SKIPPED_REVIEW_PAYLOAD_STATUSES
            ):
                return
        fail("neither post-result.json nor post-error.json exists")
    if post_result.exists():
        receipt = load_json(post_result)
        if receipt.get("status") == "skipped":
            if receipt.get("review_payload_status") not in SKIPPED_REVIEW_PAYLOAD_STATUSES:
                fail("post-result.json skipped receipt has wrong review_payload_status")
            require_skipped_payload_contract(receipt, root, post_result)
            return
        if receipt.get("status") != "ok":
            fail(f"post-result.json status expected ok or skipped, got {receipt.get('status')!r}")
        if receipt.get("review_json_valid") is not True:
            fail("post-result.json did not mark review_json_valid true")
        if receipt.get("off_diff_comment_count") not in (None, 0):
            fail("post-result.json recorded off-diff comments")
    if post_error.exists():
        receipt = load_json(post_error)
        if receipt.get("status") != "failed":
            fail(f"post-error.json status expected failed, got {receipt.get('status')!r}")
        if receipt.get("review_json_valid") is not True:
            fail("post-error.json review_json_valid is not true")
        if receipt.get("off_diff_comment_count") not in (None, 0):
            fail("post-error.json recorded off-diff comments")
        if receipt.get("failure_tolerated") is not True:
            fail("post-error.json failure_tolerated is not true")


def require_no_secret_markers(root: pathlib.Path) -> None:
    paths = [
        root / "running-summary.md",
        root / "work_queue.json",
        root / "work_events.ndjson",
        root / "tool-gate-outcomes.json",
        root / "tool_gate_outcomes.ndjson",
        root / "review/shared_context.md",
        root / "review/pr_thread_context.json",
        root / "review/terminal_state.json",
        root / "review/review.json",
        root / "review/review.md",
        root / "review/github-review.json",
        root / "review/github-review-skip.json",
        root / "review/post-result.json",
        root / "review/post-error.json",
        root / "review/post-stdout.json",
        root / "review/post-stderr.txt",
        root / "review/resource_leases.json",
        root / "review/resource_plan.md",
        root / "review/tool-gate-outcomes.json",
        root / "resource_leases.ndjson",
    ]
    paths.extend((root / "lanes").glob("*.md"))
    paths.extend((root / "sensors").glob("*/ub-review-sensor-status.json"))
    paths.extend((root / "review/model").glob("**/*.json"))
    paths.extend((root / "review/provider-preflight").glob("**/*.json"))

    for path in paths:
        if not path.exists() or path.is_dir():
            continue
        text = read_text(path)
        marker = secret_leak_marker(text)
        if marker is not None:
            fail(f"secret marker {marker!r} found in {path}")


def write_self_test_json(path: pathlib.Path, value: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value), encoding="utf-8")


def self_test_empty_candidate_artifacts_without_dir() -> None:
    with tempfile.TemporaryDirectory() as temp_dir:
        root = pathlib.Path(temp_dir)
        review = {"inline_comments": [], "summary_only_findings": []}
        write_self_test_json(root / "review/candidates.json", [])
        (root / "candidates.ndjson").write_text("", encoding="utf-8")
        require_candidate_artifacts(root, review)


def self_test_missing_nonempty_candidate_dir_fails() -> None:
    with tempfile.TemporaryDirectory() as temp_dir:
        root = pathlib.Path(temp_dir)
        review = {
            "inline_comments": [
                {
                    "lane": "tests-oracle",
                    "severity": "medium",
                    "confidence": "high",
                    "path": "src/main.rs",
                    "line": 12,
                    "side": "RIGHT",
                    "body": "Confirm the focused proof reaches the changed branch.",
                    "evidence": "self-test candidate fixture",
                }
            ],
            "summary_only_findings": [],
        }
        candidates = expected_candidate_records(review)
        write_self_test_json(root / "review/candidates.json", candidates)
        (root / "candidates.ndjson").write_text(
            "\n".join(json.dumps(candidate) for candidate in candidates) + "\n",
            encoding="utf-8",
        )
        require_candidate_artifacts(root, review)


def self_test_coverage_sidecar_receipts() -> None:
    with tempfile.TemporaryDirectory() as temp_dir:
        root = pathlib.Path(temp_dir)
        coverage_dir = root / "sensors/coverage"
        coverage_dir.mkdir(parents=True)
        (coverage_dir / "lcov.info").write_text(
            "TN:\nSF:src/lib.rs\nFNF:2\nFNH:1\nLF:4\nLH:3\nend_of_record\n",
            encoding="utf-8",
        )
        tool_status = {
            "status": "ok",
            "reason": "completed",
            "artifact_paths": [
                "sensors/coverage/ub-review-sensor-status.json",
                "sensors/coverage/status.json",
                "sensors/coverage/coverage-summary.json",
                "sensors/coverage/changed-lines.json",
                "sensors/coverage/upload.json",
                "sensors/coverage/lcov.info",
            ],
        }
        write_self_test_json(
            coverage_dir / "coverage-summary.json",
            {
                "schema": "ub-review.coverage_summary.v1",
                "status": "collected",
                "reason": "lcov.info parsed",
                "execution_surface_only": True,
                "correctness_claim": False,
                "lcov": {
                    "path": "sensors/coverage/lcov.info",
                    "present": True,
                },
                "line_totals": {"found": 4, "hit": 3},
                "function_totals": {"found": 2, "hit": 1},
            },
        )
        write_self_test_json(
            coverage_dir / "changed-lines.json",
            {
                "schema": "ub-review.coverage_changed_lines.v1",
                "status": "not_collected",
                "reason": "changed-line coverage is not computed by the local coverage sensor yet",
                "execution_surface_only": True,
                "correctness_claim": False,
                "source_artifacts": ["sensors/coverage/lcov.info"],
            },
        )
        write_self_test_json(
            coverage_dir / "upload.json",
            {
                "schema": "ub-review.coverage_upload.v1",
                "status": "workflow_owned",
                "reason": "Codecov upload is performed by the coverage workflow, not this local sensor",
                "execution_surface_only": True,
                "correctness_claim": False,
                "source_artifacts": [],
            },
        )
        write_self_test_json(
            coverage_dir / "status.json",
            {
                "schema": "ub-review.coverage_status.v1",
                "status": "ok",
                "reason": "completed",
                "execution_surface_only": True,
                "correctness_claim": False,
                "lcov": {
                    "path": "sensors/coverage/lcov.info",
                    "present": True,
                },
                "summary": {
                    "path": "sensors/coverage/coverage-summary.json",
                    "status": "collected",
                },
                "changed_lines": {
                    "path": "sensors/coverage/changed-lines.json",
                    "status": "not_collected",
                },
                "upload": {
                    "path": "sensors/coverage/upload.json",
                    "status": "workflow_owned",
                },
            },
        )
        require_coverage_status_artifact(root, tool_status)


def self_test_tool_status_metadata_mismatch_fails() -> None:
    resolved_entry = {
        "id": "ripr",
        "class": "static",
        "command": "ripr",
        "required_if": "rust-behavior-or-tests-changed",
        "required": False,
        "required_reason": "Rust behavior or tests changed",
        "runtime_profile": "gh-runner",
        "planned_run": True,
        "timeout_sec": 240,
        "artifact_budget_mb": 128,
        "requires_lease": False,
        "gate": None,
        "artifact_paths": ["sensors/ripr/ub-review-sensor-status.json"],
    }
    status_entry = dict(resolved_entry)
    status_entry["timeout_sec"] = 120
    require_tool_status_matches_resolved("ripr", resolved_entry, status_entry)


def self_test_non_discriminating_routes_as_missing_evidence() -> None:
    status = routed_status_for_proof_receipt(
        {
            "result": "non_discriminating",
        }
    )
    if status != "missing-evidence":
        fail(f"non_discriminating proof routed as {status!r}, expected missing-evidence")


def self_test_sanitize_artifact_name_matches_rust_contract() -> None:
    raw = "confirm-the-focused-proof-before-upstream-" * 12 + "terminal-proof-question"
    sanitized = sanitize_artifact_name(raw)
    digest = hashlib.sha256(raw.encode("utf-8")).hexdigest()
    if len(sanitized) != ARTIFACT_NAME_MAX_CHARS:
        fail(f"sanitized artifact name length is {len(sanitized)}, expected {ARTIFACT_NAME_MAX_CHARS}")
    if not sanitized.endswith("-" + digest[:ARTIFACT_NAME_HASH_CHARS]):
        fail("sanitized artifact name does not include expected hash suffix")
    if sanitize_artifact_name("source-route/question one") != "source-route-question-one":
        fail("short artifact name sanitization drifted from Rust")
    if sanitize_artifact_name("évidence") != "-vidence":
        fail("non-ASCII artifact name sanitization drifted from Rust")


def self_test_tool_gate_outcome_false_pass_fails() -> None:
    entry = {
        "schema": "ub-review.tool_gate_outcome.v1",
        "tool": "ripr",
        "policy": {"scope": "on-diff", "max_new_unsuppressed": 0},
        "required": False,
        "planned_run": True,
        "sensor_status": "ok",
        "sensor_reason": "completed",
        "sensor_receipt_path": "sensors/ripr/ub-review-sensor-status.json",
        "status_source": "tool-status.json",
        "outcome": "passed",
        "evaluated": True,
        "reason": "bad fixture claims pass despite threshold breach",
        "metrics": {"new_unsuppressed": 1},
        "source_artifacts": [
            "sensors/ripr/ub-review-sensor-status.json",
            "tool-status.json",
            "sensors/ripr/gate-decision.json",
        ],
        "packet_policy": "gate-only",
        "gate_policy": "trust-affecting",
    }
    require_tool_gate_outcome_entry(entry)
    require_tool_gate_outcome_consistency(entry)


def crashed_sensor_tool_gate_outcome_entry(outcome: str, evaluated: bool) -> dict:
    return {
        "schema": "ub-review.tool_gate_outcome.v1",
        "tool": "ripr",
        "policy": {"scope": "on-diff", "max_new_unsuppressed": 0},
        "required": False,
        "planned_run": True,
        "sensor_status": "failed",
        "sensor_reason": "exit 101",
        "sensor_receipt_path": "sensors/ripr/ub-review-sensor-status.json",
        "status_source": "tool-status.json",
        "outcome": outcome,
        "evaluated": evaluated,
        "reason": "tool gate threshold could not be evaluated because the sensor did not "
        "produce a verdict (sensor status `failed`)",
        "metrics": {"new_unsuppressed": None},
        "source_artifacts": [
            "sensors/ripr/ub-review-sensor-status.json",
            "tool-status.json",
        ],
        "packet_policy": "gate-only",
        "gate_policy": "trust-affecting",
    }


def self_test_crashed_sensor_routes_as_missing_evidence() -> None:
    # A crashed sensor is missing evidence, never an evaluated threshold
    # failure; the artifact contract pins that distinction.
    entry = crashed_sensor_tool_gate_outcome_entry("missing_evidence", False)
    require_tool_gate_outcome_entry(entry)
    require_tool_gate_outcome_consistency(entry)


def self_test_crashed_sensor_claiming_failed_outcome_fails() -> None:
    entry = crashed_sensor_tool_gate_outcome_entry("failed", True)
    require_tool_gate_outcome_entry(entry)
    require_tool_gate_outcome_consistency(entry)


def self_test_sanitize_artifact_name_bounds_long_values() -> None:
    raw = "candidate-" + ("generated-id-segment-" * 24)
    sanitized = sanitize_artifact_name(raw)
    expected_suffix = hashlib.sha256(raw.encode("utf-8")).hexdigest()[
        :ARTIFACT_NAME_HASH_CHARS
    ]
    if len(sanitized) > ARTIFACT_NAME_MAX_CHARS:
        fail("artifact name sanitizer did not bound long value")
    if not sanitized.endswith(f"-{expected_suffix}"):
        fail("artifact name sanitizer did not use stable hash suffix")
    if sanitize_artifact_name("source-route/question one") != "source-route-question-one":
        fail("artifact name sanitizer changed safe replacement behavior")


def run_self_tests() -> None:
    require_run_mode("review-byok", "self-test review-byok mode")
    require_run_mode("intelligent-ci", "self-test intelligent-ci mode")
    if secret_leak_marker("OPENCODE=opencodeSecret123456") != "OPENCODE":
        fail("self-test OPENCODE secret assignment was not detected")
    if secret_leak_marker("OPENCODE=${{ secrets.OPENCODE }}") is not None:
        fail("self-test OPENCODE secret placeholder was treated as a leak")
    expect_self_test_failure(
        "legacy run mode artifact",
        "expected one of",
        lambda: require_run_mode("review-direct", "self-test legacy mode"),
    )
    factory_key_name = "FACTORY" + "_API_KEY"
    factory_key_value = hashlib.sha256(b"factory-key-self-test").hexdigest()[:24]
    factory_key_assignment = factory_key_name + "=" + factory_key_value
    factory_key_placeholder = factory_key_name + "=${{ secrets." + factory_key_name + " }}"
    if secret_leak_marker(factory_key_assignment) != factory_key_name:
        fail("self-test FACTORY_API_KEY secret assignment was not detected")
    if secret_leak_marker(factory_key_placeholder) is not None:
        fail("self-test FACTORY_API_KEY secret placeholder was treated as a leak")
    low_diversity_value = factory_key_name + "=" + (("x" * 15) + "1")
    if secret_leak_marker(low_diversity_value) is not None:
        fail("self-test low-diversity synthetic value was treated as a leak")
    escaped_placeholder = factory_key_name + r"=\u003csha256-truncated\u003e"
    if secret_leak_marker(escaped_placeholder) is not None:
        fail("self-test escaped placeholder was treated as a leak")
    double_escaped_placeholder = factory_key_name + r"=\\u003csha256-truncated\\u003e"
    if secret_leak_marker(double_escaped_placeholder) is not None:
        fail("self-test double-escaped placeholder was treated as a leak")
    require_github_comment(
        {
            "path": "src/lib.rs",
            "side": "RIGHT",
            "line": 12,
            "body": (
                "[tests-oracle] Added no-finalizer FFI test passes on HEAD "
                "and fails on base+tests."
            ),
        },
        0,
    )
    expect_self_test_failure(
        "inline comment boilerplate",
        "artifact-only boilerplate",
        lambda: require_github_comment(
            {
                "path": "src/lib.rs",
                "side": "RIGHT",
                "line": 12,
                "body": (
                    "[tests-oracle] No blocking finding after bounded review; "
                    "residual risk remains for human review."
                ),
            },
            1,
        ),
    )
    expect_self_test_failure(
        "residual risk PR section",
        "artifact-only status section",
        lambda: require_pr_review_body_policy(
            "## Residual risk\n\n- External trust risk remains.",
            pathlib.Path("review/github-review.json"),
        ),
    )
    expect_self_test_failure(
        "meta review prose",
        "artifact-only boilerplate",
        lambda: require_pr_review_body_policy(
            (
                "## Verification questions\n\n"
                "- Confirm the cached prior observation still matches; "
                "the refuter demoted inline candidate because Gate proof is pending."
            ),
            pathlib.Path("review/github-review.json"),
        ),
    )
    expect_self_test_failure(
        "too many PR body bullets",
        "not concise enough",
        lambda: require_pr_review_body_policy(
            (
                "## Decision\n\n"
                "- Needs focused cleanup before merge.\n\n"
                "## Verification questions\n\n"
                + "\n".join(
                    f"- Confirm decision-relevant proof item {index}."
                    for index in range(1, 14)
                )
            ),
            pathlib.Path("review/github-review.json"),
        ),
    )
    expect_self_test_failure(
        "oversized PR body",
        "not concise enough",
        lambda: require_pr_review_body_policy(
            (
                "## Decision\n\n"
                "- Needs focused cleanup before merge.\n\n"
                "## Evidence gaps\n\n"
                f"- {'proof gap ' * 800}"
            ),
            pathlib.Path("review/github-review.json"),
        ),
    )
    canonical_groups = expected_proof_request_groups(
        [
            {
                "schema": "ub-review.proof_request.v1",
                "id": "proof-tests-001",
                "lane": "tests-oracle",
                "requested_by": ["tests-oracle"],
                "command": "bun test test/js/bun/ffi/ffi.test.js -t 'ffi toBuffer bad free'",
                "reason": "Need red/green proof.",
                "cost": "focused-test",
                "timeout_sec": 300,
                "required": False,
                "status": "requested",
            },
            {
                "schema": "ub-review.proof_request.v1",
                "id": "proof-opposition-001",
                "lane": "opposition",
                "requested_by": ["opposition"],
                "command": (
                    "bun bd test test/js/bun/ffi/ffi.test.js "
                    "--test-name-pattern \"ffi toBuffer bad free\""
                ),
                "reason": "Same focused proof.",
                "cost": "focused-test",
                "timeout_sec": 300,
                "required": True,
                "status": "requested",
            },
            {
                "schema": "ub-review.proof_request.v1",
                "id": "proof-system-bun-001",
                "lane": "tests-red-green",
                "requested_by": ["tests-red-green"],
                "command": (
                    "USE_SYSTEM_BUN=1 bun test test/js/bun/ffi/ffi.test.js "
                    "-t 'ffi toBuffer bad free'"
                ),
                "reason": "Same old-main red proof.",
                "cost": "focused-test",
                "timeout_sec": 300,
                "required": False,
                "status": "requested",
            },
        ]
    )
    if len(canonical_groups) != 1 or canonical_groups[0]["duplicate_count"] != 3:
        fail("canonical Bun proof request grouping self-test failed")
    if (
        canonical_groups[0]["command"]
        != "bun test test/js/bun/ffi/ffi.test.js -t 'ffi toBuffer bad free'"
    ):
        fail("canonical Bun proof request grouping did not preserve first raw command")
    expect_self_test_failure(
        "standing workflow trust posture prose",
        "workflow trust posture prose",
        lambda: require_pr_review_body_policy(
            (
                "## Confirmed findings\n\n"
                "- Ub-review action receives secrets.MINIMAX and github.token at runtime; "
                "a malicious or compromised dad0f23 would exfiltrate these. Pinning to SHA "
                "is correct posture but does not eliminate upstream trust."
            ),
            pathlib.Path("review/github-review.json"),
        ),
    )
    expect_self_test_failure(
        "no-defect pinning posture prose",
        "workflow trust posture prose",
        lambda: require_pr_review_body_policy(
            (
                "## Confirmed findings\n\n"
                "- No pinning defect introduced. The only standing concern is upstream "
                "SHA trust for EffortlessMetrics/ub-review@e76ccbc, which is identical "
                "in posture to the prior pin and is a repo-level policy item, not a "
                "diff finding."
            ),
            pathlib.Path("review/github-review.json"),
        ),
    )
    expect_self_test_failure(
        "mechanical pin no-change prose",
        "workflow trust posture prose",
        lambda: require_pr_review_body_policy(
            (
                "## Confirmed findings\n\n"
                "- The diff is a 4-line mechanical SHA bump at the three expected "
                "sites: cache `key`, `restore-keys` prefix, and action `uses:`. "
                "No permission, trigger, or `with:` block change; net new "
                "secret/permission surface relative to the prior pin is zero."
            ),
            pathlib.Path("review/github-review.json"),
        ),
    )
    expect_self_test_failure(
        "stale bot refutation prose",
        "workflow trust posture prose",
        lambda: require_pr_review_body_policy(
            (
                "## Refuted\n\n"
                "- cursor[bot] and coderabbitai[bot] comments claim target is e76ccbcb... "
                "and demand swap back; PR body, diff, and head tree all show ec8f890 "
                "as the actual target. Their objection is a false positive against "
                "the current diff and reopens nothing."
            ),
            pathlib.Path("review/github-review.json"),
        ),
    )
    expect_self_test_failure(
        "stale bot target sha prose",
        "workflow trust posture prose",
        lambda: require_pr_review_body_policy(
            (
                "## Confirmed findings\n\n"
                "- CodeRabbit's review-comment at ub-review-packet.yml:58 asserts "
                "the PR gate target SHA is 892e1bb44b7cb24753b7701b405d078f4ef11ee1, "
                "not be524219e33ff37edeab61ddc28c01250a08b492 used in the diff. "
                "If that claim is correct the workflow pin does not match the upstream gate.\n\n"
                "## Evidence gaps\n\n"
                "- CodeRabbit review-comment on .github/workflows/ub-review-packet.yml:58, "
                "scripted check showing 0 references to 892e1bb44b... in the file; "
                "PR body and droid-ub/droid-tests receipts only confirm internal "
                "lockstep, not match to gate target."
            ),
            pathlib.Path("review/github-review.json"),
        ),
    )
    expect_self_test_failure(
        "refuted-only PR body",
        "refuted-only artifact note",
        lambda: require_pr_review_body_policy(
            "## Refuted\n\n- A prior objection was false, and no finding remains.",
            pathlib.Path("review/github-review.json"),
        ),
    )
    summary_only_waived_body = (
        "## Confirmed findings\n\n"
        "- [opposition] Residual risk remains for human review in the resize path."
    )
    expect_self_test_failure(
        "summary-only boilerplate body without waiver",
        "artifact-only boilerplate",
        lambda: require_pr_review_body_policy(
            summary_only_waived_body, pathlib.Path("review/github-review.json")
        ),
    )
    require_pr_review_body_policy(
        summary_only_waived_body,
        pathlib.Path("review/github-review.json"),
        waive_suppressible=True,
    )
    expect_self_test_failure(
        "summary-only waiver keeps status-section wall",
        "artifact-only status section",
        lambda: require_pr_review_body_policy(
            "## Confirmed findings\n\n- A finding.\n\n## Sensor status\n\n- ok",
            pathlib.Path("review/github-review.json"),
            waive_suppressible=True,
        ),
    )
    expect_self_test_failure(
        "summary-only waiver keeps execution-summary wall",
        "execution summary boilerplate",
        lambda: require_pr_review_body_policy(
            "## Confirmed findings\n\n- A finding.\n\nRuntime: `31s`",
            pathlib.Path("review/github-review.json"),
            waive_suppressible=True,
        ),
    )
    with tempfile.TemporaryDirectory() as summary_only_tempdir:
        summary_only_root = pathlib.Path(summary_only_tempdir)
        if effective_summary_only_body(summary_only_root) != "suppress":
            fail("self-test missing effective-config should default to suppress")
        write_self_test_json(
            summary_only_root / "effective-config.json",
            {"review_body": {"summary_only_body": "post_substantive"}},
        )
        if effective_summary_only_body(summary_only_root) != "post_substantive":
            fail("self-test post_substantive effective config was not honored")
        write_self_test_json(
            summary_only_root / "effective-config.json",
            {"review_body": {"summary_only_body": "post-everything"}},
        )
        if effective_summary_only_body(summary_only_root) != "suppress":
            fail("self-test unknown summary_only_body should fall back to suppress")
    expect_self_test_failure(
        "workflow tool-status gap prose",
        "workflow trust posture prose",
        lambda: require_pr_review_body_policy(
            (
                "## Evidence gaps\n\n"
                "- actionlint receipt is 'ok' per sensor table; no per-line output "
                "inlined into this lane packet, so re-verification of lint findings "
                "depends on the central proof broker artifact.\n"
                "- No fresh PR-build smoke run is available (build/test skipped, "
                "--allow-heavy required); only tokmd/actionlint receipts are present "
                "for this 4-line workflow pin."
            ),
            pathlib.Path("review/github-review.json"),
        ),
    )
    expect_self_test_failure(
        "paths-ignore no-posture review prose",
        "workflow trust posture prose",
        lambda: require_pr_review_body_policy(
            (
                "## Decision\n\n"
                "- Needs one verification check before upstream.\n\n"
                "## Verification questions\n\n"
                "- Confirm checkout credential persistence: workflows using "
                "pull_request from forks receive a read-only GITHUB_TOKEN; this "
                "lane did not change checkout config, so no new persistence "
                "vector is introduced. Actionlint receipt 'ok' supports no "
                "syntactic regression.\n\n"
                "## Refuted\n\n"
                "- Adding a workflow file to paths-ignore could grant implicit "
                "permission expansion; refuted because: paths-ignore only "
                "filters trigger activation; it does not alter token scopes, "
                "permissions blocks, or any job-level security context.\n\n"
                "## Evidence gaps\n\n"
                "- zizmor, gitleaks, osv-scanner, cargo-audit, cargo-deny, "
                "shellcheck, semgrep, coverage all disabled by config or "
                "trigger-mismatched. No security/pinning tool independently "
                "re-validated this workflow file."
            ),
            pathlib.Path("review/github-review.json"),
        ),
    )
    expect_self_test_failure(
        "paths-ignore smoke-proof review prose",
        "workflow trust posture prose",
        lambda: require_pr_review_body_policy(
            (
                "## Decision\n\n"
                "- Needs one verification check before upstream.\n\n"
                "## Verification questions\n\n"
                "- Confirm no focused smoke proof (workflow_run on a fork-PR dry-run, "
                "or a temporary pull_request_target guard test) was executed for the "
                "paths-ignore change. Trust rests on actionlint parse only; semantic "
                "skip behavior on the droid lane is not proven by sensors.\n\n"
                "## Refuted\n\n"
                "- adding ub-review-packet.yml to paths-ignore could mask future "
                "unpinned uses: additions in that file from Droid lane coverage; "
                "refuted because: paths-ignore lift is per-PR: any future PR that "
                "also touches ub-review-packet.yml will change the changed-files set "
                "and re-trigger Droid. Droid lanes are non-blocking/auxiliary by "
                "design; UB gate is the authoritative review.\n\n"
                "## Evidence gaps\n\n"
                "- PR body states actionlint is not installed locally, so the 'ok' "
                "receipt must come from the ub-review gate's own tooling rather "
                "than a local pre-push run; trust depends on that gate having "
                "actually executed actionlint v1 against this ref."
            ),
            pathlib.Path("review/github-review.json"),
        ),
    )
    expect_self_test_failure(
        "paths-ignore actionlint skip-proof review prose",
        "workflow trust posture prose",
        lambda: require_pr_review_body_policy(
            (
                "## Decision\n\n"
                "- Needs one verification check before upstream.\n\n"
                "## Verification questions\n\n"
                "- Confirm actionlint receipt 'ok' confirms syntactic validity, "
                "but no semantic proof of skip behavior on the droid lane is "
                "available; trust rests on actionlint parse plus per-PR trigger "
                "semantics - the droid lane is auxiliary/non-blocking and the "
                "UB gate is authoritative, so residual workflow risk is bounded.\n\n"
                "## Refuted\n\n"
                "- paths-ignore addition could mask future unpinned uses: "
                "additions in ub-review-packet.yml from Droid lane coverage; "
                "refuted because: paths-ignore lift is per-PR: any future PR "
                "that also touches ub-review-packet.yml (adds/changes uses:) "
                "will change the changed-files set and re-trigger Droid. "
                "UB gate is the authoritative review surface and runs on the "
                "new pin.\n\n"
                "## Parked follow-ups\n\n"
                "- Residual workflow risk: cache key/restore-keys prefix is "
                "coupled to action SHA. Any future repin must update all three "
                "sites; a partial update silently mismatches cache restore. "
                "Not actionable in this PR (current state is consistent) - "
                "parked for follow-up lint rule or script.\n\n"
                "## Evidence gaps\n\n"
                "- trust gap: no focused smoke proof (workflow_run on fork-PR "
                "dry-run or pull_request_target guard) executed for the "
                "paths-ignore change; semantic skip behavior on Droid lane "
                "unproven beyond actionlint parse."
            ),
            pathlib.Path("review/github-review.json"),
        ),
    )
    expect_self_test_failure(
        "workflow lockstep summary review prose",
        "workflow trust posture prose",
        lambda: require_pr_review_body_policy(
            (
                "## Decision\n\n"
                "- Needs one verification check before upstream.\n\n"
                "## Verification questions\n\n"
                "- Workflow-pinning lane for PR #49. Two workflow YAML files "
                "touched. Pin lockstep verified across 3 sites, old pin absent, "
                "cache key/restore-keys prefix match, no other third-party "
                "actions changed.\n\n"
                "## Parked follow-ups\n\n"
                "- Cache key/restore-keys prefix is coupled to action SHA; any "
                "future partial repin silently mismatches restore. Current state "
                "consistent, parked for lint-rule follow-up."
            ),
            pathlib.Path("review/github-review.json"),
        ),
    )
    meta_observations = [
        self_test_observation(
            "obsgrp-meta-proof",
            "meta-proof",
            "Gate proof is pending and commit-existence/ancestry proof is unavailable.",
            "verification-question",
        ),
        self_test_observation(
            "obsgrp-meta-cache",
            "meta-cache",
            "The cached prior observation still matches current evidence.",
            "bug",
        ),
        self_test_observation(
            "obsgrp-tool-status",
            "tool-status",
            "Sensors: zizmor disabled by config.",
            "missing-evidence",
        ),
        self_test_observation(
            "obsgrp-actionlint",
            "actionlint-status",
            "Actionlint ran ok; no reviewer-value change remains.",
            "missing-evidence",
        ),
        self_test_observation(
            "obsgrp-pinning-format",
            "pinning-format-refuted",
            (
                "Pinning format could be 39-hex or all-zero making the gate unsafe.; "
                "refuted because: e76ccbcbe94258fd03cf6ddb4e1536833cad610d "
                "is 40 hex characters, non-zero, and matches expected SHA-1 shape; "
                "the gate's SHA-pinning control remains effective."
            ),
            "false-premise",
        ),
        self_test_observation(
            "obsgrp-stale-bot",
            "stale-bot-pin",
            (
                "cursor[bot] and coderabbitai[bot] comments claim target is e76ccbcb... "
                "and demand swap back; PR body, diff, and head tree all show ec8f890 "
                "as the actual target. Their objection is a false positive against "
                "the current diff and reopens nothing."
            ),
            "false-premise",
        ),
        self_test_observation(
            "obsgrp-actionlint-artifact",
            "actionlint-artifact-gap",
            (
                "actionlint receipt is 'ok' per sensor table; no per-line output "
                "inlined into this lane packet, so re-verification of lint findings "
                "depends on the central proof broker artifact"
            ),
            "missing-evidence",
        ),
        self_test_observation(
            "obsgrp-heavy-smoke",
            "heavy-smoke-gap",
            (
                "No fresh PR-build smoke run is available (build/test skipped, "
                "--allow-heavy required); only tokmd/actionlint receipts are present "
                "for this 4-line workflow pin"
            ),
            "missing-evidence",
        ),
    ]
    plan = expected_orchestrator_plan([], meta_observations, [], [])
    if plan["follow_up_tasks"]:
        fail("artifact-only meta observations created follow-up tasks in self-test")
    self_test_empty_candidate_artifacts_without_dir()
    expect_self_test_failure(
        "non-empty candidate artifacts without directory",
        "missing candidates directory",
        self_test_missing_nonempty_candidate_dir_fails,
    )
    self_test_coverage_sidecar_receipts()
    self_test_non_discriminating_routes_as_missing_evidence()
    self_test_sanitize_artifact_name_matches_rust_contract()
    expect_self_test_failure(
        "tool status metadata mismatch",
        "tool-status.json timeout_sec for ripr does not match resolved-tools.json",
        self_test_tool_status_metadata_mismatch_fails,
    )
    expect_self_test_failure(
        "tool gate outcome false pass",
        "passed with new_unsuppressed 1 above threshold 0",
        self_test_tool_gate_outcome_false_pass_fails,
    )
    self_test_crashed_sensor_routes_as_missing_evidence()
    expect_self_test_failure(
        "tool gate outcome crashed sensor claiming failed",
        "sensor without verdict must produce missing_evidence outcome",
        self_test_crashed_sensor_claiming_failed_outcome_fails,
    )
    self_test_sanitize_artifact_name_bounds_long_values()
    require_proof_request_files(pathlib.Path("__missing_empty_artifact_dir__"), [])
    require_skipped_payload_contract(
        {"github_review_json": None},
        pathlib.Path("__missing_empty_artifact_dir__"),
        pathlib.Path("review/github-review-skip.json"),
    )
    expect_self_test_failure(
        "skip receipt missing payload path",
        "points at missing artifact",
        lambda: require_skipped_payload_contract(
            {"github_review_json": "review/github-review.json"},
            pathlib.Path("__missing_empty_artifact_dir__"),
            pathlib.Path("review/github-review-skip.json"),
        ),
    )
    print("Bun review artifact verifier self-test passed")


def self_test_observation(
    observation_id: str,
    dedupe_key: str,
    claim: str,
    kind: str,
) -> dict:
    return {
        "schema": "ub-review.observation_group.v1",
        "id": observation_id,
        "dedupe_key": dedupe_key,
        "claim": claim,
        "kind": kind,
        "status": "open",
        "severity": "low",
        "confidence": "medium",
        "path": None,
        "line": None,
        "evidence": [],
        "lanes": ["self-test"],
        "sources": ["self-test"],
        "observation_ids": [f"{observation_id}-raw"],
        "duplicate_count": 0,
    }


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("root", nargs="?", default="target/ub-review")
    parser.add_argument("--min-ok-model-lanes", type=int, default=0)
    parser.add_argument("--max-inline-comments", type=int)
    parser.add_argument("--require-no-model-evidence-failures", action="store_true")
    parser.add_argument("--expected-review-profile", default="bun-ub-v0")
    parser.add_argument("--expected-repo-kind", default="bun")
    parser.add_argument("--self-test", action="store_true")
    return parser.parse_args(argv[1:])


def main(argv: list[str]) -> int:
    args = parse_args(argv)
    if args.self_test:
        run_self_tests()
        return 0

    root = pathlib.Path(args.root)
    if not root.is_dir():
        fail(f"artifact root is not a directory: {root}")

    require_common_tree(root)
    require_summary(root)
    require_profile_artifacts(
        root, args.expected_review_profile, args.expected_repo_kind
    )
    require_sensor_receipts(root)
    require_tool_registry_artifacts(root)
    review = require_review(
        root, args.max_inline_comments, args.expected_review_profile
    )
    metrics = require_metrics(root, review)
    require_model_receipts(review, metrics, args.min_ok_model_lanes)
    if args.require_no_model_evidence_failures:
        require_no_model_evidence_failures(review)
    require_post_receipt(root)
    require_no_secret_markers(root)

    usable_lanes = [
        lane.get("lane")
        for lane in review.get("model_lanes", [])
        if lane.get("status") in {"ok", "degraded"} and lane.get("lane") != "refuter"
    ]
    observations = load_json(root / "review/observations.json")
    unique_observations = load_json(root / "review/unique_observations.json")
    merged_observations = load_json(root / "review/merged_observations.json")
    dropped_observations = load_json(root / "review/dropped_observations.json")
    print(
        "review artifact contract verified: "
        f"root={root} "
        f"review_profile={args.expected_review_profile} "
        f"shared_context={review['shared_context_id']} "
        f"inline_comments={len(review.get('inline_comments', []))} "
        f"observations={len(observations)} "
        f"unique_observations={len(unique_observations)} "
        f"merged_observations={len(merged_observations)} "
        f"dropped_observations={len(dropped_observations)} "
        f"usable_model_lanes={','.join(usable_lanes)}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
