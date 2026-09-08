use std::env;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use super::receipts::{Authorization, write_json};

pub(super) const MISSING_ASSET: &str = "ub-review-missing-x86_64-unknown-linux-gnu.tar.gz";

#[derive(Debug, Deserialize, Serialize)]
pub(super) struct ResolverEnvironment {
    cargo_present: bool,
    rustc_present: bool,
    source_workdir_present: bool,
}

#[derive(Deserialize, Serialize)]
struct Before {
    schema: String,
    authorization: Authorization,
    environment: ResolverEnvironment,
}

#[derive(Serialize)]
struct Receipt {
    schema: &'static str,
    authorization: Authorization,
    release_tag: &'static str,
    requested_asset: &'static str,
    step_outcome: String,
    before: ResolverEnvironment,
    after: ResolverEnvironment,
    outcome: &'static str,
}

fn executable_present(name: &str) -> Result<bool> {
    let path = env::var_os("PATH").context("resolver PATH is missing")?;
    for directory in env::split_paths(&path) {
        if directory.join(name).is_file() {
            return Ok(true);
        }
    }
    Ok(false)
}

fn observe() -> Result<ResolverEnvironment> {
    ensure!(
        cfg!(target_os = "linux"),
        "strict resolver proof requires its clean Linux container"
    );
    let temp = env::var_os("RUNNER_TEMP").context("resolver RUNNER_TEMP is missing")?;
    Ok(ResolverEnvironment {
        cargo_present: executable_present("cargo")?,
        rustc_present: executable_present("rustc")?,
        source_workdir_present: Path::new(&temp).join("ub-review-action-src").exists(),
    })
}

fn require_clean(environment: &ResolverEnvironment) -> Result<()> {
    ensure!(
        !environment.cargo_present
            && !environment.rustc_present
            && !environment.source_workdir_present,
        "strict release resolver has Rust tooling or a source fallback workdir"
    );
    Ok(())
}

pub(super) fn before(out: &Path, authorization: Authorization) -> Result<()> {
    let environment = observe()?;
    require_clean(&environment)?;
    write_json(
        &out.join("resolver-before.json"),
        &Before {
            schema: "ub-review.release_resolver_before.v2".to_owned(),
            authorization,
            environment,
        },
    )
}

pub(super) fn after(out: &Path, authorization: Authorization, outcome: &str) -> Result<()> {
    let before: Before = serde_json::from_slice(&fs::read(out.join("resolver-before.json"))?)?;
    ensure!(
        before.schema == "ub-review.release_resolver_before.v2"
            && before.authorization == authorization,
        "resolver baseline does not bind this source and dispatch"
    );
    require_clean(&before.environment)?;
    let after = observe()?;
    require_clean(&after)?;
    ensure!(
        outcome == "failure",
        "strict missing-asset Action did not fail"
    );
    write_json(
        &out.join("resolver-no-fallback.json"),
        &Receipt {
            schema: "ub-review.release_resolver_no_fallback.v2",
            authorization,
            release_tag: super::metadata::TAG,
            requested_asset: MISSING_ASSET,
            step_outcome: outcome.to_owned(),
            before: before.environment,
            after,
            outcome: "rejected_without_fallback",
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_resolver_rejects_each_fallback_surface() -> Result<()> {
        require_clean(&ResolverEnvironment {
            cargo_present: false,
            rustc_present: false,
            source_workdir_present: false,
        })?;
        for (cargo, rustc, source) in [
            (true, false, false),
            (false, true, false),
            (false, false, true),
        ] {
            ensure!(
                require_clean(&ResolverEnvironment {
                    cargo_present: cargo,
                    rustc_present: rustc,
                    source_workdir_present: source
                })
                .is_err()
            );
        }
        Ok(())
    }
}
