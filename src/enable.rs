//! One-command adoption: `ub-review enable` writes a safe GitHub Actions
//! workflow + minimal `.ub-review.toml` for the chosen review posture, then
//! prints the exact secret to add. This is the Droid/Factory-style first-run
//! path — the complexity stays inside the tool (#721).

use crate::*;
use std::fs;

const REQUIRED_CHECK: &str = "ub-review/gate";
const WORKFLOW_RELATIVE_PATH: &str = ".github/workflows/ub-review.yml";
const CONFIG_RELATIVE_PATH: &str = ".ub-review.toml";
const RELEASE_BINARY_ASSET: &str = "ub-review-x86_64-unknown-linux-gnu.tar.gz";
/// The GitHub repo that publishes ub-review releases. Used to resolve the
/// latest release tag during `enable` so generated workflows download a binary
/// instead of source-building every run (#732).
const UBM_REPOSITORY: &str = "EffortlessMetrics/ub-review";

/// How the generated workflow installs ub-review. Release is the fast path
/// (download + sha256-verify a binary); Source is the fallback when no release
/// exists yet or the network is unavailable. Mirrors `action.yml`
/// `install-mode` (#732).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum InstallStrategy {
    /// Download and sha256-verify the release binary; pin `uses:` to the tag.
    Release { tag: String },
    /// Source-build from the pinned commit SHA (with `Swatinem/rust-cache`).
    Source { sha: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReleaseFallbackReason {
    RequestUnavailable,
    RequestFailed,
    MalformedResponse,
    InvalidTag,
    AssetsIncomplete,
}

impl ReleaseFallbackReason {
    fn description(self) -> &'static str {
        match self {
            ReleaseFallbackReason::RequestUnavailable => "release lookup unavailable",
            ReleaseFallbackReason::RequestFailed => "release request failed",
            ReleaseFallbackReason::MalformedResponse => "release response malformed",
            ReleaseFallbackReason::InvalidTag => "release tag invalid",
            ReleaseFallbackReason::AssetsIncomplete => "release assets incomplete",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ReleaseLookup {
    Installable { tag: String },
    Unavailable { reason: ReleaseFallbackReason },
}

impl InstallStrategy {
    /// Test/spec constructor for the source-build fallback path.
    #[cfg(test)]
    pub(crate) fn source(sha: &str) -> Self {
        InstallStrategy::Source {
            sha: sha.to_owned(),
        }
    }
}

/// A release tag is installable when it is `v<digit>...` — matching the
/// `^v[0-9]` guard `action.yml` uses to decide auto-mode release download.
/// Extracted from the resolver so the validation is unit-testable without a
/// network call.
fn is_installable_release_tag(tag: &str) -> bool {
    let tag = tag.trim();
    tag.starts_with('v') && tag.len() > 1 && tag.as_bytes()[1].is_ascii_digit()
}

/// Best-effort lookup of the latest ub-review release tag. Every unavailable
/// outcome is classified so the source fallback is explicit and testable.
/// Unauthenticated: the public API is reachable without a token, just rate
/// limited, and a missing release must not block first-run enablement.
fn resolve_latest_release_lookup() -> ReleaseLookup {
    let url = format!("https://api.github.com/repos/{UBM_REPOSITORY}/releases/latest");
    let output = std::process::Command::new("curl")
        .arg("-sS")
        .arg("--fail-with-body")
        .arg("--max-time")
        .arg("15")
        .arg("-H")
        .arg("Accept: application/vnd.github+json")
        .arg("-H")
        .arg("User-Agent: ub-review-enable")
        .arg(&url)
        .output()
        .map(|output| (output.status.success(), output.stdout))
        .map_err(|_| ReleaseFallbackReason::RequestUnavailable);
    classify_release_lookup_output(output)
}

fn classify_release_lookup_output(
    output: std::result::Result<(bool, Vec<u8>), ReleaseFallbackReason>,
) -> ReleaseLookup {
    let (success, stdout) = match output {
        Ok(output) => output,
        Err(reason) => return ReleaseLookup::Unavailable { reason },
    };
    if !success {
        return ReleaseLookup::Unavailable {
            reason: ReleaseFallbackReason::RequestFailed,
        };
    }
    let body: serde_json::Value = match serde_json::from_slice(&stdout) {
        Ok(body) => body,
        Err(_) => {
            return ReleaseLookup::Unavailable {
                reason: ReleaseFallbackReason::MalformedResponse,
            };
        }
    };
    classify_release_lookup_response(&body)
}

/// Classify a release response against the archive and checksum consumed by
/// `action.yml`'s release-install path. A tagged but incomplete release must
/// use the source fallback, not generate a workflow that fails on its first PR.
fn classify_release_lookup_response(body: &serde_json::Value) -> ReleaseLookup {
    let Some(tag) = body.get("tag_name").and_then(|value| value.as_str()) else {
        return ReleaseLookup::Unavailable {
            reason: ReleaseFallbackReason::InvalidTag,
        };
    };
    let tag = tag.trim();
    if !is_installable_release_tag(tag) {
        return ReleaseLookup::Unavailable {
            reason: ReleaseFallbackReason::InvalidTag,
        };
    }
    let checksum = format!("{RELEASE_BINARY_ASSET}.sha256");
    let Some(assets) = body.get("assets").and_then(|value| value.as_array()) else {
        return ReleaseLookup::Unavailable {
            reason: ReleaseFallbackReason::AssetsIncomplete,
        };
    };
    let has_archive = assets
        .iter()
        .filter_map(|asset| asset.get("name").and_then(|name| name.as_str()))
        .any(|name| name == RELEASE_BINARY_ASSET);
    let has_checksum = assets
        .iter()
        .filter_map(|asset| asset.get("name").and_then(|name| name.as_str()))
        .any(|name| name == checksum);
    if has_archive && has_checksum {
        ReleaseLookup::Installable {
            tag: tag.to_owned(),
        }
    } else {
        ReleaseLookup::Unavailable {
            reason: ReleaseFallbackReason::AssetsIncomplete,
        }
    }
}

/// Select the install strategy from a classified release lookup and an
/// optional, already-validated fallback pin. Source installation is impossible
/// without an explicit SHA; release lookup failure never invents a ref.
fn select_install_strategy(
    lookup: ReleaseLookup,
    action_sha: Option<&str>,
) -> Result<InstallStrategy> {
    match lookup {
        ReleaseLookup::Installable { tag } => Ok(InstallStrategy::Release { tag }),
        ReleaseLookup::Unavailable { reason } => {
            let sha = action_sha.ok_or_else(|| {
                anyhow::anyhow!(
                    "no installable ub-review release was resolvable ({}); pass --action-sha <40-hex-sha> to permit the cached source-build fallback",
                    reason.description()
                )
            })?;
            Ok(InstallStrategy::Source {
                sha: sha.to_owned(),
            })
        }
    }
}

/// Run `ub-review enable`. Validates any supplied fallback SHA, refuses to
/// overwrite existing files without `--force`, writes the workflow + minimal
/// config, and prints secret instructions.
pub(crate) fn cmd_enable(args: EnableArgs) -> Result<()> {
    cmd_enable_with_resolver(args, resolve_latest_release_lookup)
}

/// Resolver-injectable core so tests can run `enable` fully offline. The real
/// entrypoint (`cmd_enable`) wires in `resolve_latest_release_lookup`, which does
/// a best-effort live GitHub Releases lookup; tests pass a stub returning a fixed
/// `ReleaseLookup` so they never touch the network (determinism, per #732
/// self-review).
fn cmd_enable_with_resolver(args: EnableArgs, resolve: fn() -> ReleaseLookup) -> Result<()> {
    let action_sha = args
        .action_sha
        .as_deref()
        .map(validate_action_sha)
        .transpose()?;
    if !matches!(args.model.as_str(), "minimax") {
        bail!(
            "--model {} is not supported in v0; only `minimax` is available",
            args.model
        );
    }
    let root = &args.root;
    let workflow_path = root.join(WORKFLOW_RELATIVE_PATH);
    let config_path = root.join(CONFIG_RELATIVE_PATH);

    // Refuse to clobber an existing ub-review setup unless the user opted in
    // with --force. Detecting existing ub-review workflows by name (not just
    // the exact path) avoids leaving two competing workflows behind.
    if !args.force {
        refuse_existing_config(&config_path)?;
        refuse_existing_workflow(root)?;
    }

    let strategy = select_install_strategy(resolve(), action_sha)?;
    let workflow = render_enable_workflow(&strategy, args.mode);
    let config = render_enable_config();

    let workflow_dir = workflow_path.parent().ok_or_else(|| {
        anyhow::anyhow!("workflow path {} has no parent", workflow_path.display())
    })?;
    fs::create_dir_all(workflow_dir)
        .with_context(|| format!("create {}", workflow_dir.display()))?;
    fs::write(&workflow_path, &workflow)
        .with_context(|| format!("write {}", workflow_path.display()))?;
    fs::write(&config_path, &config).with_context(|| format!("write {}", config_path.display()))?;

    print_enable_summary(&workflow_path, &config_path, args.mode, &strategy);
    Ok(())
}

/// Reject anything that is not a full 40-character lowercase-or-uppercase hex
/// SHA. Mirrors `valid_setup_ci_action_sha` (`src/ci_audit.rs`): the generator
/// never invents a pin.
fn validate_action_sha(sha: &str) -> Result<&str> {
    let trimmed = sha.trim();
    if trimmed.len() == 40 && trimmed.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Ok(trimmed);
    }
    bail!(
        "--action-sha must be the full 40-hex ub-review commit to pin in the generated workflow; got `{sha}`"
    );
}

fn refuse_existing_config(config_path: &Path) -> Result<()> {
    if config_path.exists() {
        bail!(
            "{} already exists; re-run with --force to overwrite",
            config_path.display()
        );
    }
    Ok(())
}

/// Scan `.github/workflows/` and refuse if any workflow references the
/// ub-review action. This catches a hand-written workflow under a different
/// filename, not just the default `ub-review.yml`.
fn refuse_existing_workflow(root: &Path) -> Result<()> {
    for scan in scan_local_workflows(root)? {
        if scan.path.contains("ub-review") {
            bail!(
                "existing ub-review workflow found at {}; re-run with --force to overwrite",
                scan.path
            );
        }
    }
    Ok(())
}

/// The MiniMax-primary enable workflow. Distinct from `render_setup_ci_gate_workflow`
/// (which is deterministic-gate-only: `model-mode: off`, `posting:
/// artifact-only`). The enable template turns MiniMax review ON and uses the
/// `review-mode` preset (#720) so normal users never compose the internal
/// mode/fail-on-gate/review_forward knobs.
///
/// When a release is available (`InstallStrategy::Release`) the generated
/// workflow downloads + sha256-verifies the binary instead of source-building
/// (Ordered Program item 1: distribution first, source build is fallback
/// only). Without this, a SHA pin + `install-mode: auto` (the default) forces
/// a source build on every run even after a release exists (#732).
pub(crate) fn render_enable_workflow(strategy: &InstallStrategy, mode: ReviewModePreset) -> String {
    let mode_key = mode.key();
    let install_block = match strategy {
        InstallStrategy::Release { tag } => format!(
            r#"      # Release install: download + sha256-verify the ub-review binary
      # instead of source-building (~seconds vs ~12 min). install-mode=release
      # fails closed if the asset is missing; auto would silently fall back to
      # a source build. See action.yml and #732.
      - name: ub-review
        uses: {UBM_REPOSITORY}@{tag}
        with:
          install-mode: release
          release-version: {tag}
          review-mode: {mode_key}
          provider-policy: primary-with-fallback
          minimax-api-key: ${{{{ secrets.MINIMAX_API_KEY }}}}
          opencode-api-key: ${{{{ secrets.OPENCODE }}}}
          opencode-model: mimo-v2.5
          github-token: ${{{{ github.token }}}}
          root: .
          base: origin/${{{{ github.base_ref }}}}
          head: HEAD
          out: target/ub-review
          posting: review
"#
        ),
        InstallStrategy::Source { sha } => format!(
            r#"      # No ub-review release was resolvable at enable time, so this workflow
      # source-builds ub-review and caches the build (Swatinem/rust-cache,
      # keyed on the SHA). Re-run `ub-review enable` after a release ships to
      # switch to the fast binary-download path. See #732.
      - uses: Swatinem/rust-cache@v2
        with:
          workspaces: ". -> target/ub-review-build"
          key: ub-review-{sha}

      - name: ub-review
        env:
          CARGO_TARGET_DIR: target/ub-review-build
        uses: {UBM_REPOSITORY}@{sha}
        with:
          review-mode: {mode_key}
          provider-policy: primary-with-fallback
          minimax-api-key: ${{{{ secrets.MINIMAX_API_KEY }}}}
          opencode-api-key: ${{{{ secrets.OPENCODE }}}}
          opencode-model: mimo-v2.5
          github-token: ${{{{ github.token }}}}
          root: .
          base: origin/${{{{ github.base_ref }}}}
          head: HEAD
          out: target/ub-review
          posting: review
"#
        ),
    };
    format!(
        r#"name: ub-review

# Generated by `ub-review enable`. MiniMax reviews each PR with specialist
# lanes, with OpenCode available as an optional fallback. The team selects
# relevant PR-specific proof, runs it safely, and posts one review plus a CI
# gate result. See docs/QUICKSTART.md and docs/ADOPTION_MODES.md.
on:
  pull_request:
    types: [opened, reopened, ready_for_review, synchronize]

permissions:
  contents: read
  pull-requests: write
  checks: write

concurrency:
  group: ub-review-${{{{ github.event.pull_request.number || github.ref }}}}
  cancel-in-progress: true

jobs:
  ub-review:
    runs-on: ubuntu-latest
    timeout-minutes: 30
    steps:
      - uses: actions/checkout@v5
        with:
          fetch-depth: 0
          persist-credentials: false

{install_block}
      - name: Upload ub-review artifacts
        if: always()
        uses: actions/upload-artifact@v7
        with:
          name: ub-review-${{{{ github.event.pull_request.number || github.run_id }}}}
          path: target/ub-review
          if-no-files-found: warn
          retention-days: 7
"#
    )
}

/// The minimal `.ub-review.toml`. Deliberately NOT `Config::default()`, which
/// carries a hardcoded `repo.kind = "bun"` and a personal ledger path. The
/// enable config is generic; repos add proof/sensors/lanes later as the tool's
/// internal sophistication needs them.
pub(crate) fn render_enable_config() -> String {
    format!(
        r#"# ub-review config generated by `ub-review enable`.
# Repo-specific proof, sensors, and lanes can be added here later; this
# minimal config is enough for MiniMax review + the deterministic gate.

profile = "gh-runner"

[repo]
base = "origin/main"
head = "HEAD"

[gate]
required_check = "{REQUIRED_CHECK}"

# Post actionable findings (severity medium+ or confidence medium-high+) to
# the PR; suppress pure lane-status boilerplate. Without this, the default
# `suppress` policy posts nothing, so the acted-on-comment metric is
# structurally zero and a human can never cite a finding. See ub-review #717.
[review_body]
summary_only_body = "post_substantive"
"#
    )
}

fn render_enable_summary(
    workflow_path: &Path,
    config_path: &Path,
    mode: ReviewModePreset,
    strategy: &InstallStrategy,
) -> String {
    let (header, install_detail) = match strategy {
        InstallStrategy::Release { tag } => {
            (
                format!("ub-review enabled ({}, release {tag}).", mode.key()),
                format!(
                    "  The workflow downloads the ub-review {tag} binary and verifies its sha256,\n  so each run starts in seconds instead of source-building (~12 min)."
                ),
            )
        }
        InstallStrategy::Source { sha } => {
            (
                format!(
                    "ub-review enabled ({}, source-build pinned to {sha}).",
                    mode.key()
                ),
                "  No ub-review release was resolvable, so each run source-builds ub-review\n  (~12 min, cached). Re-run `ub-review enable` after a release ships to switch\n  to the fast binary-download path.".to_owned(),
            )
        }
    };
    let mode_detail = match mode {
        ReviewModePreset::Advisory => {
            "  MiniMax reviews and comments; the gate is non-required and never blocks."
        }
        ReviewModePreset::Gate => {
            "  MiniMax reviews + the deterministic-floor gate is the required check.\n  Make `ub-review/gate` required in branch protection when ready."
        }
        ReviewModePreset::Strict => {
            "  Gate + the reporter verdict (changes_requested/uncertain) can block.\n  Use only after calibration shows low false positives."
        }
    };
    format!(
        "{header}\n\n  wrote {}\n  wrote {}\n\n{install_detail}\n\nNext:\n  1. Add MINIMAX_API_KEY as a required repository secret:\n       repo Settings → Secrets and variables → Actions → New repository secret\n  2. Optionally add OPENCODE for provider fallback. Without it, MiniMax remains\n     operational but OpenCode fallback is unavailable.\n  3. Commit the two files and open a pull request.\n  4. ub-review will post a MiniMax review and a CI gate result on the PR.\n\nMode `{}`:\n{mode_detail}\n",
        workflow_path.display(),
        config_path.display(),
        mode.key(),
    )
}

fn print_enable_summary(
    workflow_path: &Path,
    config_path: &Path,
    mode: ReviewModePreset,
    strategy: &InstallStrategy,
) {
    print!(
        "{}",
        render_enable_summary(workflow_path, config_path, mode, strategy)
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic offline resolver stub for `cmd_enable`-level tests so they
    /// never hit the live GitHub Releases API (determinism, per #732
    /// self-review). Always resolves to the source-build path.
    fn offline_resolve_source() -> ReleaseLookup {
        ReleaseLookup::Unavailable {
            reason: ReleaseFallbackReason::RequestUnavailable,
        }
    }

    /// Offline resolver stub that always resolves a release tag, so the
    /// resolver-seam test can exercise the release-install path without a
    /// network call. A named `fn` keeps the injected resolver deterministic.
    fn offline_resolve_release() -> ReleaseLookup {
        ReleaseLookup::Installable {
            tag: "v9.9.9".to_owned(),
        }
    }

    #[test]
    fn enable_workflow_renders_review_mode_input() {
        for mode in [
            ReviewModePreset::Advisory,
            ReviewModePreset::Gate,
            ReviewModePreset::Strict,
        ] {
            let sha = "aa".repeat(20);
            let yaml = render_enable_workflow(&InstallStrategy::source(&sha), mode);
            assert!(
                yaml.contains(&format!("review-mode: {}", mode.key())),
                "workflow for {mode:?} must set review-mode"
            );
            assert!(
                yaml.contains(&format!("@{sha}")),
                "workflow must pin the action ref"
            );
        }
    }

    #[test]
    fn enable_workflow_caches_source_build() {
        let sha = "c".repeat(40);
        let yaml = render_enable_workflow(&InstallStrategy::source(&sha), ReviewModePreset::Gate);
        // The rust-cache step is critical for the source-build path: without it,
        // every run recompiles ~12 min of deps, making ub-review too slow to post
        // before human reviewers. The cache key must include the action SHA so a
        // new pin invalidates it.
        assert!(
            yaml.contains("Swatinem/rust-cache"),
            "source workflow must cache the source build"
        );
        assert!(
            yaml.contains(&format!("key: ub-review-{sha}")),
            "cache key must be keyed on the action SHA"
        );
        assert!(
            yaml.contains("CARGO_TARGET_DIR"),
            "source workflow must set CARGO_TARGET_DIR so the action reuses the cache"
        );
    }

    #[test]
    fn enable_workflow_release_path_downloads_binary_not_source_builds() {
        // Regression for #732: when a release is available the generated
        // workflow must install the binary, not source-build. Otherwise a SHA
        // pin + install-mode=auto (default) silently forces a source build on
        // every run even after a release ships.
        let tag = "v0.1.0";
        let yaml = render_enable_workflow(
            &InstallStrategy::Release {
                tag: tag.to_owned(),
            },
            ReviewModePreset::Gate,
        );
        assert!(
            yaml.contains("install-mode: release"),
            "release workflow must set install-mode: release"
        );
        assert!(
            yaml.contains(&format!("release-version: {tag}")),
            "release workflow must pin release-version to the tag"
        );
        assert!(
            yaml.contains(&format!("@{tag}")),
            "release workflow must pin uses@<tag>"
        );
        // The source-build cache is the fallback; it must NOT appear in the
        // release path (it would be dead weight and signal the wrong install).
        assert!(
            !yaml.contains("Swatinem/rust-cache"),
            "release workflow must not carry the source-build cache step"
        );
        assert!(
            !yaml.contains("CARGO_TARGET_DIR"),
            "release workflow must not set the source-build CARGO_TARGET_DIR"
        );
    }

    #[test]
    fn enable_workflow_release_version_and_uses_ref_agree() {
        // The release-version input and the uses@<ref> must name the same tag,
        // otherwise action.yml downloads a different binary than the pinned
        // action source (confusing and a supply-chain smell).
        for mode in [
            ReviewModePreset::Advisory,
            ReviewModePreset::Gate,
            ReviewModePreset::Strict,
        ] {
            let yaml = render_enable_workflow(
                &InstallStrategy::Release {
                    tag: "v0.2.3".to_owned(),
                },
                mode,
            );
            assert!(
                yaml.contains("release-version: v0.2.3") && yaml.contains("@v0.2.3"),
                "release-version and uses@ ref must both be the tag for {mode:?}"
            );
        }
    }

    #[test]
    fn enable_workflow_uses_minimax_key_not_env() {
        // Fork-safety must hold on BOTH install paths: the secret must only
        // appear inside `with:` inputs, never in an `env:` block, and never on
        // a pull_request_target trigger.
        for strategy in [
            InstallStrategy::Release {
                tag: "v0.1.0".to_owned(),
            },
            InstallStrategy::source(&"b".repeat(40)),
        ] {
            let yaml = render_enable_workflow(&strategy, ReviewModePreset::Gate);
            assert!(
                yaml.contains("${{ secrets.MINIMAX_API_KEY }}"),
                "workflow must reference the MINIMAX_API_KEY secret"
            );
            for line in yaml.lines() {
                if line.contains("secrets.MINIMAX_API_KEY") {
                    assert!(
                        line.contains("minimax-api-key:"),
                        "MINIMAX_API_KEY must only appear in the with: inputs, not env:; got: {line}"
                    );
                }
            }
            assert!(
                !yaml.contains("pull_request_target"),
                "workflow must not use pull_request_target (fork-safety)"
            );
            assert!(
                yaml.contains("persist-credentials: false"),
                "checkout must disable credential persistence (fork-safety)"
            );
        }
    }

    #[test]
    fn is_installable_release_tag_accepts_version_tags_only() {
        // Only v<digit> tags are installable: this mirrors action.yml's
        // ^v[0-9] auto-mode guard. A non-version tag (or a SHA) must NOT be
        // treated as a release, or the resolver would emit a broken workflow.
        assert!(is_installable_release_tag("v0.1.0"));
        assert!(is_installable_release_tag("v1.0.0-rc.1"));
        assert!(is_installable_release_tag(" v2.3 ")); // trimmed
        assert!(!is_installable_release_tag("latest"));
        assert!(!is_installable_release_tag("v")); // no digit after v
        assert!(!is_installable_release_tag("vx"));
        assert!(!is_installable_release_tag("vX1"));
        assert!(
            !is_installable_release_tag(&"a".repeat(40)),
            "a commit SHA must never be mistaken for a release tag"
        );
    }

    #[test]
    fn release_resolver_requires_the_action_archive_and_checksum() {
        assert!(
            include_str!("../action.yml").contains(&format!("default: {RELEASE_BINARY_ASSET}")),
            "enable's expected asset must match action.yml's release asset"
        );
        let complete = serde_json::json!({
            "tag_name": "v0.1.0",
            "assets": [
                { "name": RELEASE_BINARY_ASSET },
                { "name": format!("{RELEASE_BINARY_ASSET}.sha256") }
            ]
        });
        assert_eq!(
            classify_release_lookup_response(&complete),
            ReleaseLookup::Installable {
                tag: "v0.1.0".to_owned()
            }
        );

        for incomplete in [
            serde_json::json!({ "tag_name": "v0.1.0", "assets": [] }),
            serde_json::json!({
                "tag_name": "v0.1.0",
                "assets": [{ "name": RELEASE_BINARY_ASSET }]
            }),
            serde_json::json!({
                "tag_name": "v0.1.0",
                "assets": [{ "name": format!("{RELEASE_BINARY_ASSET}.sha256") }]
            }),
        ] {
            assert_eq!(
                classify_release_lookup_response(&incomplete),
                ReleaseLookup::Unavailable {
                    reason: ReleaseFallbackReason::AssetsIncomplete
                },
                "incomplete releases must use the source-build fallback"
            );
        }
    }

    #[test]
    fn release_lookup_classifies_unavailable_paths() {
        let unavailable =
            classify_release_lookup_output(Err(ReleaseFallbackReason::RequestUnavailable));
        assert_eq!(
            unavailable,
            ReleaseLookup::Unavailable {
                reason: ReleaseFallbackReason::RequestUnavailable
            }
        );
        assert_eq!(
            classify_release_lookup_output(Ok((false, Vec::new()))),
            ReleaseLookup::Unavailable {
                reason: ReleaseFallbackReason::RequestFailed
            }
        );
        assert_eq!(
            classify_release_lookup_output(Ok((true, b"{".to_vec()))),
            ReleaseLookup::Unavailable {
                reason: ReleaseFallbackReason::MalformedResponse
            }
        );
        let invalid_tag = serde_json::json!({
            "tag_name": "latest",
            "assets": [
                { "name": RELEASE_BINARY_ASSET },
                { "name": format!("{RELEASE_BINARY_ASSET}.sha256") }
            ]
        });
        assert_eq!(
            classify_release_lookup_response(&invalid_tag),
            ReleaseLookup::Unavailable {
                reason: ReleaseFallbackReason::InvalidTag
            }
        );

        let complete = serde_json::json!({
            "tag_name": "v0.1.0",
            "assets": [
                { "name": RELEASE_BINARY_ASSET },
                { "name": format!("{RELEASE_BINARY_ASSET}.sha256") }
            ]
        });
        assert_eq!(
            classify_release_lookup_output(Ok((true, complete.to_string().into_bytes()))),
            ReleaseLookup::Installable {
                tag: "v0.1.0".to_owned()
            },
            "successful response bytes must reach complete-asset classification"
        );
    }

    #[test]
    fn enable_summary_renders_install_and_mode_contracts() {
        let workflow = Path::new(".github/workflows/ub-review.yml");
        let config = Path::new(".ub-review.toml");
        for (strategy, install_phrase) in [
            (
                InstallStrategy::Release {
                    tag: "v0.1.0".to_owned(),
                },
                "downloads the ub-review v0.1.0 binary",
            ),
            (
                InstallStrategy::source(&"a".repeat(40)),
                "No ub-review release was resolvable",
            ),
        ] {
            for (mode, mode_phrase) in [
                (ReviewModePreset::Advisory, "never blocks"),
                (ReviewModePreset::Gate, "deterministic-floor gate"),
                (ReviewModePreset::Strict, "reporter verdict"),
            ] {
                let summary = render_enable_summary(workflow, config, mode, &strategy);
                assert!(summary.contains(install_phrase), "{mode:?}: {summary}");
                assert!(summary.contains(mode_phrase), "{mode:?}: {summary}");
                assert!(summary.contains(".github/workflows/ub-review.yml"));
                assert!(summary.contains(".ub-review.toml"));
            }
        }
    }

    #[test]
    fn enable_config_round_trips() -> Result<()> {
        let toml = render_enable_config();
        let config: Config = toml::from_str(&toml)?;
        assert_eq!(config.gate.required_check, REQUIRED_CHECK);
        assert_eq!(config.profile, "gh-runner");
        assert_eq!(config.repo.base, "origin/main");
        assert_eq!(config.repo.head, "HEAD");
        // post_substantive so actionable findings reach the PR (#717).
        assert_eq!(
            config.review_body.summary_only_body.key(),
            "post_substantive",
            "enable config must default to post_substantive so findings are actionable"
        );
        Ok(())
    }

    #[test]
    fn enable_config_is_not_the_bun_dogfood_default() {
        // The enable config must NOT carry the bun-dogfood defaults that
        // Config::default() ships (repo.kind="bun", personal ledger path).
        let toml = render_enable_config();
        assert!(
            !toml.contains("bun"),
            "enable config must not hardcode the bun repo kind"
        );
        assert!(
            !toml.contains("/home/steven"),
            "enable config must not carry the personal ledger path"
        );
    }

    #[test]
    fn enable_rejects_non_hex_action_sha() {
        // 39 hex chars (too short).
        assert!(validate_action_sha(&"a".repeat(39)).is_err());
        // 40 chars but non-hex.
        let mut non_hex = "z".repeat(40);
        non_hex = non_hex.replace('z', "g");
        let bad = format!("{non_hex}1"); // 41 chars, but test the 40-char non-hex path
        assert!(validate_action_sha(&bad[..40]).is_err());
        // 40 hex chars (valid).
        assert!(validate_action_sha(&"c".repeat(40)).is_ok());
        // Mixed case hex is accepted (GitHub SHAs are lowercase but tolerate both).
        assert!(validate_action_sha(&"D".repeat(40)).is_ok());
    }

    #[test]
    fn enable_refuses_overwrite_without_force() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let root = temp.path();
        let config_path = root.join(CONFIG_RELATIVE_PATH);
        let workflow_path = root.join(WORKFLOW_RELATIVE_PATH);
        let workflow_dir = workflow_path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("workflow path has no parent"))?;
        fs::create_dir_all(workflow_dir)?;

        // Pre-existing config without --force must refuse.
        fs::write(&config_path, "profile = \"gh-runner\"\n")?;
        let args = EnableArgs {
            mode: ReviewModePreset::Gate,
            model: "minimax".to_owned(),
            action_sha: Some("d".repeat(40)),
            root: root.to_path_buf(),
            force: false,
        };
        assert!(cmd_enable_with_resolver(args, offline_resolve_source).is_err());

        // With --force it proceeds even with the pre-existing config.
        fs::remove_file(&config_path)?; // reset
        fs::write(&config_path, "profile = \"gh-runner\"\n")?;
        let args = EnableArgs {
            mode: ReviewModePreset::Gate,
            model: "minimax".to_owned(),
            action_sha: Some("d".repeat(40)),
            root: root.to_path_buf(),
            force: true,
        };
        cmd_enable_with_resolver(args, offline_resolve_source)?;
        assert!(config_path.exists());
        assert!(workflow_path.exists());
        Ok(())
    }

    #[test]
    fn enable_refuses_existing_ub_review_workflow_by_name() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let root = temp.path();
        let workflows = root.join(".github/workflows");
        fs::create_dir_all(&workflows)?;
        // A hand-written ub-review workflow under a non-default filename.
        fs::write(
            workflows.join("custom-ub-review.yml"),
            "name: ci\non:\n  pull_request:\njobs:\n  x:\n    runs-on: ubuntu-latest\n",
        )?;
        let args = EnableArgs {
            mode: ReviewModePreset::Gate,
            model: "minimax".to_owned(),
            action_sha: Some("e".repeat(40)),
            root: root.to_path_buf(),
            force: false,
        };
        let err = match cmd_enable_with_resolver(args, offline_resolve_source) {
            Ok(()) => anyhow::bail!("should have refused a competing ub-review workflow"),
            Err(err) => err.to_string(),
        };
        assert!(
            err.contains("existing ub-review workflow"),
            "should refuse a competing ub-review workflow: {err}"
        );
        Ok(())
    }

    #[test]
    fn enable_rejects_unsupported_model() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let args = EnableArgs {
            mode: ReviewModePreset::Gate,
            model: "openai".to_owned(),
            action_sha: Some("f".repeat(40)),
            root: temp.path().to_path_buf(),
            force: false,
        };
        let err = match cmd_enable_with_resolver(args, offline_resolve_source) {
            Ok(()) => anyhow::bail!("should have rejected the unsupported openai model"),
            Err(err) => err.to_string(),
        };
        assert!(err.contains("not supported"), "should reject openai: {err}");
        Ok(())
    }

    #[test]
    fn cmd_enable_with_resolver_writes_release_workflow_offline() -> Result<()> {
        // The resolver seam is what lets cmd_enable run fully offline. Inject a
        // resolver that always resolves a release tag and confirm the written
        // workflow is the release-install path — no network involved.
        let temp = tempfile::tempdir()?;
        let root = temp.path();
        let args = EnableArgs {
            mode: ReviewModePreset::Gate,
            model: "minimax".to_owned(),
            action_sha: None,
            root: root.to_path_buf(),
            force: false,
        };
        let resolve = offline_resolve_release;
        cmd_enable_with_resolver(args, resolve)?;
        let workflow = fs::read_to_string(root.join(WORKFLOW_RELATIVE_PATH))?;
        assert!(
            workflow.contains("install-mode: release")
                && workflow.contains("release-version: v9.9.9")
                && workflow.contains("@v9.9.9"),
            "injected release resolver must produce a release-install workflow"
        );
        assert!(
            !workflow.contains("Swatinem/rust-cache"),
            "release workflow must not carry the source-build cache"
        );
        Ok(())
    }

    #[test]
    fn enable_requires_a_valid_sha_only_for_source_fallback() -> Result<()> {
        let fallback_sha = "a".repeat(40);
        let release = select_install_strategy(
            ReleaseLookup::Installable {
                tag: "v9.9.9".to_owned(),
            },
            Some(&fallback_sha),
        )?;
        anyhow::ensure!(
            release
                == InstallStrategy::Release {
                    tag: "v9.9.9".to_owned()
                }
        );

        for (reason, description) in [
            (
                ReleaseFallbackReason::RequestUnavailable,
                "release lookup unavailable",
            ),
            (
                ReleaseFallbackReason::RequestFailed,
                "release request failed",
            ),
            (
                ReleaseFallbackReason::MalformedResponse,
                "release response malformed",
            ),
            (ReleaseFallbackReason::InvalidTag, "release tag invalid"),
            (
                ReleaseFallbackReason::AssetsIncomplete,
                "release assets incomplete",
            ),
        ] {
            let error = select_install_strategy(ReleaseLookup::Unavailable { reason }, None)
                .err()
                .ok_or_else(|| anyhow::anyhow!("{description} without a SHA should fail"))?;
            anyhow::ensure!(
                error.to_string()
                    == format!(
                        "no installable ub-review release was resolvable ({description}); pass --action-sha <40-hex-sha> to permit the cached source-build fallback"
                    )
            );

            let source = select_install_strategy(
                ReleaseLookup::Unavailable { reason },
                Some(&fallback_sha),
            )?;
            anyhow::ensure!(
                source
                    == InstallStrategy::Source {
                        sha: fallback_sha.clone()
                    }
            );
        }

        let padded_sha = format!("  {fallback_sha}\r\n");
        let normalized_sha = validate_action_sha(&padded_sha)?;
        anyhow::ensure!(normalized_sha == fallback_sha);
        let normalized_source = select_install_strategy(
            ReleaseLookup::Unavailable {
                reason: ReleaseFallbackReason::RequestUnavailable,
            },
            Some(normalized_sha),
        )?;
        anyhow::ensure!(
            normalized_source
                == InstallStrategy::Source {
                    sha: fallback_sha.clone()
                }
        );

        let temp = tempfile::tempdir()?;
        let args = EnableArgs {
            mode: ReviewModePreset::Gate,
            model: "minimax".to_owned(),
            action_sha: None,
            root: temp.path().to_path_buf(),
            force: false,
        };
        let error = cmd_enable_with_resolver(args, offline_resolve_source)
            .err()
            .ok_or_else(|| anyhow::anyhow!("source fallback without a SHA should fail"))?;
        let message = error.to_string();
        anyhow::ensure!(message.contains("release lookup unavailable"));
        anyhow::ensure!(message.contains("--action-sha <40-hex-sha>"));
        anyhow::ensure!(!temp.path().join(WORKFLOW_RELATIVE_PATH).exists());
        anyhow::ensure!(!temp.path().join(CONFIG_RELATIVE_PATH).exists());

        let malformed_root = tempfile::tempdir()?;
        let malformed = EnableArgs {
            mode: ReviewModePreset::Gate,
            model: "minimax".to_owned(),
            action_sha: Some("not-a-sha".to_owned()),
            root: malformed_root.path().to_path_buf(),
            force: false,
        };
        let error = cmd_enable_with_resolver(malformed, offline_resolve_release)
            .err()
            .ok_or_else(|| anyhow::anyhow!("malformed explicit SHA should fail"))?;
        anyhow::ensure!(
            error
                .to_string()
                .contains("--action-sha must be the full 40-hex")
        );
        anyhow::ensure!(!malformed_root.path().join(WORKFLOW_RELATIVE_PATH).exists());
        anyhow::ensure!(!malformed_root.path().join(CONFIG_RELATIVE_PATH).exists());
        Ok(())
    }

    #[test]
    fn enable_workflows_render_primary_with_optional_opencode_fallback() -> Result<()> {
        let strategies = [
            InstallStrategy::Release {
                tag: "v9.9.9".to_owned(),
            },
            InstallStrategy::Source {
                sha: "1".repeat(40),
            },
        ];

        for strategy in strategies {
            let workflow = render_enable_workflow(&strategy, ReviewModePreset::Gate);
            anyhow::ensure!(workflow.contains("provider-policy: primary-with-fallback"));
            anyhow::ensure!(workflow.contains("minimax-api-key: ${{ secrets.MINIMAX_API_KEY }}"));
            anyhow::ensure!(workflow.contains("opencode-api-key: ${{ secrets.OPENCODE }}"));
            anyhow::ensure!(workflow.contains("opencode-model: mimo-v2.5"));
            anyhow::ensure!(!workflow.contains("provider-policy: minimax-only"));
            anyhow::ensure!(!workflow.contains("pull_request_target"));

            let mut minimax_secret_lines = 0_u8;
            let mut opencode_secret_lines = 0_u8;
            for line in workflow.lines() {
                if line.contains("secrets.MINIMAX_API_KEY") {
                    minimax_secret_lines = minimax_secret_lines.saturating_add(1);
                    anyhow::ensure!(
                        line.trim() == "minimax-api-key: ${{ secrets.MINIMAX_API_KEY }}",
                        "MINIMAX_API_KEY must only appear as its action input; got: {line}"
                    );
                }
                if line.contains("secrets.OPENCODE") {
                    opencode_secret_lines = opencode_secret_lines.saturating_add(1);
                    anyhow::ensure!(
                        line.trim() == "opencode-api-key: ${{ secrets.OPENCODE }}",
                        "OPENCODE must only appear as its action input; got: {line}"
                    );
                }
            }
            anyhow::ensure!(
                minimax_secret_lines == 1,
                "expected exactly one MINIMAX_API_KEY action input, got {minimax_secret_lines}"
            );
            anyhow::ensure!(
                opencode_secret_lines == 1,
                "expected exactly one OPENCODE action input, got {opencode_secret_lines}"
            );

            let summary = render_enable_summary(
                Path::new(".github/workflows/ub-review.yml"),
                Path::new(".ub-review.toml"),
                ReviewModePreset::Gate,
                &strategy,
            );
            anyhow::ensure!(summary.contains("MINIMAX_API_KEY as a required repository secret"));
            anyhow::ensure!(summary.contains("Optionally add OPENCODE for provider fallback"));
            anyhow::ensure!(summary.contains("MiniMax remains"));
        }
        Ok(())
    }
}
