//! JS/TS focused test name parsing and command display utilities
//! (cleanup train step 44, pure code motion).

use crate::*;

pub(crate) fn focused_test_names_for_file(patch: &str, file: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut current_path = String::new();
    for line in patch.lines() {
        if let Some(path) = line.strip_prefix("+++ b/") {
            current_path = normalize_repo_path(path);
            continue;
        }
        if current_path != file || !line.starts_with('+') || line.starts_with("+++") {
            continue;
        }
        if let Some(name) = extract_focused_test_name(&line[1..]) {
            push_unique(&mut names, &name);
        }
    }
    names
}

pub(crate) fn extract_focused_test_name(line: &str) -> Option<String> {
    let trimmed = line.trim_start();
    for callee in ["test", "it", "describe"] {
        if let Some(rest) = trimmed.strip_prefix(callee)
            && let Some(call_args) = strip_focused_test_callee_prefix(rest)
            && let Some(name) = parse_js_string_literal(call_args.trim_start())
        {
            return Some(name);
        }
    }
    None
}

pub(crate) fn strip_focused_test_callee_prefix(mut rest: &str) -> Option<&str> {
    loop {
        let trimmed = rest.trim_start();
        if let Some(call_args) = trimmed.strip_prefix('(') {
            return Some(call_args);
        }
        let after_dot = trimmed.strip_prefix('.')?;
        let (modifier, after_modifier) = parse_js_identifier(after_dot)?;
        if is_simple_test_modifier(modifier) {
            rest = after_modifier;
            continue;
        }
        if is_parameterized_test_modifier(modifier) {
            rest = strip_balanced_js_call(after_modifier.trim_start())?;
            continue;
        }
        return None;
    }
}

pub(crate) fn parse_js_identifier(text: &str) -> Option<(&str, &str)> {
    let end = text
        .char_indices()
        .take_while(|(_, ch)| ch.is_ascii_alphanumeric() || *ch == '_' || *ch == '$')
        .map(|(index, ch)| index + ch.len_utf8())
        .last()?;
    Some((&text[..end], &text[end..]))
}

pub(crate) fn is_simple_test_modifier(value: &str) -> bool {
    matches!(
        value,
        "only" | "skip" | "todo" | "failing" | "concurrent" | "serial"
    )
}

pub(crate) fn is_parameterized_test_modifier(value: &str) -> bool {
    matches!(value, "each")
}

pub(crate) fn strip_balanced_js_call(text: &str) -> Option<&str> {
    if !text.starts_with('(') {
        return None;
    }
    let mut depth = 0_u32;
    let mut quote = None;
    let mut escaped = false;
    for (index, ch) in text.char_indices() {
        if let Some(active_quote) = quote {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == active_quote {
                quote = None;
            }
            continue;
        }
        match ch {
            '"' | '\'' | '`' => quote = Some(ch),
            '(' => depth = depth.saturating_add(1),
            ')' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(&text[index + ch.len_utf8()..]);
                }
            }
            _ => {}
        }
    }
    None
}

pub(crate) fn parse_js_string_literal(text: &str) -> Option<String> {
    let mut chars = text.chars();
    let quote = chars.next()?;
    if !matches!(quote, '"' | '\'') {
        return None;
    }
    let mut escaped = false;
    let mut out = String::new();
    for ch in chars {
        if escaped {
            out.push(ch);
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == quote {
            return Some(out.trim().to_owned()).filter(|value| !value.is_empty());
        } else {
            out.push(ch);
        }
    }
    None
}

pub(crate) fn command_display(argv: &[String]) -> String {
    argv.iter()
        .map(|part| {
            if part
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '/' | '.' | '_' | '-' | ':'))
            {
                part.clone()
            } else {
                format!("'{}'", part.replace('\'', "'\\''"))
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub(crate) fn command_display_with_env(env: &BTreeMap<String, String>, argv: &[String]) -> String {
    if env.is_empty() {
        return command_display(argv);
    }
    let mut parts = env
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>();
    parts.push(command_display(argv));
    parts.join(" ")
}

pub(crate) fn push_unique(values: &mut Vec<String>, value: &str) {
    if !values.iter().any(|existing| existing == value) {
        values.push(value.to_owned());
    }
}
