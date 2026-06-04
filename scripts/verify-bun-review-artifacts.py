#!/usr/bin/env python3
"""Verify that a UB review packet satisfies the Bun v0 artifact contract."""

from __future__ import annotations

import argparse
import hashlib
import json
import pathlib
import re
import sys
from typing import Any


SENSORS = ["tokmd", "ripr", "unsafe-review", "ast-grep", "actionlint"]
BOX_FROM_ALLOCATION_FALSE_PREMISE_DEDUPE_KEY = "rust-box-from-allocation-failure"
APPROVAL_LINES = {
    "lgtm",
    "looks good",
    "clean",
    "solid",
    "no issues found",
    "no actionable findings",
    "no actionable",
}
SECRET_MARKERS = [
    "Authorization:",
    "Bearer ",
    "X-Api-Key:",
    "X-API-Key:",
    "github_token",
    "GITHUB_TOKEN",
    "UB_REVIEW_GITHUB_TOKEN",
    "UB_REVIEW_MINIMAX_API_KEY",
    "UB_REVIEW_OPENCODE_API_KEY",
]


def fail(message: str) -> None:
    print(f"verify-bun-review-artifacts: {message}", file=sys.stderr)
    raise SystemExit(1)


def read_text(path: pathlib.Path) -> str:
    try:
        return path.read_text(encoding="utf-8")
    except FileNotFoundError:
        fail(f"missing {path}")
    except UnicodeDecodeError as error:
        fail(f"invalid UTF-8 in {path}: {error}")


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
            "## Residual risk",
            "## Parked follow-ups",
            "## Evidence gaps",
            "## Missing evidence",
        ]
    )


def require_pr_review_body_policy(body: str, path: pathlib.Path) -> None:
    lowered = body.lower()
    for phrase in [
        "no blocking finding after",
        "no blocking ub finding",
        "no actionable findings",
        "a human should still inspect",
        "lane transcript",
        "raw observations",
    ]:
        if phrase in lowered:
            fail(f"{path} contains artifact-only boilerplate: {phrase!r}")
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
        "plan.json",
        "resolved-profile.json",
        "resolved-plan.json",
        "running-summary.md",
        "review/shared_context.md",
        "review/shared_context_cache_block.md",
        "review/shared_context_hash.txt",
        "review/cache_manifest.json",
        "review/cache_events.ndjson",
        "review/pr_thread_context.json",
        "review/terminal_state.json",
        "review/provider-preflight-status.json",
        "review/metrics.json",
        "review/review.json",
        "review/review.md",
        "review/observations.json",
        "review/unique_observations.json",
        "review/merged_observations.json",
        "review/dropped_observations.json",
        "review/orchestrator_plan.json",
        "review/follow_up_results.json",
        "review/follow_up_outputs.json",
        "review/follow_up_evidence.json",
        "review/witnesses.json",
        "review/witness_registry.json",
        "review/proof_requests.json",
        "review/proof_planner_input.json",
        "review/proof_planner_output.json",
        "review/proof_request_groups.json",
        "review/proof_receipts.json",
        "review/proof_plan.md",
        "review/resource_leases.json",
        "review/resource_plan.md",
        "follow_up_questions.ndjson",
        "follow_up_results.ndjson",
        "follow_up_outputs.ndjson",
        "witnesses.ndjson",
        "proof_requests.ndjson",
        "proof_tasks.ndjson",
        "proof_receipts.ndjson",
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

    plan = load_json(root / "plan.json")
    if not isinstance(plan, dict):
        fail("plan.json is not an object")
    lanes = plan.get("lanes")
    if not isinstance(lanes, list):
        fail("plan.json lanes is not an array")
    for lane_item in lanes:
        if not isinstance(lane_item, dict):
            fail(f"plan.json lane is not an object: {lane_item!r}")
        lane = lane_item.get("id")
        if not isinstance(lane, str) or not lane:
            fail(f"plan.json lane missing id: {lane_item!r}")
        lane_path = require_file(root / "lanes" / f"{sanitize_artifact_name(lane)}.md")
        lane_text = read_text(lane_path)
        if f"[{lane}]" not in lane_text:
            fail(f"lane packet {lane_path} does not include [{lane}] prefix")
        no_standalone_approval_line(lane_text, lane_path)


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
    for required in ["run_started", "run_finished"]:
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


def require_profile_artifacts(root: pathlib.Path) -> tuple[dict, dict]:
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
    if resolved_profile.get("selected_review_profile") != "bun-ub-v0":
        fail("resolved-profile.json selected_review_profile is not bun-ub-v0")
    review_profile = resolved_profile.get("review_profile")
    if not isinstance(review_profile, dict):
        fail("resolved-profile.json review_profile is not an object")
    if review_profile.get("name") != "bun-ub-v0":
        fail("resolved-profile.json review_profile.name is not bun-ub-v0")
    if review_profile.get("repo_kind") != "bun":
        fail("resolved-profile.json review_profile.repo_kind is not bun")
    runtime_profile = resolved_profile.get("selected_runtime_profile")
    if not isinstance(runtime_profile, str) or not runtime_profile:
        fail("resolved-profile.json selected_runtime_profile is invalid")
    if resolved_plan.get("review_profile") != "bun-ub-v0":
        fail("resolved-plan.json review_profile is not bun-ub-v0")
    if resolved_plan.get("runtime_profile") != runtime_profile:
        fail("resolved-plan.json runtime_profile does not match resolved-profile.json")
    return resolved_profile, resolved_plan


def require_review(root: pathlib.Path, max_inline_comments: int | None) -> dict:
    review = load_json(root / "review/review.json")
    review_body = read_text(root / "review/review.md")
    shared_context = read_text(root / "review/shared_context.md")

    shared_context_id = review.get("shared_context_id")
    if not isinstance(shared_context_id, str) or not re.fullmatch(
        r"[0-9a-f]{64}", shared_context_id
    ):
        fail("review.json shared_context_id is not a 64-character hex digest")
    if review.get("mode") != "review-direct":
        fail(f"review.json mode expected review-direct, got {review.get('mode')!r}")
    if review.get("review_profile") != "bun-ub-v0":
        fail(
            "review.json review_profile expected bun-ub-v0, "
            f"got {review.get('review_profile')!r}"
        )
    if review.get("posting") not in {"review", "artifact-only"}:
        fail(f"review.json posting has unexpected value {review.get('posting')!r}")
    if not isinstance(review.get("model_lanes"), list):
        fail("review.json model_lanes is not an array")
    if "## UB Ledger Context" not in shared_context:
        fail("shared_context.md missing UB ledger context section")
    if "## PR Thread Context" not in shared_context:
        fail("shared_context.md missing PR thread context section")
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
        require_pr_review_body_policy(body, github_review_path)
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
        if skip.get("review_payload_status") != "skipped_empty_smoke":
            fail(
                "github-review-skip.json review_payload_status expected skipped_empty_smoke"
            )
        if skip.get("terminal_state") != review.get("terminal_state", {}).get("status"):
            fail("github-review-skip.json terminal_state does not match review.json")

    return review


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


def require_metrics(root: pathlib.Path, review: dict) -> dict:
    metrics = load_json(root / "review/metrics.json")
    if metrics.get("schema_version") != 1:
        fail(f"metrics schema_version expected 1, got {metrics.get('schema_version')!r}")
    if metrics.get("shared_context_id") != review.get("shared_context_id"):
        fail("metrics shared_context_id does not match review.json")
    require_run_loop_metrics(metrics)
    if metrics.get("mode") != review.get("mode"):
        fail("metrics mode does not match review.json")
    if metrics.get("review_profile") != review.get("review_profile"):
        fail("metrics review_profile does not match review.json")
    if metrics.get("provider_policy") != review.get("provider_policy"):
        fail("metrics provider_policy does not match review.json")
    if metrics.get("inline_comments") != len(review.get("inline_comments", [])):
        fail("metrics inline_comments does not match review.json")
    if metrics.get("summary_only_findings") != len(review.get("summary_only_findings", [])):
        fail("metrics summary_only_findings does not match review.json")
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
    orchestrator_plan = load_json(root / "review/orchestrator_plan.json")
    follow_up_results = require_follow_up_results(root, orchestrator_plan["follow_up_tasks"])
    follow_up_outputs = require_follow_up_outputs(root, follow_up_results)
    follow_up_evidence = require_follow_up_evidence(root, follow_up_outputs)
    require_witness_artifacts(root, follow_up_evidence)
    require_follow_up_result_metrics(metrics, follow_up_results)
    require_observation_files(root, observations, orchestrator_plan["follow_up_tasks"])
    if (root / "review/github-review-skip.json").exists():
        if metrics.get("review_payload_status") != "skipped_empty_smoke":
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
    if run.get("concurrency_model") != "instrumented-sequential-v0":
        fail(f"metrics.run.concurrency_model is invalid: {run.get('concurrency_model')!r}")
    if run.get("local_proof_wall_excludes_model_wait") is not True:
        fail("metrics.run.local_proof_wall_excludes_model_wait must be true")
    for field in [
        "model_wall_ms",
        "local_proof_wall_ms",
        "compiler_wall_ms",
        "model_call_duration_ms_sum",
        "proof_command_duration_ms_sum",
        "model_proof_overlap_ms",
    ]:
        require_non_negative_int(run, f"metrics.run.{field}", field)
    loops = run.get("loops")
    if not isinstance(loops, dict):
        fail("metrics.run.loops is missing")
    for loop_name in ["model", "proof", "compiler"]:
        loop = loops.get(loop_name)
        if not isinstance(loop, dict):
            fail(f"metrics.run.loops.{loop_name} is missing")
        started = require_non_negative_int(
            loop, f"metrics.run.loops.{loop_name}.started_at_offset_ms", "started_at_offset_ms"
        )
        finished = require_non_negative_int(
            loop, f"metrics.run.loops.{loop_name}.finished_at_offset_ms", "finished_at_offset_ms"
        )
        wall = require_non_negative_int(
            loop, f"metrics.run.loops.{loop_name}.wall_ms", "wall_ms"
        )
        if finished < started:
            fail(f"metrics.run.loops.{loop_name} finished before it started")
        if wall > finished - started and finished > started:
            fail(f"metrics.run.loops.{loop_name} wall exceeds observed span")


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
    if not candidate_dir.is_dir():
        fail("missing candidates directory")
    expected_files = {
        f"{sanitize_artifact_name(candidate['id'])}.json": candidate
        for candidate in candidates
    }
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
    plan = load_json(root / "review/orchestrator_plan.json")
    expected = expected_orchestrator_plan(candidates, observations, proof_receipts, resource_leases)
    if plan != expected:
        fail("review/orchestrator_plan.json does not match candidate/observation evidence routing")

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
    return observation["dedupe_key"].startswith("lane-output-shape") or observation[
        "dedupe_key"
    ].startswith("lane-output-malformed-content")


def is_missing_evidence_observation(observation: dict) -> bool:
    return observation["kind"] == "missing-evidence"


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
    if result == "non_discriminating":
        return "residual-risk"
    if result in {"base_patch_failed", "timed_out", "skipped_budget", "skipped_profile"}:
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


def require_proof_task_schema(task: dict) -> None:
    if not isinstance(task, dict):
        fail(f"proof task is not an object: {task!r}")
    if task.get("schema") != "ub-review.proof_task.v1":
        fail(f"proof task has wrong schema: {task!r}")
    for field in ["id", "kind", "mode", "command", "purpose", "value", "cost", "status"]:
        if not isinstance(task.get(field), str) or not task[field]:
            fail(f"proof task missing string field {field}: {task!r}")
    timeout = task.get("timeout_sec")
    if not isinstance(timeout, int) or isinstance(timeout, bool) or timeout < 0:
        fail(f"proof task timeout_sec is invalid: {task!r}")
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


def require_proof_request_files(root: pathlib.Path, proof_requests: list[dict]) -> None:
    proof_request_dir = root / "proof_requests"
    if not proof_request_dir.is_dir():
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
        if lease.get("kind") != "focused-test":
            continue
        consumer = lease["consumer"]
        if consumer in focused_leases:
            fail(f"duplicate focused-test lease consumer: {consumer}")
        focused_leases[consumer] = lease
    focused_receipts = [
        receipt
        for receipt in proof_receipts
        if receipt.get("kind") in {"focused-head", "focused-red-green"}
    ]
    for receipt in focused_receipts:
        lease = focused_leases.get(receipt["id"])
        if lease is None:
            fail(f"focused proof receipt lacks resource lease: {receipt!r}")
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
        fail("review/proof_request_groups.json does not match raw proof request grouping")
    for group in groups:
        require_proof_request_group_schema(group)


def expected_proof_request_groups(proof_requests: list[dict]) -> list[dict]:
    groups: dict[tuple[str, str, int], dict] = {}
    for request in proof_requests:
        command = request["command"]
        cost = request["cost"]
        timeout_sec = request["timeout_sec"]
        key = (command, cost, timeout_sec)
        group = groups.get(key)
        if group is None:
            digest = hashlib.sha256(f"{command}\n{cost}\n{timeout_sec}".encode()).hexdigest()
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
    for field in ["task_id", "group_id", "stage", "model_lane", "status", "reason"]:
        if not isinstance(output.get(field), str) or not output[field]:
            fail(f"follow-up output missing string field {field}: {output!r}")
    for field in ["task_id", "group_id", "stage", "model_lane", "status", "reason"]:
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
    if receipt["kind"] not in {"focused-head", "focused-red-green"}:
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
    if lease["kind"] != "focused-test":
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
    return "".join(ch if ch.isalnum() or ch in "-_" else "-" for ch in value)


def require_sensor_receipts(root: pathlib.Path) -> None:
    for sensor in SENSORS:
        receipt = load_json(root / "sensors" / sensor / "ub-review-sensor-status.json")
        if receipt.get("sensor") != sensor:
            fail(f"{sensor} receipt has wrong sensor id {receipt.get('sensor')!r}")
        if receipt.get("status") not in {"ok", "missing", "skipped", "failed", "timed_out"}:
            fail(f"{sensor} receipt has unsupported status {receipt.get('status')!r}")
        if "reason" not in receipt:
            fail(f"{sensor} receipt missing reason")


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
                and skip.get("review_payload_status") == "skipped_empty_smoke"
            ):
                return
        fail("neither post-result.json nor post-error.json exists")
    if post_result.exists():
        receipt = load_json(post_result)
        if receipt.get("status") == "skipped":
            if receipt.get("review_payload_status") != "skipped_empty_smoke":
                fail("post-result.json skipped receipt has wrong review_payload_status")
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
        for marker in SECRET_MARKERS:
            if marker in text:
                fail(f"secret marker {marker!r} found in {path}")


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("root", nargs="?", default="target/ub-review")
    parser.add_argument("--min-ok-model-lanes", type=int, default=0)
    parser.add_argument("--max-inline-comments", type=int)
    parser.add_argument("--require-no-model-evidence-failures", action="store_true")
    return parser.parse_args(argv[1:])


def main(argv: list[str]) -> int:
    args = parse_args(argv)
    root = pathlib.Path(args.root)
    if not root.is_dir():
        fail(f"artifact root is not a directory: {root}")

    require_common_tree(root)
    require_summary(root)
    require_profile_artifacts(root)
    require_sensor_receipts(root)
    review = require_review(root, args.max_inline_comments)
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
        "Bun review artifact contract verified: "
        f"root={root} "
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
