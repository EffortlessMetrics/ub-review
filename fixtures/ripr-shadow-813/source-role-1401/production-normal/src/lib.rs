pub mod executable_fixture;
#[test]
fn real_capture_keeps_literal_stdout_and_native_status() -> std::io::Result<()> {
    use executable_fixture::FixtureCommandExt;
    let output = std::process::Command::new("/bin/sh")
        .args(["-c", "printf 'role-control-byte\n'; exit 7"])
        .fixture_output()?;
    assert_eq!(output.stdout, b"role-control-byte\n");
    assert_eq!(output.status.code(), Some(7));
    assert!(output.stderr.is_empty());
    Ok(())
}
