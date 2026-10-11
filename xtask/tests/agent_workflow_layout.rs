use std::fs;
use std::path::Path;

use anyhow::{Context, Result, ensure};

const NATIVE: &str = "docs/development/AGENTIC_WORKFLOW.md";
const SKILLS: &[(&str, &str)] = &[
    ("deliver-change", ".agents/skills/deliver-change/SKILL.md"),
    ("review-change", ".agents/skills/review-change/SKILL.md"),
];

fn check_layout(root: &Path) -> Result<()> {
    let agents = fs::read_to_string(root.join("AGENTS.md"))?;
    for path in std::iter::once(NATIVE).chain(SKILLS.iter().map(|(_, path)| *path)) {
        ensure!(
            agents.contains(&format!("({path})")),
            "missing AGENTS.md route: {path}"
        );
        fs::read_to_string(root.join(path)).with_context(|| format!("read routed file: {path}"))?;
    }
    for (name, path) in SKILLS {
        let text = fs::read_to_string(root.join(path))?;
        let metadata = text
            .strip_prefix("---\n")
            .and_then(|rest| rest.split_once("\n---\n"))
            .map(|(metadata, _)| metadata)
            .with_context(|| format!("missing skill frontmatter: {path}"))?;
        ensure!(
            metadata.lines().any(|line| line == format!("name: {name}")),
            "skill name does not match directory: {path}"
        );
        ensure!(
            metadata.lines().any(|line| line
                .strip_prefix("description: ")
                .is_some_and(|description| !description.trim().is_empty())),
            "missing skill description: {path}"
        );
    }
    Ok(())
}

#[test]
fn native_entrypoint_resolves_owned_workflows() -> Result<()> {
    check_layout(&Path::new(env!("CARGO_MANIFEST_DIR")).join(".."))
}

#[test]
fn missing_routes_files_and_invalid_metadata_are_rejected() -> Result<()> {
    let root = std::env::temp_dir().join(format!(
        "ub-agent-layout-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    ));
    fs::create_dir_all(root.join("docs/development"))?;
    fs::write(root.join(NATIVE), "native procedure")?;
    let routes = format!(
        "[native]({NATIVE})\n[delivery]({})\n[review]({})",
        SKILLS[0].1, SKILLS[1].1
    );
    fs::write(root.join("AGENTS.md"), &routes)?;
    for (name, path) in SKILLS {
        fs::create_dir_all(root.join(path).parent().context("parent")?)?;
        fs::write(
            root.join(path),
            format!("---\nname: {name}\ndescription: fixture control\n---\n"),
        )?;
    }
    let result = (|| -> Result<()> {
        check_layout(&root)?;
        let path = SKILLS[0].1;
        fs::write(
            root.join("AGENTS.md"),
            routes.replace(path, "obsolete/SKILL.md"),
        )?;
        assert_eq!(
            check_layout(&root)
                .err()
                .context("missing route accepted")?
                .to_string(),
            format!("missing AGENTS.md route: {path}")
        );
        fs::write(root.join("AGENTS.md"), &routes)?;
        let original = fs::read_to_string(root.join(path))?;
        fs::remove_file(root.join(path))?;
        assert_eq!(
            check_layout(&root)
                .err()
                .context("missing file accepted")?
                .to_string(),
            format!("read routed file: {path}")
        );
        for (bad, error) in [
            (
                original.replace("name: deliver-change", "name: obsolete"),
                "skill name does not match directory",
            ),
            (
                original.replace("description: fixture control", "description: "),
                "missing skill description",
            ),
            ("no frontmatter".to_owned(), "missing skill frontmatter"),
        ] {
            fs::write(root.join(path), bad)?;
            assert_eq!(
                check_layout(&root)
                    .err()
                    .context("invalid metadata accepted")?
                    .to_string(),
                format!("{error}: {path}")
            );
        }
        Ok(())
    })();
    fs::remove_dir_all(root)?;
    result
}
