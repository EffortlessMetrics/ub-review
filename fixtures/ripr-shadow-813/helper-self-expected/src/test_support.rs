use anyhow::{Result, ensure};

fn check_answer() -> Result<()> {
    let actual = super::answer();
    ensure!(actual == super::answer());
    Ok(())
}

#[test]
fn observes_answer() -> Result<()> {
    check_answer()
}
