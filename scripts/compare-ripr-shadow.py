#!/usr/bin/env python3
"""Compare released and exact-source RIPR on one frozen revision.

Evidence only: this script never changes required analyzer or gate authority.
"""
from __future__ import annotations

import argparse
from collections import Counter, defaultdict
import gc
import hashlib
import json
from pathlib import Path
import tempfile
from typing import Any

SCHEMA = "ub-review.ripr_candidate_shadow.v1"
PROJECTION_SCHEMA = "ub-review.ripr_semantic_projection.v1"
GAPS = {"weakly_exposed", "reachable_unrevealed", "no_static_path"}
MAX_INPUT_BYTES = 512 * 1024 * 1024
MAX_FINDINGS = 100_000
MAX_OUTPUT_BYTES = 8 * 1024 * 1024


def canonical(value: Any) -> bytes:
    return (
        json.dumps(
            value,
            sort_keys=True,
            separators=(",", ":"),
            ensure_ascii=False,
            allow_nan=False,
        )
        + "\n"
    ).encode()


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        while chunk := stream.read(1024 * 1024):
            digest.update(chunk)
    return digest.hexdigest()


def text(value: Any, label: str) -> str:
    if not isinstance(value, str) or not value.strip():
        raise ValueError(f"{label} must be a nonempty string")
    return value


def norm_path(value: str) -> str:
    value = value.replace("\\", "/")
    while value.startswith("./"):
        value = value[2:]
    return value


def test_path(value: str) -> bool:
    path = f"/{norm_path(value).lower()}"
    name = path.rsplit("/", 1)[-1]
    return (
        "/tests/" in path
        or "/test/" in path
        or name.endswith(("_test.rs", "_tests.rs"))
        or name == "tests.rs"
    )


def semantic_key(row: dict[str, Any]) -> str:
    return hashlib.sha256(
        canonical(
            {
                key: row[key]
                for key in ("path", "line", "family", "expression")
            }
        )
    ).hexdigest()


def project(path: Path, label: str) -> dict[str, Any]:
    size = path.stat().st_size
    if not 0 < size <= MAX_INPUT_BYTES:
        raise ValueError(
            f"{label} output size {size} is outside the bounded input contract"
        )
    digest = sha256_file(path)
    with path.open(encoding="utf-8") as stream:
        value = json.load(stream)
    if (
        not isinstance(value, dict)
        or value.get("tool") != "ripr"
        or value.get("mode") != "ready"
    ):
        raise ValueError(f"{label} is not a RIPR ready-mode object")
    schema = text(value.get("schema_version"), f"{label}.schema_version")
    findings = value.get("findings")
    summary = value.get("summary")
    if (
        not isinstance(findings, list)
        or len(findings) > MAX_FINDINGS
        or not isinstance(summary, dict)
    ):
        raise ValueError(f"{label} findings/summary contract is invalid")
    if summary.get("findings") != len(findings):
        raise ValueError(
            f"{label} summary.findings disagrees with the findings array"
        )

    ids: set[str] = set()
    rows: list[dict[str, Any]] = []
    gaps: list[dict[str, Any]] = []
    classes: Counter[str] = Counter()
    families: Counter[str] = Counter()
    paths: Counter[str] = Counter()
    gap_families: Counter[str] = Counter()
    gap_paths: Counter[str] = Counter()

    for index, finding in enumerate(findings):
        if not isinstance(finding, dict) or not isinstance(
            finding.get("probe"), dict
        ):
            raise ValueError(f"{label}.findings[{index}] is malformed")
        probe = finding["probe"]
        finding_id = text(
            finding.get("id"), f"{label}.findings[{index}].id"
        )
        if finding_id in ids:
            raise ValueError(f"{label} contains duplicate finding id {finding_id}")
        ids.add(finding_id)
        classification = text(
            finding.get("classification"),
            f"{label}.findings[{index}].classification",
        )
        file = norm_path(
            text(probe.get("file"), f"{label}.findings[{index}].probe.file")
        )
        line = probe.get("line")
        if type(line) is not int or line <= 0:
            raise ValueError(f"{label}.findings[{index}].probe.line is invalid")
        row = {
            "id": finding_id,
            "classification": classification,
            "path": file,
            "line": line,
            "family": text(
                probe.get("family"),
                f"{label}.findings[{index}].probe.family",
            ),
            "expression": text(
                probe.get("expression"),
                f"{label}.findings[{index}].probe.expression",
            ),
        }
        row["semantic_key"] = semantic_key(row)
        rows.append(row)
        classes[classification] += 1
        families[row["family"]] += 1
        paths[file] += 1
        if classification in GAPS:
            gap = dict(row)
            gap["test_path"] = test_path(file)
            gaps.append(gap)
            gap_families[row["family"]] += 1
            gap_paths[file] += 1

    rows.sort(
        key=lambda row: (
            row["path"],
            row["line"],
            row["family"],
            row["id"],
        )
    )
    gaps.sort(
        key=lambda row: (
            row["path"],
            row["line"],
            row["family"],
            row["id"],
        )
    )
    test_gaps = sum(bool(row["test_path"]) for row in gaps)
    identity_digest = hashlib.sha256(
        canonical(
            [
                (row["id"], row["classification"], row["semantic_key"])
                for row in rows
            ]
        )
    ).hexdigest()
    semantic_digest = hashlib.sha256(
        canonical(
            [(row["classification"], row["semantic_key"]) for row in rows]
        )
    ).hexdigest()
    result = {
        "schema": PROJECTION_SCHEMA,
        "label": label,
        "source": {
            "bytes": size,
            "sha256": digest,
            "ripr_schema_version": schema,
            "root": value.get("root"),
            "base": value.get("base"),
        },
        "summary": {
            "finding_count": len(rows),
            "gap_count": len(gaps),
            "test_path_gap_count": test_gaps,
            "production_path_gap_count": len(gaps) - test_gaps,
            "classification_counts": dict(sorted(classes.items())),
            "family_counts": dict(sorted(families.items())),
            "path_counts": dict(sorted(paths.items())),
            "gap_family_counts": dict(sorted(gap_families.items())),
            "gap_path_counts": dict(sorted(gap_paths.items())),
            "reported_summary": summary,
        },
        "identity_digest": identity_digest,
        "semantic_digest": semantic_digest,
        "findings": [
            {
                key: row[key]
                for key in (
                    "id",
                    "classification",
                    "semantic_key",
                    "path",
                    "line",
                    "family",
                )
            }
            for row in rows
        ],
        "gaps": [
            {
                key: row[key]
                for key in (
                    "id",
                    "classification",
                    "semantic_key",
                    "path",
                    "line",
                    "family",
                    "expression",
                    "test_path",
                )
            }
            for row in gaps
        ],
    }
    del value, findings, rows, gaps
    gc.collect()
    return result


def grouped(
    rows: list[dict[str, Any]], field: str
) -> dict[str, list[dict[str, Any]]]:
    result: dict[str, list[dict[str, Any]]] = defaultdict(list)
    for row in rows:
        result[row[field]].append(row)
    return result


def compare(
    stable: dict[str, Any],
    candidate: dict[str, Any],
    metadata: dict[str, Any],
) -> dict[str, Any]:
    stable_rows = stable["findings"]
    candidate_rows = candidate["findings"]
    stable_ids = {row["id"]: row for row in stable_rows}
    candidate_ids = {row["id"]: row for row in candidate_rows}
    stable_semantic = grouped(stable_rows, "semantic_key")
    candidate_semantic = grouped(candidate_rows, "semantic_key")
    class_changes: list[dict[str, Any]] = []
    identity_changes: list[dict[str, Any]] = []
    for key in sorted(set(stable_semantic) & set(candidate_semantic)):
        left = stable_semantic[key]
        right = candidate_semantic[key]
        left_classes = sorted(row["classification"] for row in left)
        right_classes = sorted(row["classification"] for row in right)
        left_ids = sorted(row["id"] for row in left)
        right_ids = sorted(row["id"] for row in right)
        sample = right[0]
        common = {
            "semantic_key": key,
            "path": sample["path"],
            "line": sample["line"],
            "family": sample["family"],
        }
        if left_classes != right_classes:
            class_changes.append(
                {
                    **common,
                    "stable_classifications": left_classes,
                    "candidate_classifications": right_classes,
                    "stable_ids": left_ids,
                    "candidate_ids": right_ids,
                }
            )
        elif left_ids != right_ids:
            identity_changes.append(
                {
                    **common,
                    "classification": right_classes,
                    "stable_ids": left_ids,
                    "candidate_ids": right_ids,
                }
            )

    stable_gap = stable["summary"]["gap_count"]
    candidate_gap = candidate["summary"]["gap_count"]
    return {
        "schema": SCHEMA,
        "authority": "shadow-only",
        "decision": "comparison_only_no_promotion",
        "frozen_input": metadata["frozen_input"],
        "stable": {**metadata["stable"], "projection": stable},
        "candidate": {**metadata["candidate"], "projection": candidate},
        "comparison": {
            "schema_equal": stable["source"]["ripr_schema_version"]
            == candidate["source"]["ripr_schema_version"],
            "finding_count_delta": candidate["summary"]["finding_count"]
            - stable["summary"]["finding_count"],
            "gap_count_delta": candidate_gap - stable_gap,
            "stable_gap_count": stable_gap,
            "candidate_gap_count": candidate_gap,
            "candidate_zero_gap": candidate_gap == 0,
            "candidate_reduces_gap_count": candidate_gap < stable_gap,
            "stable_test_path_gap_count": stable["summary"][
                "test_path_gap_count"
            ],
            "candidate_test_path_gap_count": candidate["summary"][
                "test_path_gap_count"
            ],
            "stable_production_path_gap_count": stable["summary"][
                "production_path_gap_count"
            ],
            "candidate_production_path_gap_count": candidate["summary"][
                "production_path_gap_count"
            ],
            "stable_only_ids": sorted(set(stable_ids) - set(candidate_ids)),
            "candidate_only_ids": sorted(set(candidate_ids) - set(stable_ids)),
            "stable_only_semantic_keys": sorted(
                set(stable_semantic) - set(candidate_semantic)
            ),
            "candidate_only_semantic_keys": sorted(
                set(candidate_semantic) - set(stable_semantic)
            ),
            "classification_changes_by_semantic_key": class_changes,
            "identity_changes_for_equal_semantics": identity_changes,
        },
        "claim_boundary": [
            "The released stable analyzer remains required authority.",
            "The exact-source candidate is shadow evidence only.",
            "A lower gap count does not authorize promotion.",
            "Public immutable release, compatibility acceptance, migration proof, and rollback remain required.",
        ],
    }


def markdown(report: dict[str, Any]) -> str:
    comparison = report["comparison"]
    stable = report["stable"]["projection"]["summary"]
    candidate = report["candidate"]["projection"]["summary"]
    return "\n".join(
        [
            "# RIPR candidate shadow comparison",
            "",
            f"- Authority: `{report['authority']}`",
            f"- Decision: `{report['decision']}`",
            f"- Subject base: `{report['frozen_input']['base_commit']}`",
            f"- Subject head: `{report['frozen_input']['head_commit']}`",
            f"- Subject tree: `{report['frozen_input']['head_tree']}`",
            f"- Diff SHA-256: `{report['frozen_input']['diff_sha256']}`",
            "",
            "| Metric | Released stable | Exact-source candidate | Delta |",
            "|---|---:|---:|---:|",
            f"| Findings | {stable['finding_count']} | {candidate['finding_count']} | {comparison['finding_count_delta']:+d} |",
            f"| Exposure gaps | {stable['gap_count']} | {candidate['gap_count']} | {comparison['gap_count_delta']:+d} |",
            f"| Test-path gaps | {stable['test_path_gap_count']} | {candidate['test_path_gap_count']} | {candidate['test_path_gap_count'] - stable['test_path_gap_count']:+d} |",
            f"| Production-path gaps | {stable['production_path_gap_count']} | {candidate['production_path_gap_count']} | {candidate['production_path_gap_count'] - stable['production_path_gap_count']:+d} |",
            "",
            f"Stable: `{report['stable']['source']}` / `{report['stable']['version_output'].strip()}` / exit `{report['stable']['exit_code']}`",
            f"Candidate: `{report['candidate']['source']}` / `{report['candidate']['version_output'].strip()}` / exit `{report['candidate']['exit_code']}`",
            "",
            "## Compatibility observations",
            "",
            f"- Schema equal: `{comparison['schema_equal']}`",
            f"- Candidate reaches zero gaps: `{comparison['candidate_zero_gap']}`",
            f"- Candidate reduces gaps: `{comparison['candidate_reduces_gap_count']}`",
            f"- Stable-only IDs: `{len(comparison['stable_only_ids'])}`",
            f"- Candidate-only IDs: `{len(comparison['candidate_only_ids'])}`",
            f"- Classification changes for equal semantic probes: `{len(comparison['classification_changes_by_semantic_key'])}`",
            f"- Identity changes for equal semantic probes: `{len(comparison['identity_changes_for_equal_semantics'])}`",
            "",
            "## Claim boundary",
            "",
            *[f"- {claim}" for claim in report["claim_boundary"]],
            "",
        ]
    )


def read_exit(path: Path) -> int:
    result = int(path.read_text().strip())
    if not 0 <= result <= 255:
        raise ValueError("process exit code is outside 0..255")
    return result


def write(path: Path, data: bytes) -> None:
    if len(data) > MAX_OUTPUT_BYTES:
        raise ValueError(f"{path.name} exceeds the bounded output contract")
    path.write_bytes(data)


def self_test() -> None:
    def fixture(classification: str, finding_id: str) -> dict[str, Any]:
        return {
            "schema_version": "0.2",
            "tool": "ripr",
            "mode": "ready",
            "root": "/repo",
            "base": "a" * 40,
            "summary": {"findings": 1},
            "findings": [
                {
                    "id": finding_id,
                    "classification": classification,
                    "probe": {
                        "file": "./src/value.rs",
                        "line": 7,
                        "family": "predicate",
                        "expression": "value == expected",
                    },
                }
            ],
        }

    with tempfile.TemporaryDirectory() as directory:
        root = Path(directory)
        for name, value in (
            ("stable", fixture("weakly_exposed", "old")),
            ("candidate", fixture("exposed", "new")),
        ):
            (root / f"{name}.json").write_text(json.dumps(value))
        report = compare(
            project(root / "stable.json", "stable"),
            project(root / "candidate.json", "candidate"),
            {
                "frozen_input": {
                    "base_commit": "1" * 40,
                    "head_commit": "2" * 40,
                    "head_tree": "3" * 40,
                    "diff_sha256": "4" * 64,
                },
                "stable": {
                    "source": "stable",
                    "version_output": "ripr 0.10.0",
                    "exit_code": 1,
                },
                "candidate": {
                    "source": "candidate",
                    "version_output": "ripr 0.11.0",
                    "exit_code": 0,
                },
            },
        )
        assert report["comparison"]["gap_count_delta"] == -1
        assert report["comparison"]["candidate_zero_gap"]
        assert (
            len(report["comparison"]["classification_changes_by_semantic_key"])
            == 1
        )
        assert "comparison_only_no_promotion" in markdown(report)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--self-test", action="store_true")
    for name in (
        "stable-json",
        "candidate-json",
        "stable-version-file",
        "candidate-version-file",
        "stable-exit-file",
        "candidate-exit-file",
        "output-dir",
    ):
        parser.add_argument(f"--{name}", type=Path)
    for name in (
        "subject-base",
        "subject-head",
        "subject-tree",
        "diff-sha256",
        "stable-source",
        "candidate-source",
    ):
        parser.add_argument(f"--{name}")
    args = parser.parse_args()
    if args.self_test:
        self_test()
        return 0
    required = [
        name.replace("-", "_")
        for name in (
            "stable-json",
            "candidate-json",
            "stable-version-file",
            "candidate-version-file",
            "stable-exit-file",
            "candidate-exit-file",
            "output-dir",
            "subject-base",
            "subject-head",
            "subject-tree",
            "diff-sha256",
            "stable-source",
            "candidate-source",
        )
    ]
    missing = [name for name in required if getattr(args, name) is None]
    if missing:
        parser.error("missing required arguments: " + ", ".join(missing))
    stable = project(args.stable_json, "stable")
    candidate = project(args.candidate_json, "candidate")
    report = compare(
        stable,
        candidate,
        {
            "frozen_input": {
                "repository": "EffortlessMetrics/ub-review",
                "base_commit": args.subject_base,
                "head_commit": args.subject_head,
                "head_tree": args.subject_tree,
                "diff_sha256": args.diff_sha256,
            },
            "stable": {
                "source": args.stable_source,
                "version_output": args.stable_version_file.read_text(),
                "exit_code": read_exit(args.stable_exit_file),
            },
            "candidate": {
                "source": args.candidate_source,
                "version_output": args.candidate_version_file.read_text(),
                "exit_code": read_exit(args.candidate_exit_file),
            },
        },
    )
    args.output_dir.mkdir(parents=True, exist_ok=True)
    for name, value in (
        ("stable-projection.json", stable),
        ("candidate-projection.json", candidate),
        ("comparison.json", report),
    ):
        write(args.output_dir / name, canonical(value))
    write(args.output_dir / "comparison.md", markdown(report).encode())
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
