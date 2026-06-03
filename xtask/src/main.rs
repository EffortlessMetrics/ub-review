use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use toml::Value;
use toml::map::Map;

fn main() {
    if let Err(error) = run() {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let mut args = env::args().skip(1);
    let command = args.next().unwrap_or_else(|| "help".to_owned());
    let root = env::current_dir().context("resolve current directory")?;

    match command.as_str() {
        "policy-check" => {
            reject_extra_args(args)?;
            let report = check_policy(&root)?;
            println!("{}", report.summary());
        }
        "policy-inventory" => {
            reject_extra_args(args)?;
            let report = check_policy(&root)?;
            print!("{}", report.inventory());
        }
        "help" | "-h" | "--help" => {
            reject_extra_args(args)?;
            print_help();
        }
        other => {
            bail!(
                "unknown xtask command `{other}`; expected policy-check, policy-inventory, or help"
            )
        }
    }

    Ok(())
}

fn reject_extra_args(mut args: impl Iterator<Item = String>) -> Result<()> {
    if let Some(extra) = args.next() {
        bail!("unexpected argument `{extra}`");
    }
    Ok(())
}

fn print_help() {
    println!(
        "\
cargo xtask commands

  cargo xtask policy-check      parse and validate repo policy receipts
  cargo xtask policy-inventory  print receipt and CI policy counts
"
    );
}

#[derive(Debug, Default)]
struct PolicyReport {
    policy_files: usize,
    exceptions: usize,
    exception_kinds: BTreeMap<String, usize>,
    ci_lanes: usize,
    implemented_lanes: usize,
    risk_packs: usize,
}

impl PolicyReport {
    fn summary(&self) -> String {
        format!(
            "policy check passed: {} policy files, {} allow receipts, {} CI lanes, {} risk packs",
            self.policy_files, self.exceptions, self.ci_lanes, self.risk_packs
        )
    }

    fn inventory(&self) -> String {
        let mut text = String::new();
        text.push_str("# Policy inventory\n\n");
        text.push_str(&format!("- policy files: {}\n", self.policy_files));
        text.push_str(&format!("- allow receipts: {}\n", self.exceptions));
        for (kind, count) in &self.exception_kinds {
            text.push_str(&format!("  - {kind}: {count}\n"));
        }
        text.push_str(&format!("- CI lanes: {}\n", self.ci_lanes));
        text.push_str(&format!(
            "- implemented CI lanes: {}\n",
            self.implemented_lanes
        ));
        text.push_str(&format!("- CI risk packs: {}\n", self.risk_packs));
        text
    }
}

fn check_policy(root: &Path) -> Result<PolicyReport> {
    let policy_dir = root.join("policy");
    let mut report = PolicyReport::default();

    for file in policy_files(&policy_dir)? {
        parse_toml(&file)?;
        report.policy_files += 1;
    }

    validate_allow(&policy_dir.join("allow.toml"), &mut report)?;
    validate_ci_budget(&policy_dir.join("ci-budget.toml"))?;
    validate_ci_lanes(&policy_dir.join("ci-lanes.toml"), &mut report)?;
    validate_ci_risk_packs(&policy_dir.join("ci-risk-packs.toml"), &mut report)?;

    Ok(report)
}

fn policy_files(policy_dir: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in fs::read_dir(policy_dir)
        .with_context(|| format!("read policy directory {}", policy_dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) == Some("toml") {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

fn parse_toml(path: &Path) -> Result<Value> {
    let text = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    toml::from_str(&text).with_context(|| format!("parse {}", path.display()))
}

fn validate_allow(path: &Path, report: &mut PolicyReport) -> Result<()> {
    let value = parse_toml(path)?;
    let root = table(&value, path, "root")?;
    require_schema_version(root, path)?;
    require_str(root, path, "tool")?;
    let exceptions = array(root, path, "exception")?;
    let mut ids = BTreeSet::new();

    for (index, exception) in exceptions.iter().enumerate() {
        let context = format!("exception[{index}]");
        let item = table(exception, path, &context)?;
        let id = require_str(item, path, "id")?;
        if !ids.insert(id.to_owned()) {
            bail!("{} duplicate exception id `{id}`", path.display());
        }
        let kind = require_str(item, path, "kind")?;
        require_str(item, path, "owner")?;
        require_str(item, path, "reason")?;
        require_str(item, path, "created")?;
        require_str(item, path, "review_after")?;
        if item.get("path").is_none() && item.get("glob").is_none() {
            bail!(
                "{} exception `{id}` must include either `path` or `glob`",
                path.display()
            );
        }
        if let Some(expires) = item.get("expires") {
            non_empty_str(expires, path, "expires")?;
        }
        *report.exception_kinds.entry(kind.to_owned()).or_insert(0) += 1;
        report.exceptions += 1;
    }

    Ok(())
}

fn validate_ci_budget(path: &Path) -> Result<()> {
    let value = parse_toml(path)?;
    let root = table(&value, path, "root")?;
    require_schema_version(root, path)?;
    let budget = table_key(root, path, "budget")?;
    require_integer(budget, path, "preferred_default_lem")?;
    require_integer(budget, path, "default_limit_lem")?;
    require_integer(budget, path, "elevated_limit_lem")?;
    require_integer(budget, path, "hard_limit_lem")?;
    table_key(root, path, "bands")?;
    Ok(())
}

fn validate_ci_lanes(path: &Path, report: &mut PolicyReport) -> Result<()> {
    let value = parse_toml(path)?;
    let root = table(&value, path, "root")?;
    require_schema_version(root, path)?;
    require_str(root, path, "summary_check")?;
    let lanes = array(root, path, "lane")?;
    let mut ids = BTreeSet::new();

    for (index, lane) in lanes.iter().enumerate() {
        let context = format!("lane[{index}]");
        let item = table(lane, path, &context)?;
        let id = require_str(item, path, "id")?;
        if !ids.insert(id.to_owned()) {
            bail!("{} duplicate lane id `{id}`", path.display());
        }
        require_str(item, path, "when")?;
        require_bool(item, path, "target_required")?;
        if require_bool(item, path, "implemented")? {
            report.implemented_lanes += 1;
        }
        require_str(item, path, "reason")?;
        report.ci_lanes += 1;
    }

    Ok(())
}

fn validate_ci_risk_packs(path: &Path, report: &mut PolicyReport) -> Result<()> {
    let value = parse_toml(path)?;
    let root = table(&value, path, "root")?;
    require_schema_version(root, path)?;
    let packs = array(root, path, "risk_pack")?;
    let mut ids = BTreeSet::new();

    for (index, pack) in packs.iter().enumerate() {
        let context = format!("risk_pack[{index}]");
        let item = table(pack, path, &context)?;
        let id = require_str(item, path, "id")?;
        if !ids.insert(id.to_owned()) {
            bail!("{} duplicate risk_pack id `{id}`", path.display());
        }
        require_string_array(item, path, "labels")?;
        require_string_array(item, path, "lanes")?;
        require_str(item, path, "reason")?;
        report.risk_packs += 1;
    }

    Ok(())
}

fn require_schema_version(table: &Map<String, Value>, path: &Path) -> Result<()> {
    let version = require_integer(table, path, "schema_version")?;
    if version != 1 {
        bail!(
            "{} expected schema_version = 1, found {version}",
            path.display()
        );
    }
    Ok(())
}

fn table<'a>(value: &'a Value, path: &Path, context: &str) -> Result<&'a Map<String, Value>> {
    value
        .as_table()
        .with_context(|| format!("{} {context} must be a TOML table", path.display()))
}

fn table_key<'a>(
    table: &'a Map<String, Value>,
    path: &Path,
    key: &str,
) -> Result<&'a Map<String, Value>> {
    let value = table
        .get(key)
        .with_context(|| format!("{} missing `{key}`", path.display()))?;
    value
        .as_table()
        .with_context(|| format!("{} `{key}` must be a table", path.display()))
}

fn array<'a>(table: &'a Map<String, Value>, path: &Path, key: &str) -> Result<&'a [Value]> {
    let values = table
        .get(key)
        .with_context(|| format!("{} missing `[[{key}]]` entries", path.display()))?
        .as_array()
        .with_context(|| format!("{} `{key}` must be an array", path.display()))?;
    if values.is_empty() {
        bail!("{} `{key}` must not be empty", path.display());
    }
    Ok(values)
}

fn require_str<'a>(table: &'a Map<String, Value>, path: &Path, key: &str) -> Result<&'a str> {
    let value = table
        .get(key)
        .with_context(|| format!("{} missing `{key}`", path.display()))?;
    non_empty_str(value, path, key)
}

fn non_empty_str<'a>(value: &'a Value, path: &Path, key: &str) -> Result<&'a str> {
    let text = value
        .as_str()
        .with_context(|| format!("{} `{key}` must be a string", path.display()))?
        .trim();
    if text.is_empty() {
        bail!("{} `{key}` must not be empty", path.display());
    }
    Ok(text)
}

fn require_integer(table: &Map<String, Value>, path: &Path, key: &str) -> Result<i64> {
    table
        .get(key)
        .with_context(|| format!("{} missing `{key}`", path.display()))?
        .as_integer()
        .with_context(|| format!("{} `{key}` must be an integer", path.display()))
}

fn require_bool(table: &Map<String, Value>, path: &Path, key: &str) -> Result<bool> {
    table
        .get(key)
        .with_context(|| format!("{} missing `{key}`", path.display()))?
        .as_bool()
        .with_context(|| format!("{} `{key}` must be a boolean", path.display()))
}

fn require_string_array(table: &Map<String, Value>, path: &Path, key: &str) -> Result<()> {
    let values = table
        .get(key)
        .with_context(|| format!("{} missing `{key}`", path.display()))?
        .as_array()
        .with_context(|| format!("{} `{key}` must be an array", path.display()))?;
    if values.is_empty() {
        bail!("{} `{key}` must not be empty", path.display());
    }
    for value in values {
        non_empty_str(value, path, key)?;
    }
    Ok(())
}
