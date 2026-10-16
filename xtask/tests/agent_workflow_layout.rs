use std::fs;
use std::path::Path;

use anyhow::{Context, Result, ensure};
use tempfile::TempDir;

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
        for key in ["name", "description"] {
            ensure!(
                metadata
                    .lines()
                    .filter(|line| line
                        .split_once(':')
                        .is_some_and(|(field, _)| field.trim() == key))
                    .count()
                    <= 1,
                "duplicate skill {key}: {path}"
            );
        }
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
    let fixture = TempDir::new().context("create layout fixture")?;
    let root = fixture.path();
    fs::create_dir_all(root.join("docs/development"))?;
    let native_path = "docs/development/AGENTIC_WORKFLOW.md";
    fs::write(root.join(native_path), "native procedure")?;
    // Fixture inputs are independent of the catalogue under test. Otherwise a
    // duplicate delivery entry can erase review routing from both sides.
    let fixture_skills = [
        ("deliver-change", ".agents/skills/deliver-change/SKILL.md"),
        ("review-change", ".agents/skills/review-change/SKILL.md"),
    ];
    let routes = "[native](docs/development/AGENTIC_WORKFLOW.md)\n[delivery](.agents/skills/deliver-change/SKILL.md)\n[review](.agents/skills/review-change/SKILL.md)";
    fs::write(root.join("AGENTS.md"), routes)?;
    for (name, path) in fixture_skills {
        fs::create_dir_all(root.join(path).parent().context("parent")?)?;
        fs::write(
            root.join(path),
            format!("---\nname: {name}\ndescription: fixture control\n---\n"),
        )?;
    }
    check_layout(root)?;
    fs::write(
        root.join("AGENTS.md"),
        format!(
            "`({native_path})`\n`(.agents/skills/deliver-change/SKILL.md)`\n`(.agents/skills/review-change/SKILL.md)`"
        ),
    )?;
    check_layout(root)?;
    fs::write(
        root.join("AGENTS.md"),
        routes.replace(native_path, "obsolete/native.md"),
    )?;
    assert_eq!(
        check_layout(root)
            .err()
            .context("missing native route accepted")?
            .to_string(),
        format!("missing AGENTS.md route: {native_path}")
    );
    for malformed_routes in [routes.replace('(', ""), routes.replace(')', "")] {
        fs::write(root.join("AGENTS.md"), malformed_routes)?;
        ensure!(check_layout(root).is_err(), "malformed route accepted");
    }
    fs::write(root.join("AGENTS.md"), routes)?;
    fs::remove_file(root.join(native_path))?;
    assert_eq!(
        check_layout(root)
            .err()
            .context("missing native file accepted")?
            .to_string(),
        format!("read routed file: {native_path}")
    );
    fs::write(root.join(native_path), [0xff_u8, 0xfe])?;
    ensure!(
        check_layout(root).is_err(),
        "invalid UTF-8 guidance accepted"
    );
    fs::write(root.join(native_path), "native procedure")?;
    check_layout(root)?;
    for (name, path) in fixture_skills {
        fs::write(
            root.join("AGENTS.md"),
            routes.replace(path, "obsolete/SKILL.md"),
        )?;
        assert_eq!(
            check_layout(root)
                .err()
                .with_context(|| format!("missing route accepted: {path}"))?
                .to_string(),
            format!("missing AGENTS.md route: {path}")
        );
        fs::write(root.join("AGENTS.md"), routes)?;
        let original = fs::read_to_string(root.join(path))?;
        fs::remove_file(root.join(path))?;
        assert_eq!(
            check_layout(root)
                .err()
                .with_context(|| format!("missing file accepted: {path}"))?
                .to_string(),
            format!("read routed file: {path}")
        );
        for (bad, error) in [
            (
                format!("garbage before fence\n{original}"),
                "missing skill frontmatter",
            ),
            (
                original.replace("\n---\n", "\n---garbage\n"),
                "missing skill frontmatter",
            ),
            (
                original.replace("description: fixture control", "description:    "),
                "missing skill description",
            ),
            (
                original.replace(&format!("name: {name}"), &format!("name: {name}-obsolete")),
                "skill name does not match directory",
            ),
            (
                original.replace(&format!("name: {name}"), "name: obsolete"),
                "skill name does not match directory",
            ),
            (
                original.replace("description: fixture control", "description: "),
                "missing skill description",
            ),
            (
                original.replace(
                    &format!("name: {name}"),
                    &format!("name: {name}\nname: obsolete"),
                ),
                "duplicate skill name",
            ),
            (
                original.replace(
                    &format!("name: {name}"),
                    &format!("name: obsolete\nname: {name}"),
                ),
                "duplicate skill name",
            ),
            (
                original.replace(
                    &format!("name: {name}"),
                    &format!("name: {name}\nname: {name}"),
                ),
                "duplicate skill name",
            ),
            (
                original.replace(
                    "description: fixture control",
                    "description: fixture control\ndescription: ",
                ),
                "duplicate skill description",
            ),
            ("no frontmatter".to_owned(), "missing skill frontmatter"),
        ] {
            fs::write(root.join(path), bad)?;
            assert_eq!(
                check_layout(root)
                    .err()
                    .with_context(|| format!("invalid metadata accepted: {path}"))?
                    .to_string(),
                format!("{error}: {path}")
            );
        }
        fs::write(root.join(path), original)?;
        check_layout(root)?;
    }
    Ok(())
}
