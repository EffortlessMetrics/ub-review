#!/usr/bin/env python3
"""Verify that the MiniMax smoke artifact proves a live provider call."""

from __future__ import annotations

import json
import pathlib
import sys


def fail(message: str) -> None:
    print(f"verify-model-smoke-artifacts: {message}", file=sys.stderr)
    raise SystemExit(1)


def load_json(path: pathlib.Path):
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError:
        fail(f"missing {path}")
    except json.JSONDecodeError as error:
        fail(f"invalid JSON in {path}: {error}")


AUTH_HEADER_KEYS = {
    "authorization",
    "x-api-key",
    "x_api_key",
    "api-key",
    "api_key",
}

ENDPOINT_EXPECTATIONS = {
    "openai-chat": {
        "response_shape": "openai",
    },
    "anthropic-messages": {
        "response_shape": "anthropic",
    },
}


def endpoint_expectation(endpoint_kind: str) -> dict:
    expectation = ENDPOINT_EXPECTATIONS.get(endpoint_kind)
    if expectation is None:
        fail(f"unsupported endpoint_kind {endpoint_kind!r}")
    return expectation


def require_single_ok_preflight(preflights) -> dict:
    if not isinstance(preflights, list):
        fail("provider preflight status is not a JSON array")
    if len(preflights) != 1:
        fail(f"expected exactly one provider preflight, got {len(preflights)}")
    receipt = preflights[0]
    expected = {
        "provider": "minimax",
        "model": "MiniMax-M3",
        "status": "ok",
        "reason": "completed",
        "http_status": 200,
    }
    for key, value in expected.items():
        if receipt.get(key) != value:
            fail(f"provider preflight `{key}` expected {value!r}, got {receipt.get(key)!r}")
    endpoint_kind = receipt.get("endpoint_kind")
    expectation = endpoint_expectation(endpoint_kind)
    if receipt.get("response_shape") != expectation["response_shape"]:
        fail(
            "provider preflight `response_shape` expected "
            f"{expectation['response_shape']!r}, got {receipt.get('response_shape')!r}"
        )
    if receipt.get("duration_ms", 0) <= 0:
        fail("provider preflight duration_ms must be positive")
    return receipt


def require_ok_lanes(lanes, min_ok_lanes: int) -> list[dict]:
    ok_lanes = [
        lane
        for lane in lanes
        if lane.get("status") == "ok" and lane.get("lane") != "refuter"
    ]
    if len(ok_lanes) < min_ok_lanes:
        fail(f"expected at least {min_ok_lanes} ok model lane(s), got {len(ok_lanes)}")
    for lane in ok_lanes:
        require_ok_lane_receipt(lane)
    return ok_lanes


def require_ok_lane_receipt(lane: dict) -> None:
    expected = {
        "provider": "minimax",
        "model": "MiniMax-M3",
        "http_status": 200,
    }
    for key, value in expected.items():
        if lane.get(key) != value:
            fail(f"ok model lane `{key}` expected {value!r}, got {lane.get(key)!r}")
    endpoint_kind = lane.get("endpoint_kind")
    expectation = endpoint_expectation(endpoint_kind)
    if lane.get("response_shape") != expectation["response_shape"]:
        fail(
            "ok model lane `response_shape` expected "
            f"{expectation['response_shape']!r}, got {lane.get('response_shape')!r}"
        )
    if lane.get("duration_ms", 0) <= 0:
        fail("ok model lane duration_ms must be positive")
    if lane.get("fallback_from") is not None:
        fail("ok model lane unexpectedly used fallback_from")


def require_metrics(metrics, min_ok_lanes: int) -> None:
    run = metrics.get("run")
    if not isinstance(run, dict):
        fail("metrics.run is missing")
    if run.get("concurrency_model") != "instrumented-sequential-v0":
        fail("metrics.run.concurrency_model is invalid")
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
        value = run.get(field)
        if isinstance(value, bool) or not isinstance(value, int) or value < 0:
            fail(f"metrics.run.{field} is not a non-negative integer")
    models = metrics.get("models", {})
    expected = {
        "provider_preflights": 1,
        "provider_preflight_calls_attempted": 1,
        "model_fallbacks_used": 0,
    }
    for key, value in expected.items():
        if models.get(key) != value:
            fail(f"metrics.models.{key} expected {value!r}, got {models.get(key)!r}")
    if models.get("provider_preflight_status_counts", {}).get("ok") != 1:
        fail("metrics did not record exactly one ok provider preflight")
    if models.get("model_lane_calls_attempted", 0) < min_ok_lanes:
        fail(
            "metrics did not record enough model lane calls: "
            f"expected at least {min_ok_lanes}, got {models.get('model_lane_calls_attempted')!r}"
        )
    lane_status_counts = models.get("model_lane_status_counts", {})
    usable_lanes = lane_status_counts.get("ok", 0) + lane_status_counts.get("degraded", 0)
    if usable_lanes < min_ok_lanes:
        fail(
            "metrics did not record enough usable ok/degraded model lanes: "
            f"expected at least {min_ok_lanes}, got {usable_lanes!r}"
        )


def sanitize_artifact_name(value: str) -> str:
    return "".join(char if char.isalnum() or char in "-_" else "-" for char in value)


def require_preflight_artifacts(root: pathlib.Path, receipt: dict) -> None:
    label = sanitize_artifact_name(
        f"{receipt['provider']}:{receipt['model']}:{receipt['endpoint_kind']}"
    )
    content = require_model_call_artifacts(
        root / "review/provider-preflight" / label,
        "provider preflight",
        receipt["model"],
        receipt["endpoint_kind"],
    )
    expected_content = {
        "summary": "preflight ok",
        "inline_comments": [],
        "summary_only_findings": [],
    }
    if content != expected_content:
        fail(f"provider preflight content expected {expected_content!r}, got {content!r}")


def require_ok_lane_artifacts(root: pathlib.Path, lane: dict) -> None:
    lane_id = lane.get("lane")
    if not isinstance(lane_id, str) or not lane_id:
        fail("ok model lane has no lane id")
    require_model_call_artifacts(
        root / "review/model" / lane_id,
        f"model lane {lane_id}",
        "MiniMax-M3",
        lane["endpoint_kind"],
    )


def require_model_call_artifacts(
    call_dir: pathlib.Path, label: str, expected_model: str, endpoint_kind: str
) -> dict:
    request_path = call_dir / "request.json"
    response_path = call_dir / "response.json"
    content_path = call_dir / "content.json"
    stderr_path = call_dir / "stderr.txt"

    request = load_json(request_path)
    response = load_json(response_path)
    content = load_json(content_path)
    assert_no_auth_header_fields(request_path, request)
    assert_no_auth_header_fields(response_path, response)
    assert_no_auth_header_fields(content_path, content)
    stderr = stderr_path.read_text(encoding="utf-8")
    if stderr.strip():
        fail(f"{label} stderr is not empty: {stderr_path}")
    if request.get("model") != expected_model:
        fail(f"{label} request did not use {expected_model}")
    if endpoint_kind == "openai-chat":
        parsed_content = require_openai_response_content(response, label, expected_model)
    elif endpoint_kind == "anthropic-messages":
        parsed_content = require_anthropic_response_content(response, label, expected_model)
    else:
        fail(f"{label} has unsupported endpoint_kind {endpoint_kind!r}")
    if parsed_content != content:
        fail(f"{label} content.json does not match parsed assistant content")
    return content


def require_openai_response_content(
    response: dict, label: str, expected_model: str
) -> dict:
    if response.get("object") != "chat.completion":
        fail(f"{label} response is not an OpenAI chat completion")
    if response.get("model") != expected_model:
        fail(f"{label} response did not report {expected_model}")
    choices = response.get("choices")
    if not isinstance(choices, list) or not choices:
        fail(f"{label} response has no choices")
    choice = choices[0]
    if choice.get("finish_reason") != "stop":
        fail(
            f"{label} finish_reason expected 'stop', got {choice.get('finish_reason')!r}"
        )
    message = choice.get("message", {})
    assistant_content = message.get("content")
    if not isinstance(assistant_content, str) or not assistant_content.strip():
        fail(f"{label} assistant content is empty")
    try:
        parsed_content = json.loads(assistant_content)
    except json.JSONDecodeError as error:
        fail(f"{label} assistant content is not strict JSON: {error}")
    usage = response.get("usage", {})
    for key in ["prompt_tokens", "completion_tokens", "total_tokens"]:
        if usage.get(key, 0) <= 0:
            fail(f"{label} response usage.{key} must be positive")
    return parsed_content


def require_anthropic_response_content(
    response: dict, label: str, expected_model: str
) -> dict:
    if response.get("type") != "message":
        fail(f"{label} response is not an Anthropic message")
    if response.get("role") != "assistant":
        fail(f"{label} response role expected 'assistant', got {response.get('role')!r}")
    if response.get("model") != expected_model:
        fail(f"{label} response did not report {expected_model}")
    if response.get("stop_reason") != "end_turn":
        fail(
            f"{label} stop_reason expected 'end_turn', got {response.get('stop_reason')!r}"
        )
    content_blocks = response.get("content")
    if not isinstance(content_blocks, list) or not content_blocks:
        fail(f"{label} response has no content blocks")
    text_blocks = [
        block.get("text")
        for block in content_blocks
        if isinstance(block, dict) and block.get("type") == "text"
    ]
    assistant_content = next(
        (text for text in text_blocks if isinstance(text, str) and text.strip()), None
    )
    if assistant_content is None:
        fail(f"{label} assistant text content is empty")
    try:
        parsed_content = json.loads(assistant_content)
    except json.JSONDecodeError as error:
        fail(f"{label} assistant content is not strict JSON: {error}")
    usage = response.get("usage", {})
    for key in ["input_tokens", "output_tokens"]:
        if usage.get(key, 0) <= 0:
            fail(f"{label} response usage.{key} must be positive")
    return parsed_content


def assert_no_auth_header_fields(path: pathlib.Path, value) -> None:
    if isinstance(value, dict):
        for key, child in value.items():
            normalized_key = key.strip().lower()
            if normalized_key in AUTH_HEADER_KEYS:
                fail(f"auth header field `{key}` found in {path}")
            assert_no_auth_header_fields(path, child)
    elif isinstance(value, list):
        for child in value:
            assert_no_auth_header_fields(path, child)


def assert_no_auth_header_leak(root: pathlib.Path) -> None:
    needles = [
        "Authorization:",
        "Bearer ",
        "X-Api-Key:",
        "X-API-Key:",
        "github_token",
        "GITHUB_TOKEN",
        "UB_REVIEW_GITHUB_TOKEN",
    ]
    paths = []
    for pattern in [
        "review/model/**/stderr.txt",
        "review/provider-preflight/**/stderr.txt",
        "review/post-error.json",
        "review/post-result.json",
        "review/post-stdout.json",
        "review/post-stderr.txt",
    ]:
        paths.extend(root.glob(pattern))
    for path in paths:
        try:
            text = path.read_text(encoding="utf-8")
        except UnicodeDecodeError:
            continue
        for needle in needles:
            if needle in text:
                fail(f"auth header marker `{needle}` found in {path}")


def expected_min_ok_lanes(argv: list[str]) -> int:
    if len(argv) <= 2:
        return 1
    try:
        value = int(argv[2])
    except ValueError:
        fail(f"expected ok lane count must be an integer, got {argv[2]!r}")
    if value <= 0:
        fail(f"expected ok lane count must be positive, got {value}")
    return value


def main(argv: list[str]) -> int:
    root = pathlib.Path(argv[1] if len(argv) > 1 else "target/ub-review-model-smoke")
    min_ok_lanes = expected_min_ok_lanes(argv)
    if not root.is_dir():
        fail(f"artifact root is not a directory: {root}")

    preflights = load_json(root / "review/provider-preflight-status.json")
    review = load_json(root / "review/review.json")
    metrics = load_json(root / "review/metrics.json")

    preflight = require_single_ok_preflight(preflights)
    ok_lanes = require_ok_lanes(review.get("model_lanes", []), min_ok_lanes)
    require_metrics(metrics, min_ok_lanes)
    require_preflight_artifacts(root, preflight)
    for lane in ok_lanes:
        require_ok_lane_artifacts(root, lane)

    post_result = root / "review/post-result.json"
    post_error = root / "review/post-error.json"
    if not post_result.exists() and not post_error.exists():
        fail("neither post-result.json nor post-error.json exists")

    assert_no_auth_header_leak(root)

    print(
        "MiniMax smoke verified: "
        f"preflights={metrics['models']['provider_preflight_calls_attempted']} "
        f"lane_calls={metrics['models']['model_lane_calls_attempted']} "
        f"ok_lanes={','.join(lane['lane'] for lane in ok_lanes)}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
