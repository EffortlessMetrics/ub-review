use super::*;
use anyhow::ensure;
use std::time::Instant;

#[test]
fn transport_child_fixture() -> Result<()> {
    let Some(root) = std::env::var_os("UB_REVIEW_TRANSPORT_CHILD_ROOT") else {
        return Ok(());
    };
    let root = PathBuf::from(root);
    fs::write(root.join("ready"), std::process::id().to_string())?;
    std::thread::sleep(Duration::from_millis(900));
    println!("late stdout");
    eprintln!("late stderr");
    fs::write(root.join("late"), b"child finished normally")?;
    Ok(())
}

#[test]
fn transport_wait_failure_terminates_reaps_and_removes_both_outputs() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let stdout_path = temp.path().join("stdout");
    let stderr_path = temp.path().join("stderr");
    let mut child = ProcessCommand::new(std::env::current_exe()?)
        .args([
            "--exact",
            "model_api::lifecycle_tests::transport_child_fixture",
            "--nocapture",
        ])
        .env("UB_REVIEW_TRANSPORT_CHILD_ROOT", temp.path())
        .stdout(Stdio::from(File::create(&stdout_path)?))
        .stderr(Stdio::from(File::create(&stderr_path)?))
        .spawn()?;
    let pid = child.id();
    let mut observed_running = false;
    let result = wait_for_child_output_files_with_wait(
        &mut child,
        &stdout_path,
        &stderr_path,
        3,
        &mut |child, _timeout| {
            let deadline = Instant::now() + Duration::from_secs(3);
            while !temp.path().join("ready").exists() {
                if Instant::now() >= deadline {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "fixture readiness deadline",
                    ));
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            observed_running = child.try_wait()?.is_none();
            Err(std::io::Error::other("injected transport wait failure"))
        },
    );
    // Snapshot before fixture teardown so teardown cannot satisfy the oracle.
    #[cfg(target_os = "linux")]
    let reaped_at_return = !Path::new(&format!("/proc/{pid}")).exists();
    #[cfg(not(target_os = "linux"))]
    let _ = pid;
    let status_at_return = child.try_wait()?;
    let outputs_removed = !stdout_path.exists() && !stderr_path.exists();
    let late_marker = temp.path().join("late").exists();
    // Retain the handle so a failing regression never abandons its fixture.
    if status_at_return.is_none() {
        let _ = child.kill();
        child.wait()?;
    }

    ensure!(observed_running, "fixture never reached a running child");
    let error = result.err().context("wait failure must remain an error")?;
    ensure!(format!("{error:#}").contains("injected transport wait failure"));
    ensure!(
        status_at_return.is_some_and(|status| !status.success()),
        "wait failure returned before terminating its child"
    );
    #[cfg(target_os = "linux")]
    ensure!(reaped_at_return, "child was not reaped at helper return");
    ensure!(
        outputs_removed,
        "temporary output files survived wait failure"
    );
    ensure!(!late_marker, "child completed normally after wait failure");
    Ok(())
}

#[test]
fn transport_wait_error_custody_for_confirmed_reap() -> Result<()> {
    for kill_fails in [false, true] {
        let wait_error = std::io::Error::from_raw_os_error(24);
        let wait_message = wait_error.to_string();
        let cleanup: Result<Option<std::io::Error>> = Ok(kill_fails.then(|| {
            std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "injected kill failure",
            )
        }));
        let error = transport_wait_failure_error(wait_error, cleanup);
        let typed_wait = error
            .downcast_ref::<std::io::Error>()
            .context("original wait error must remain typed")?;
        ensure!(typed_wait.raw_os_error() == Some(24));
        let expected = if kill_fails {
            format!(
                "wait for child process; kill request failed: injected kill failure; child reap confirmed: {wait_message}"
            )
        } else {
            format!("wait for child process: {wait_message}")
        };
        ensure!(format!("{error:#}") == expected);
    }
    Ok(())
}

#[test]
fn transport_wait_error_custody_for_unconfirmed_reap() -> Result<()> {
    let wait_error = std::io::Error::from_raw_os_error(24);
    let wait_message = wait_error.to_string();
    let cleanup_error = std::io::Error::new(
        std::io::ErrorKind::PermissionDenied,
        "injected reap failure",
    );
    let error = transport_wait_failure_error(wait_error, Err(cleanup_error.into()));
    let typed_cleanup = error
        .downcast_ref::<std::io::Error>()
        .context("cleanup root cause must remain typed")?;
    ensure!(typed_cleanup.kind() == std::io::ErrorKind::PermissionDenied);
    ensure!(
        format!("{error:#}")
            == format!(
                "terminate child after wait failure; original wait error: {wait_message}: injected reap failure"
            )
    );
    Ok(())
}
