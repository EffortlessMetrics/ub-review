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


def require_single_ok_preflight(preflights) -> None:
    if not isinstance(preflights, list):
        fail("provider preflight status is not a JSON array")
    if len(preflights) != 1:
        fail(f"expected exactly one provider preflight, got {len(preflights)}")
    receipt = preflights[0]
    expected = {
        "provider": "minimax",
        "model": "MiniMax-M3",
        "endpoint_kind": "openai-chat",
        "status": "ok",
        "reason": "completed",
        "http_status": 200,
        "response_shape": "openai",
    }
    for key, value in expected.items():
        if receipt.get(key) != value:
            fail(f"provider preflight `{key}` expected {value!r}, got {receipt.get(key)!r}")
    if receipt.get("duration_ms", 0) <= 0:
        fail("provider preflight duration_ms must be positive")


def require_single_ok_lane(lanes) -> dict:
    ok_lanes = [lane for lane in lanes if lane.get("status") == "ok"]
    if len(ok_lanes) != 1:
        fail(f"expected exactly one ok model lane, got {len(ok_lanes)}")
    lane = ok_lanes[0]
    expected = {
        "provider": "minimax",
        "model": "MiniMax-M3",
        "endpoint_kind": "openai-chat",
        "http_status": 200,
        "response_shape": "openai",
    }
    for key, value in expected.items():
        if lane.get(key) != value:
            fail(f"ok model lane `{key}` expected {value!r}, got {lane.get(key)!r}")
    if lane.get("duration_ms", 0) <= 0:
        fail("ok model lane duration_ms must be positive")
    if lane.get("fallback_from") is not None:
        fail("ok model lane unexpectedly used fallback_from")
    return lane


def require_metrics(metrics) -> None:
    models = metrics.get("models", {})
    expected = {
        "provider_preflights": 1,
        "provider_preflight_calls_attempted": 1,
        "model_lane_calls_attempted": 1,
        "model_fallbacks_used": 0,
    }
    for key, value in expected.items():
        if models.get(key) != value:
            fail(f"metrics.models.{key} expected {value!r}, got {models.get(key)!r}")
    if models.get("provider_preflight_status_counts", {}).get("ok") != 1:
        fail("metrics did not record exactly one ok provider preflight")
    if models.get("model_lane_status_counts", {}).get("ok") != 1:
        fail("metrics did not record exactly one ok model lane")


def require_ok_lane_artifacts(root: pathlib.Path, lane: dict) -> None:
    lane_id = lane.get("lane")
    if not isinstance(lane_id, str) or not lane_id:
        fail("ok model lane has no lane id")
    lane_dir = root / "review/model" / lane_id
    request_path = lane_dir / "request.json"
    response_path = lane_dir / "response.json"
    content_path = lane_dir / "content.json"
    stderr_path = lane_dir / "stderr.txt"

    request = load_json(request_path)
    response = load_json(response_path)
    content = load_json(content_path)
    stderr = stderr_path.read_text(encoding="utf-8")
    if stderr.strip():
        fail(f"model lane stderr is not empty: {stderr_path}")
    if request.get("model") != "MiniMax-M3":
        fail("model request did not use MiniMax-M3")
    if response.get("object") != "chat.completion":
        fail("model response is not an OpenAI chat completion")
    if response.get("model") != "MiniMax-M3":
        fail("model response did not report MiniMax-M3")
    choices = response.get("choices")
    if not isinstance(choices, list) or not choices:
        fail("model response has no choices")
    choice = choices[0]
    if choice.get("finish_reason") != "stop":
        fail(f"model finish_reason expected 'stop', got {choice.get('finish_reason')!r}")
    message = choice.get("message", {})
    assistant_content = message.get("content")
    if not isinstance(assistant_content, str) or not assistant_content.strip():
        fail("model assistant content is empty")
    try:
        parsed_content = json.loads(assistant_content)
    except json.JSONDecodeError as error:
        fail(f"assistant content is not strict JSON: {error}")
    if parsed_content != content:
        fail("content.json does not match parsed assistant content")
    usage = response.get("usage", {})
    for key in ["prompt_tokens", "completion_tokens", "total_tokens"]:
        if usage.get(key, 0) <= 0:
            fail(f"model response usage.{key} must be positive")


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
        "review/model/**/request.json",
        "review/provider-preflight/**/request.json",
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


def main(argv: list[str]) -> int:
    root = pathlib.Path(argv[1] if len(argv) > 1 else "target/ub-review-model-smoke")
    if not root.is_dir():
        fail(f"artifact root is not a directory: {root}")

    preflights = load_json(root / "review/provider-preflight-status.json")
    review = load_json(root / "review/review.json")
    metrics = load_json(root / "review/metrics.json")

    require_single_ok_preflight(preflights)
    ok_lane = require_single_ok_lane(review.get("model_lanes", []))
    require_metrics(metrics)
    require_ok_lane_artifacts(root, ok_lane)

    post_result = root / "review/post-result.json"
    post_error = root / "review/post-error.json"
    if not post_result.exists() and not post_error.exists():
        fail("neither post-result.json nor post-error.json exists")

    assert_no_auth_header_leak(root)

    print(
        "MiniMax smoke verified: "
        f"preflights={metrics['models']['provider_preflight_calls_attempted']} "
        f"lane_calls={metrics['models']['model_lane_calls_attempted']} "
        f"ok_lane={ok_lane['lane']}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
