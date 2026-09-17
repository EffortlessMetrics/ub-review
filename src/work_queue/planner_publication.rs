//! Recoverable publication of the immutable planner artifact set.

use crate::*;
use std::io;

const WORK_QUEUE_FILE: &str = "work_queue.json";
const WORK_EVENTS_FILE: &str = "work_events.ndjson";
const WORK_QUEUE_PLAN_FILE: &str = "work_queue_plan.json";
const WORK_EVENTS_PLAN_FILE: &str = "work_events_plan.ndjson";

const WORK_QUEUE_STAGE_FILE: &str = ".work_queue.json.tmp";
const WORK_EVENTS_STAGE_FILE: &str = ".work_events.ndjson.tmp";
const WORK_QUEUE_PLAN_STAGE_FILE: &str = ".work_queue_plan.json.tmp";
const WORK_EVENTS_PLAN_STAGE_FILE: &str = ".work_events_plan.ndjson.tmp";

struct PlannerArtifact<'a> {
    destination: &'static str,
    staging: &'static str,
    bytes: &'a [u8],
}

pub(super) fn publish_work_queue_plan_artifacts(
    out: &Path,
    queue_bytes: &[u8],
    event_bytes: &[u8],
) -> Result<()> {
    publish_work_queue_plan_artifacts_with_hook(out, queue_bytes, event_bytes, |_, _| Ok(()))
}

fn publish_work_queue_plan_artifacts_with_hook(
    out: &Path,
    queue_bytes: &[u8],
    event_bytes: &[u8],
    mut before_replace: impl FnMut(usize, &Path) -> Result<()>,
) -> Result<()> {
    let artifacts = planner_artifacts(queue_bytes, event_bytes);
    let previous = artifacts
        .iter()
        .map(|artifact| read_optional_file(&out.join(artifact.destination)))
        .collect::<Result<Vec<_>>>()?;

    cleanup_staged_artifacts(out, &artifacts)?;
    for artifact in &artifacts {
        let staged = out.join(artifact.staging);
        if let Err(error) = fs::write(&staged, artifact.bytes)
            .with_context(|| format!("stage planner artifact {}", staged.display()))
        {
            return match cleanup_staged_artifacts(out, &artifacts) {
                Ok(()) => Err(error),
                Err(cleanup_error) => Err(anyhow::anyhow!(
                    "{error:#}; planner staging cleanup after write failure was incomplete: {cleanup_error:#}"
                )),
            };
        }
    }

    // The explicit plan queue is the planner-generation commit marker used by
    // the proof-receipt writer. Remove the previous marker before replacing
    // any sibling so interruption cannot expose a mixed generation as current.
    if let Err(error) = remove_file_if_present(&out.join(WORK_QUEUE_PLAN_FILE))
        .context("invalidate prior work-queue plan commit marker")
    {
        return match cleanup_staged_artifacts(out, &artifacts) {
            Ok(()) => Err(error),
            Err(cleanup_error) => Err(anyhow::anyhow!(
                "{error:#}; planner staging cleanup after marker invalidation failure was incomplete: {cleanup_error:#}"
            )),
        };
    }

    let mut publication_error = None;
    for (index, artifact) in artifacts.iter().enumerate() {
        let staged = out.join(artifact.staging);
        let destination = out.join(artifact.destination);
        let result = before_replace(index, &destination)
            .and_then(|()| replace_with_staged_file(&staged, &destination));
        if let Err(error) = result {
            publication_error = Some(error);
            break;
        }
    }

    if let Some(error) = publication_error {
        return match rollback_planner_artifacts(out, &artifacts, &previous) {
            Ok(()) => Err(error).context("publish work-queue planner artifact set"),
            Err(rollback_error) => Err(anyhow::anyhow!(
                "publish work-queue planner artifact set failed: {error:#}; planner rollback incomplete: {rollback_error:#}"
            )),
        };
    }

    cleanup_staged_artifacts(out, &artifacts)?;
    Ok(())
}

fn planner_artifacts<'a>(queue_bytes: &'a [u8], event_bytes: &'a [u8]) -> [PlannerArtifact<'a>; 4] {
    [
        PlannerArtifact {
            destination: WORK_QUEUE_FILE,
            staging: WORK_QUEUE_STAGE_FILE,
            bytes: queue_bytes,
        },
        PlannerArtifact {
            destination: WORK_EVENTS_FILE,
            staging: WORK_EVENTS_STAGE_FILE,
            bytes: event_bytes,
        },
        PlannerArtifact {
            destination: WORK_EVENTS_PLAN_FILE,
            staging: WORK_EVENTS_PLAN_STAGE_FILE,
            bytes: event_bytes,
        },
        // Publish the commit marker last after every sibling artifact is durable.
        PlannerArtifact {
            destination: WORK_QUEUE_PLAN_FILE,
            staging: WORK_QUEUE_PLAN_STAGE_FILE,
            bytes: queue_bytes,
        },
    ]
}

fn cleanup_staged_artifacts(out: &Path, artifacts: &[PlannerArtifact<'_>]) -> Result<()> {
    let mut failures = Vec::new();
    for artifact in artifacts {
        if let Err(error) = remove_file_if_present(&out.join(artifact.staging)) {
            failures.push(format!("{}: {error:#}", artifact.staging));
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        anyhow::bail!(
            "planner staging cleanup incomplete: {}",
            failures.join("; ")
        )
    }
}

fn rollback_planner_artifacts(
    out: &Path,
    artifacts: &[PlannerArtifact<'_>],
    previous: &[Option<Vec<u8>>],
) -> Result<()> {
    anyhow::ensure!(
        artifacts.len() == previous.len(),
        "planner rollback input length mismatch"
    );
    let marker_index = artifacts
        .iter()
        .position(|artifact| artifact.destination == WORK_QUEUE_PLAN_FILE)
        .context("planner rollback has no plan commit marker")?;
    anyhow::ensure!(
        marker_index + 1 == artifacts.len(),
        "planner rollback commit marker is not last"
    );

    let mut failures = Vec::new();
    for (artifact, prior) in artifacts[..marker_index]
        .iter()
        .zip(&previous[..marker_index])
    {
        if let Err(error) = restore_optional_file(&out.join(artifact.destination), prior.as_deref()) {
            failures.push(format!(
                "restore prior planner artifact {}: {error:#}",
                artifact.destination
            ));
        }
    }

    let marker = &artifacts[marker_index];
    let marker_path = out.join(marker.destination);
    if failures.is_empty() {
        if let Err(error) = restore_optional_file(&marker_path, previous[marker_index].as_deref()) {
            failures.push(format!(
                "restore prior planner commit marker {}: {error:#}",
                marker.destination
            ));
            if let Err(withhold_error) = remove_file_if_present(&marker_path) {
                failures.push(format!(
                    "withhold incomplete planner commit marker {}: {withhold_error:#}",
                    marker.destination
                ));
            }
        }
    } else if let Err(error) = remove_file_if_present(&marker_path) {
        failures.push(format!(
            "withhold incomplete planner commit marker {}: {error:#}",
            marker.destination
        ));
    }

    if let Err(error) = cleanup_staged_artifacts(out, artifacts) {
        failures.push(format!("clean planner staging after rollback: {error:#}"));
    }

    if failures.is_empty() {
        Ok(())
    } else {
        anyhow::bail!(failures.join("; "))
    }
}

fn read_optional_file(path: &Path) -> Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("read prior {}", path.display())),
    }
}

fn remove_file_if_present(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("remove {}", path.display())),
    }
}

fn replace_with_staged_file(staged: &Path, destination: &Path) -> Result<()> {
    remove_file_if_present(destination)?;
    fs::rename(staged, destination)
        .with_context(|| format!("publish staged planner artifact {}", destination.display()))
}

fn restore_optional_file(path: &Path, previous: Option<&[u8]>) -> Result<()> {
    match previous {
        Some(bytes) => {
            fs::write(path, bytes).with_context(|| format!("restore {}", path.display()))
        }
        None => remove_file_if_present(path),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn previous_artifacts() -> [(&'static str, &'static [u8]); 4] {
        [
            (WORK_QUEUE_FILE, b"previous queue"),
            (WORK_EVENTS_FILE, b"previous events\n"),
            (WORK_EVENTS_PLAN_FILE, b"previous plan events\n"),
            (WORK_QUEUE_PLAN_FILE, b"previous plan queue"),
        ]
    }

    #[test]
    fn planner_publication_rolls_back_each_replacement_boundary() -> Result<()> {
        for failure_index in 0..4 {
            let temp = tempfile::tempdir()?;
            let out = temp.path();
            for (name, bytes) in previous_artifacts() {
                fs::write(out.join(name), bytes)?;
            }

            let error = publish_work_queue_plan_artifacts_with_hook(
                out,
                b"replacement queue",
                b"replacement events\n",
                |index, destination| {
                    anyhow::ensure!(
                        !out.join(WORK_QUEUE_PLAN_FILE).exists(),
                        "planner commit marker remained visible before replacement {index}"
                    );
                    if index == failure_index {
                        anyhow::bail!(
                            "injected planner replacement failure at {}",
                            destination.display()
                        );
                    }
                    Ok(())
                },
            )
            .err()
            .with_context(|| {
                format!("replacement boundary {failure_index} unexpectedly succeeded")
            })?;
            assert!(
                format!("{error:#}").contains("injected planner replacement failure"),
                "unexpected error at boundary {failure_index}: {error:#}"
            );
            for (name, bytes) in previous_artifacts() {
                assert_eq!(
                    fs::read(out.join(name))?,
                    bytes,
                    "boundary {failure_index} did not restore {name}"
                );
            }
            for artifact in planner_artifacts(b"", b"") {
                assert!(
                    !out.join(artifact.staging).exists(),
                    "boundary {failure_index} retained staging {}",
                    artifact.staging
                );
            }
        }
        Ok(())
    }

    #[test]
    fn planner_publication_restores_absence_after_partial_replacement() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let out = temp.path();
        let error = publish_work_queue_plan_artifacts_with_hook(
            out,
            b"replacement queue",
            b"replacement events\n",
            |index, destination| {
                assert!(!out.join(WORK_QUEUE_PLAN_FILE).exists());
                if index == 3 {
                    anyhow::bail!(
                        "injected final planner replacement failure at {}",
                        destination.display()
                    );
                }
                Ok(())
            },
        )
        .err()
        .context("final replacement boundary unexpectedly succeeded")?;
        assert!(format!("{error:#}").contains("injected final planner replacement failure"));
        for artifact in planner_artifacts(b"", b"") {
            assert!(!out.join(artifact.destination).exists());
            assert!(!out.join(artifact.staging).exists());
        }
        Ok(())
    }

    #[test]
    fn planner_publication_commits_plan_queue_last_and_leaves_no_staging() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let out = temp.path();
        fs::write(out.join(WORK_QUEUE_PLAN_FILE), b"previous plan queue")?;
        let mut order = Vec::new();
        publish_work_queue_plan_artifacts_with_hook(
            out,
            b"replacement queue",
            b"replacement events\n",
            |_, destination| {
                assert!(!out.join(WORK_QUEUE_PLAN_FILE).exists());
                order.push(
                    destination
                        .file_name()
                        .context("planner destination has no file name")?
                        .to_string_lossy()
                        .into_owned(),
                );
                Ok(())
            },
        )?;

        assert_eq!(
            order,
            vec![
                WORK_QUEUE_FILE,
                WORK_EVENTS_FILE,
                WORK_EVENTS_PLAN_FILE,
                WORK_QUEUE_PLAN_FILE,
            ]
        );
        assert_eq!(fs::read(out.join(WORK_QUEUE_FILE))?, b"replacement queue");
        assert_eq!(
            fs::read(out.join(WORK_QUEUE_PLAN_FILE))?,
            b"replacement queue"
        );
        assert_eq!(
            fs::read(out.join(WORK_EVENTS_FILE))?,
            b"replacement events\n"
        );
        assert_eq!(
            fs::read(out.join(WORK_EVENTS_PLAN_FILE))?,
            b"replacement events\n"
        );
        for artifact in planner_artifacts(b"", b"") {
            assert!(!out.join(artifact.staging).exists());
        }
        Ok(())
    }

    #[test]
    fn planner_rollback_failure_withholds_marker_and_restores_other_siblings() -> Result<()> {
        for blocked in [WORK_QUEUE_FILE, WORK_EVENTS_FILE, WORK_EVENTS_PLAN_FILE] {
            let temp = tempfile::tempdir()?;
            let out = temp.path();
            for (name, bytes) in previous_artifacts() {
                fs::write(out.join(name), bytes)?;
            }
            let error = publish_work_queue_plan_artifacts_with_hook(
                out,
                b"replacement queue",
                b"replacement events\n",
                |index, _| {
                    if index == 3 {
                        fs::remove_file(out.join(blocked))?;
                        fs::create_dir(out.join(blocked))?;
                        anyhow::bail!("injected publication failure with blocked restore {blocked}");
                    }
                    Ok(())
                },
            )
            .err()
            .context("blocked rollback unexpectedly succeeded")?;
            let diagnostic = format!("{error:#}");
            assert!(diagnostic.contains("injected publication failure"));
            assert!(diagnostic.contains(blocked));
            assert!(
                !out.join(WORK_QUEUE_PLAN_FILE).exists(),
                "incomplete rollback left an accepted plan marker for {blocked}"
            );
            for (name, bytes) in previous_artifacts() {
                if name != blocked && name != WORK_QUEUE_PLAN_FILE {
                    assert_eq!(fs::read(out.join(name))?, bytes, "unrestored sibling {name}");
                }
            }
            assert!(out.join(blocked).is_dir());
            for artifact in planner_artifacts(b"", b"") {
                assert!(!out.join(artifact.staging).exists());
            }
            fs::remove_dir(out.join(blocked))?;
            publish_work_queue_plan_artifacts(out, b"retry queue", b"retry events\n")?;
            assert_eq!(fs::read(out.join(WORK_QUEUE_FILE))?, b"retry queue");
            assert_eq!(fs::read(out.join(WORK_QUEUE_PLAN_FILE))?, b"retry queue");
            assert_eq!(fs::read(out.join(WORK_EVENTS_FILE))?, b"retry events\n");
            assert_eq!(fs::read(out.join(WORK_EVENTS_PLAN_FILE))?, b"retry events\n");
        }
        Ok(())
    }

    #[test]
    fn planner_staging_cleanup_attempts_every_path_after_one_failure() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let out = temp.path();
        let artifacts = planner_artifacts(b"", b"");
        for artifact in &artifacts {
            fs::write(out.join(artifact.staging), b"stale staging")?;
        }
        fs::remove_file(out.join(WORK_QUEUE_STAGE_FILE))?;
        fs::create_dir(out.join(WORK_QUEUE_STAGE_FILE))?;
        let error = cleanup_staged_artifacts(out, &artifacts)
            .err()
            .context("blocked staging cleanup unexpectedly succeeded")?;
        assert!(format!("{error:#}").contains(WORK_QUEUE_STAGE_FILE));
        assert!(out.join(WORK_QUEUE_STAGE_FILE).is_dir());
        for artifact in &artifacts[1..] {
            assert!(
                !out.join(artifact.staging).exists(),
                "cleanup skipped {} after an earlier failure",
                artifact.staging
            );
        }
        Ok(())
    }
}
