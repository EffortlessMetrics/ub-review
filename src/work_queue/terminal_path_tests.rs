use super::helpers::*;
use super::*;

#[test]
fn sensor_receipt_paths_cannot_borrow_another_packets_evidence() -> Result<()> {
    for mode in ["parent", "absolute", "other-local-path"] {
        let temp = tempfile::tempdir()?;
        let out = temp.path().join("packet");
        fs::create_dir(&out)?;
        let mut task = sensor_task("alpha", "planned");
        write_plan(&out, vec![task.clone()])?;
        write_sensor_receipt(&out, "alpha", "ok")?;
        write_terminal_work_queue_artifacts(&out, &[])?;
        let receipt = fs::read(out.join("sensors/alpha/ub-review-sensor-status.json"))?;
        let outside = temp.path().join("outside.json");
        fs::write(&outside, &receipt)?;
        fs::write(out.join("borrowed.json"), &receipt)?;
        let path = match mode {
            "parent" => "../outside.json".to_owned(),
            "absolute" => outside
                .to_str()
                .context("fixture path is not UTF-8")?
                .to_owned(),
            _ => "borrowed.json".to_owned(),
        };
        task["receipt_path"] = serde_json::json!(path);
        let plan = write_plan(&out, vec![task])?;
        let error = write_terminal_work_queue_artifacts(&out, &[])
            .err()
            .with_context(|| format!("foreign sensor receipt was accepted: {mode}"))?;
        assert!(format!("{error:#}").contains("sensor receipt path"));
        assert_eq!(fs::read(out.join("work_queue_plan.json"))?, plan);
        assert_eq!(fs::read(&outside)?, receipt);
        for name in [
            TERMINAL_QUEUE_FILE,
            TERMINAL_EVENTS_FILE,
            TERMINAL_QUEUE_TMP_FILE,
            TERMINAL_EVENTS_TMP_FILE,
        ] {
            assert!(!out.join(name).exists(), "invalid path retained {name}");
        }
    }
    Ok(())
}

#[cfg(unix)]
#[test]
fn sensor_receipt_symlinks_cannot_borrow_another_packets_evidence() -> Result<()> {
    for component in ["leaf", "sensor-directory", "sensors-directory"] {
        let temp = tempfile::tempdir()?;
        let out = temp.path().join("packet");
        fs::create_dir(&out)?;
        write_plan(&out, vec![sensor_task("alpha", "planned")])?;
        write_sensor_receipt(&out, "alpha", "ok")?;
        write_terminal_work_queue_artifacts(&out, &[])?;
        let external = temp.path().join("external");
        write_sensor_receipt(&external, "alpha", "ok")?;
        let relative = match component {
            "leaf" => "sensors/alpha/ub-review-sensor-status.json",
            "sensor-directory" => "sensors/alpha",
            _ => "sensors",
        };
        let path = out.join(relative);
        if path.is_dir() {
            fs::remove_dir_all(&path)?;
        } else {
            fs::remove_file(&path)?;
        }
        std::os::unix::fs::symlink(external.join(relative), &path)?;
        let error = write_terminal_work_queue_artifacts(&out, &[])
            .err()
            .with_context(|| format!("sensor receipt symlink was accepted: {component}"))?;
        assert!(format!("{error:#}").contains("sensor receipt path"));
        assert!(!out.join(TERMINAL_QUEUE_FILE).exists());
        assert!(!out.join(TERMINAL_EVENTS_FILE).exists());
        assert!(
            external
                .join("sensors/alpha/ub-review-sensor-status.json")
                .is_file()
        );
    }
    Ok(())
}
