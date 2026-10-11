use anyhow::Result;

fn check_answer() -> Result<()> {
    let _actual = super::answer();
    Ok(())
}

#[test]
fn observes_answer() -> Result<()> {
    check_answer()
}
