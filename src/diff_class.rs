//! Diff classification: file type detection, language mix, diff class
//! derivation, and path predicates (cleanup train step 24, pure code motion).

use crate::*;

pub(crate) fn classify_diff(files: &[String], patch: &str) -> DiffFlags {
    let mut flags = DiffFlags {
        docs_only: !files.is_empty(),
        ..DiffFlags::default()
    };
    for path in files {
        let lower = path.to_ascii_lowercase();
        flags.docs_only &= is_doc_path(&lower);
        flags.source_changed |= is_source_path(&lower);
        flags.rust_changed |= lower.ends_with(".rs");
        flags.rust_tests_changed |=
            lower.ends_with(".rs") && (lower.contains("test") || lower.contains("tests/"));
        flags.workflow_changed |= lower.starts_with(".github/workflows/")
            || lower.ends_with("action.yml")
            || lower.ends_with("action.yaml");
        flags.dependency_changed |= is_dependency_path(&lower);
        flags.shell_changed |= lower.ends_with(".sh") || lower.starts_with("scripts/");
        flags.cpp_changed |= is_cpp_path(&lower);
        flags.unsafe_or_native_risk |= is_native_risk_path(&lower);
    }
    if patch_tokens_can_promote_native_risk(files, &flags)
        && patch_contains_native_risk_token(patch)
    {
        flags.unsafe_or_native_risk = true;
    }
    flags
}

pub(crate) fn classify_language_mix(files: &[String]) -> LanguageMix {
    let mut language_counts = BTreeMap::<&'static str, usize>::new();
    let mut surfaces = BTreeSet::<&'static str>::new();

    for path in files {
        let lower = path.to_ascii_lowercase();
        if let Some(language) = language_for_path(&lower) {
            *language_counts.entry(language).or_default() += 1;
        }
        for surface in surfaces_for_path(&lower) {
            surfaces.insert(surface);
        }
    }

    let languages = language_counts
        .keys()
        .map(|language| (*language).to_owned())
        .collect::<Vec<_>>();
    let primary_language = language_counts
        .iter()
        .fold(
            None::<(&'static str, usize)>,
            |best, (language, count)| match best {
                Some((best_language, best_count))
                    if best_count > *count
                        || (best_count == *count && best_language <= *language) =>
                {
                    Some((best_language, best_count))
                }
                _ => Some((*language, *count)),
            },
        )
        .map(|(language, _count)| language.to_owned());

    LanguageMix {
        mixed_language: languages.len() > 1,
        languages,
        primary_language,
        surfaces: surfaces.into_iter().map(str::to_owned).collect::<Vec<_>>(),
    }
}

pub(crate) fn classify_diff_class(files: &[String], flags: &DiffFlags) -> DiffClass {
    if files.is_empty() {
        return DiffClass::ArtifactOnlySmoke;
    }
    if flags.docs_only {
        return DiffClass::DocsOnly;
    }
    if files.iter().all(|path| is_workflow_tooling_path(path)) {
        return DiffClass::WorkflowTooling;
    }
    if files.iter().all(|path| is_test_or_fixture_path(path)) {
        return DiffClass::TestsOnly;
    }
    if flags.unsafe_or_native_risk {
        return DiffClass::SourceUb;
    }
    DiffClass::SourceGeneral
}

pub(crate) fn is_source_path(path: &str) -> bool {
    [
        ".rs", ".zig", ".cpp", ".cc", ".c", ".h", ".hpp", ".ts", ".tsx", ".js", ".jsx", ".go",
        ".py",
    ]
    .iter()
    .any(|suffix| path.ends_with(suffix))
}

pub(crate) fn language_for_path(path: &str) -> Option<&'static str> {
    if path.ends_with(".rs") {
        Some("rust")
    } else if path.ends_with(".ts") || path.ends_with(".tsx") {
        Some("typescript")
    } else if path.ends_with(".js")
        || path.ends_with(".jsx")
        || path.ends_with(".mjs")
        || path.ends_with(".cjs")
    {
        Some("javascript")
    } else if is_cpp_path(path) {
        Some("c-cpp")
    } else if path.ends_with(".zig") {
        Some("zig")
    } else if path.ends_with(".go") {
        Some("go")
    } else if path.ends_with(".py") {
        Some("python")
    } else if path.ends_with(".sh") {
        Some("shell")
    } else if path.ends_with(".yml") || path.ends_with(".yaml") {
        Some("yaml")
    } else if path.ends_with(".toml") {
        Some("toml")
    } else if path.ends_with(".json") {
        Some("json")
    } else if path.ends_with(".md") {
        Some("markdown")
    } else {
        None
    }
}

pub(crate) fn surfaces_for_path(path: &str) -> Vec<&'static str> {
    let mut surfaces = Vec::new();
    if is_doc_path(path) {
        surfaces.push("docs");
    }
    if is_dependency_path(path) {
        surfaces.push("dependencies");
    }
    if path.starts_with(".github/workflows/") {
        surfaces.push("workflow");
    }
    if path.starts_with(".github/actions/")
        || path.ends_with("action.yml")
        || path.ends_with("action.yaml")
    {
        surfaces.push("action");
    }
    if path.contains("/fixtures/") || path.starts_with("fixtures/") {
        surfaces.push("fixtures");
    }
    if path.contains("/test/")
        || path.contains("/tests/")
        || path.starts_with("test/")
        || path.starts_with("tests/")
        || path.ends_with(".test.ts")
        || path.ends_with(".test.js")
        || path.ends_with("_test.rs")
    {
        surfaces.push("tests");
    }
    if path.ends_with(".sh") || path.starts_with("scripts/") {
        surfaces.push("scripts");
    }
    if path.starts_with("configs/") {
        surfaces.push("config");
    }
    if is_source_path(path) && !surfaces.contains(&"tests") {
        surfaces.push("source");
    }
    if surfaces.is_empty() {
        surfaces.push("other");
    }
    surfaces
}

pub(crate) fn is_doc_path(path: &str) -> bool {
    path.ends_with(".md") || path.starts_with("docs/")
}

pub(crate) fn is_cpp_path(path: &str) -> bool {
    [".c", ".cc", ".cpp", ".cxx", ".h", ".hpp"]
        .iter()
        .any(|suffix| path.ends_with(suffix))
}

pub(crate) fn is_zig_path(path: &str) -> bool {
    path.ends_with(".zig")
}

pub(crate) fn is_native_risk_path(path: &str) -> bool {
    path.contains("ffi")
        || path.contains("jsc")
        || path.contains("arraybuffer")
        || path.contains("typedarray")
        || path.contains("worker")
        || path.contains("crypto")
        || path.contains("zstd")
        || path.contains("src/runtime/")
        || path.contains("src/bun.js/bindings/")
}

pub(crate) fn patch_tokens_can_promote_native_risk(files: &[String], flags: &DiffFlags) -> bool {
    flags.rust_changed
        || flags.cpp_changed
        || files.iter().any(|path| {
            let lower = path.to_ascii_lowercase();
            is_zig_path(&lower) || is_native_risk_path(&lower)
        })
}

pub(crate) fn patch_contains_native_risk_token(patch: &str) -> bool {
    let lower_patch = patch.to_ascii_lowercase();
    [
        "unsafe",
        "extern",
        "from_raw_parts",
        "as_ptr",
        "as_mut_ptr",
        "maybeuninit",
        "nonnull",
        "arraybuffer",
        "typedarray",
        "detach",
        "resize",
        "transfer",
        "protect",
        "unprotect",
        "worker",
        "ffi",
        "jsc",
        "stringorbuffer",
        "sharedarraybuffer",
    ]
    .iter()
    .any(|token| lower_patch.contains(token))
}

pub(crate) fn is_dependency_path(path: &str) -> bool {
    matches!(
        path,
        "cargo.lock"
            | "cargo.toml"
            | "package.json"
            | "bun.lock"
            | "bun.lockb"
            | "pnpm-lock.yaml"
            | "yarn.lock"
            | "package-lock.json"
    ) || path.ends_with("/cargo.toml")
        || path.ends_with("/cargo.lock")
}

pub(crate) fn is_workflow_tooling_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.starts_with(".github/workflows/")
        || lower.starts_with(".github/actions/")
        || lower.ends_with("action.yml")
        || lower.ends_with("action.yaml")
        || lower.starts_with("scripts/")
        || lower.starts_with("configs/")
        || matches!(
            lower.as_str(),
            "justfile"
                | "makefile"
                | "dockerfile"
                | ".github/dependabot.yml"
                | ".github/dependabot.yaml"
        )
}

pub(crate) fn is_github_workflow_file(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.starts_with(".github/workflows/") && (lower.ends_with(".yml") || lower.ends_with(".yaml"))
}

pub(crate) fn is_test_or_fixture_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.contains("/test/")
        || lower.contains("/tests/")
        || lower.starts_with("test/")
        || lower.starts_with("tests/")
        || lower.contains("/fixtures/")
        || lower.starts_with("fixtures/")
        || lower.ends_with(".snap")
        || lower.ends_with(".test.ts")
        || lower.ends_with(".test.js")
        || lower.ends_with("_test.rs")
}
