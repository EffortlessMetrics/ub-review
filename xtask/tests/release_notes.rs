use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result, bail};

const LITERAL_TAG: &str = "v9.8.7-rc.12";
const LITERAL_TEMPLATE: &str = "# ub-review $TAG\n\nVersion $TAG is ready.\n";
const LITERAL_EXPECTED: &str = "# ub-review v9.8.7-rc.12\n\nVersion v9.8.7-rc.12 is ready.\n";
const OUTPUT_NAME: &str = "notes-out.md";
const SENTINEL: &str = "sentinel-1342-output-untouched\n";

static COUNTER: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn create(slug: &str) -> Result<Self> {
        let count = COUNTER.fetch_add(1, Ordering::SeqCst);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "ub-review-release-notes-{slug}-{}-{nanos}-{count}",
            std::process::id()
        ));
        fs::create_dir_all(root.join(".github"))?;
        Ok(Self { root })
    }

    fn write_template(&self, contents: &str) -> Result<()> {
        fs::write(self.root.join(".github").join("release-notes.md"), contents)?;
        Ok(())
    }

    fn write_sentinel(&self) -> Result<()> {
        fs::write(self.root.join(OUTPUT_NAME), SENTINEL)?;
        Ok(())
    }

    fn read_output(&self) -> Result<String> {
        Ok(fs::read_to_string(self.root.join(OUTPUT_NAME))?)
    }

    fn run(&self, args: &[String]) -> Result<Output> {
        Ok(Command::new(env!("CARGO_BIN_EXE_xtask"))
            .args(args)
            .current_dir(&self.root)
            .output()
            .context("run xtask render-release-notes")?)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn failure_leaves_sentinel(slug: &str, template: &str, args: &[String]) -> Result<()> {
    let fixture = Fixture::create(slug)?;
    fixture.write_template(template)?;
    fixture.write_sentinel()?;
    let output = fixture.run(args)?;
    assert!(
        !output.status.success(),
        "args {args:?} unexpectedly succeeded"
    );
    assert_eq!(fixture.read_output()?, SENTINEL);
    Ok(())
}

fn success_args(tag: &str) -> Vec<String> {
    vec![
        "render-release-notes".to_owned(),
        "--tag".to_owned(),
        tag.to_owned(),
        "--output".to_owned(),
        OUTPUT_NAME.to_owned(),
    ]
}

#[test]
fn renders_literal_tag_to_exact_bytes() -> Result<()> {
    let fixture = Fixture::create("literal")?;
    fixture.write_template(LITERAL_TEMPLATE)?;
    let output = fixture.run(&success_args(LITERAL_TAG))?;
    if !output.status.success() {
        bail!("render failed: {}", String::from_utf8_lossy(&output.stderr));
    }
    let bytes = fixture.read_output()?;
    assert!(!bytes.is_empty());
    assert_eq!(bytes, LITERAL_EXPECTED);
    Ok(())
}

#[test]
fn accepts_workflow_tag_shapes() -> Result<()> {
    let cases = [
        ("v1", "# ub-review v1\n\nVersion v1 is ready.\n"),
        (
            "v0.1.2_rc.3",
            "# ub-review v0.1.2_rc.3\n\nVersion v0.1.2_rc.3 is ready.\n",
        ),
    ];
    for (index, (tag, expected)) in cases.iter().enumerate() {
        let fixture = Fixture::create(&format!("valid-{index}"))?;
        fixture.write_template(LITERAL_TEMPLATE)?;
        let output = fixture.run(&success_args(tag))?;
        if !output.status.success() {
            bail!(
                "tag {tag} failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        let bytes = fixture.read_output()?;
        assert!(!bytes.is_empty());
        assert_eq!(bytes, *expected);
    }
    Ok(())
}

#[test]
fn rejects_missing_duplicate_unknown_and_invalid_tags() -> Result<()> {
    let cases: Vec<Vec<&str>> = vec![
        vec!["render-release-notes", "--output", OUTPUT_NAME],
        vec!["render-release-notes", "--tag", "v1"],
        vec![
            "render-release-notes",
            "--tag",
            "v1",
            "--tag",
            "v1",
            "--output",
            OUTPUT_NAME,
        ],
        vec![
            "render-release-notes",
            "--tag",
            "v1",
            "--output",
            OUTPUT_NAME,
            "--output",
            OUTPUT_NAME,
        ],
        vec![
            "render-release-notes",
            "--tag",
            "v1",
            "--output",
            OUTPUT_NAME,
            "--unknown",
        ],
        vec![
            "render-release-notes",
            "--tag",
            "v1",
            "--output",
            OUTPUT_NAME,
            "extra",
        ],
        vec![
            "render-release-notes",
            "--tag",
            "1.2.3",
            "--output",
            OUTPUT_NAME,
        ],
        vec![
            "render-release-notes",
            "--tag",
            "vx",
            "--output",
            OUTPUT_NAME,
        ],
        vec![
            "render-release-notes",
            "--tag",
            "v 1",
            "--output",
            OUTPUT_NAME,
        ],
        vec![
            "render-release-notes",
            "--tag",
            "v1!",
            "--output",
            OUTPUT_NAME,
        ],
    ];
    for (index, case) in cases.iter().enumerate() {
        let args: Vec<String> = case.iter().map(|part| (*part).to_owned()).collect();
        failure_leaves_sentinel(&format!("bad-{index}"), LITERAL_TEMPLATE, &args)?;
    }
    Ok(())
}

#[test]
fn rejects_unknown_placeholders() -> Result<()> {
    let templates = [
        "# ub-review $TAG\n\n$VERSION\n",
        "# ub-review $TAG_EXTRA\n",
        "# ub-review ${TAG}\n",
    ];
    for (index, template) in templates.iter().enumerate() {
        failure_leaves_sentinel(
            &format!("placeholder-{index}"),
            template,
            &success_args(LITERAL_TAG),
        )?;
    }
    Ok(())
}

#[test]
fn preserves_literal_dollar_currency() -> Result<()> {
    let fixture = Fixture::create("currency")?;
    fixture.write_template("# ub-review $TAG\n\nPrice $20 stays.\n")?;
    let output = fixture.run(&success_args("v1"))?;
    if !output.status.success() {
        bail!(
            "currency render failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let bytes = fixture.read_output()?;
    assert!(!bytes.is_empty());
    assert_eq!(bytes, "# ub-review v1\n\nPrice $20 stays.\n");
    Ok(())
}
