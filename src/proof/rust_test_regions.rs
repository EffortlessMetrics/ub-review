//! Rust test-region scanning for the base+tests proof patch.
//!
//! Rust unit tests conventionally live in a `#[cfg(test)] mod tests` block
//! inside the production source file, so a path prefix cannot tell test code
//! from production code. This module locates the `#[cfg(test)]` / `mod tests` /
//! `#[test]` spans of one Rust file by line number, so a diff hunk can be
//! classified by where it lands instead of by which directory it lives in.
//!
//! The scanner is deliberately conservative: any construct it cannot lex or
//! brace-match makes it return `None`, and callers must treat `None` as
//! "cannot classify this file" rather than as "no test code here".

/// Inclusive 1-based line span of a Rust test region.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct TestRegion {
    pub(crate) start: usize,
    pub(crate) end: usize,
}

/// Test regions of one Rust source file, or `None` when the file cannot be
/// scanned confidently (unterminated string/comment, unbalanced braces, an
/// item whose end could not be found).
pub(crate) fn rust_test_regions(content: &str) -> Option<Vec<TestRegion>> {
    let lines = rust_code_lines(content)?;
    let mut regions = Vec::new();
    let mut attr_start: Option<usize> = None;
    let mut attr_is_test = false;
    let mut index = 0;
    while index < lines.len() {
        let line = lines.get(index)?;
        let code = line.code.trim();
        if code.is_empty() {
            index += 1;
            continue;
        }
        if code.starts_with("#[") || code.starts_with("#![") {
            let inner_attribute = code.starts_with("#![");
            let end = attribute_end_line(&lines, index)?;
            let mut text = String::new();
            for (offset, attr_line) in lines.get(index..=end)?.iter().enumerate() {
                let code = attr_line.code.trim();
                // Any item sharing the attribute's last line must not be read
                // as part of the attribute (`#[test] fn t() { .. }`).
                let code = if index + offset == end {
                    code.rfind(']')
                        .and_then(|bracket| code.get(..=bracket))
                        .unwrap_or(code)
                } else {
                    code
                };
                text.push_str(code);
                text.push(' ');
            }
            if is_test_attribute(&text) {
                if inner_attribute && line.depth_before == 0 {
                    // `#![cfg(test)]` makes the whole file test code.
                    return Some(vec![TestRegion {
                        start: 1,
                        end: lines.len().max(1),
                    }]);
                }
                attr_is_test = true;
            }
            if attr_start.is_none() {
                attr_start = Some(index);
            }
            if code_after_last_attribute_bracket(lines.get(end)?).is_empty() {
                index = end + 1;
                continue;
            }
            // The item shares the attribute's last line (`#[test] fn t() {}`).
            index = end;
        }
        let line = lines.get(index)?;
        if attr_is_test || is_test_mod_declaration(code_after_last_attribute_bracket(line)) {
            let start = attr_start.unwrap_or(index);
            let end = item_end_line(&lines, index)?;
            regions.push(TestRegion {
                start: start + 1,
                end: end + 1,
            });
            index = end + 1;
        } else {
            index += 1;
        }
        attr_start = None;
        attr_is_test = false;
    }
    Some(regions)
}

pub(crate) fn line_in_test_regions(regions: &[TestRegion], line: usize) -> bool {
    regions
        .iter()
        .any(|region| line >= region.start && line <= region.end)
}

/// Code on a line after its last attribute-closing bracket, so a line that
/// carries both an attribute and its item can be inspected as an item.
fn code_after_last_attribute_bracket(line: &CodeLine) -> &str {
    let code = line.code.trim();
    if !code.starts_with("#[") && !code.starts_with("#![") {
        return code;
    }
    code.rfind(']')
        .and_then(|index| code.get(index + 1..))
        .unwrap_or("")
        .trim()
}

/// True when the attribute text names a test-only conditional or a test item.
///
/// `#[cfg(not(test))]` is production code, so any `test` token that sits inside
/// a `not(...)` group disqualifies the attribute.
fn is_test_attribute(attribute: &str) -> bool {
    let inner = attribute
        .trim()
        .trim_start_matches('#')
        .trim_start_matches('!')
        .trim_start_matches('[')
        .trim()
        .trim_end_matches(']')
        .trim();
    if inner.starts_with("cfg(") || inner.starts_with("cfg_attr(") {
        if negated_groups_contain_test(inner) {
            return false;
        }
        return contains_identifier(inner, "test");
    }
    let mut path = inner;
    if let Some((head, _)) = path.split_once('(') {
        path = head;
    }
    if let Some((head, _)) = path.split_once('=') {
        path = head;
    }
    path.trim().rsplit("::").next().map(str::trim) == Some("test")
}

fn negated_groups_contain_test(inner: &str) -> bool {
    inner.match_indices("not(").any(|(index, _)| {
        if inner
            .get(..index)
            .and_then(|head| head.chars().next_back())
            .is_some_and(is_identifier_char)
        {
            return false;
        }
        let Some(rest) = inner.get(index + 3..) else {
            return false;
        };
        let mut depth = 0_usize;
        let mut close = None;
        for (offset, ch) in rest.char_indices() {
            match ch {
                '(' => depth += 1,
                ')' => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        close = Some(offset);
                        break;
                    }
                }
                _ => {}
            }
        }
        let group = close.and_then(|offset| rest.get(..offset)).unwrap_or(rest);
        contains_identifier(group, "test")
    })
}

fn contains_identifier(haystack: &str, needle: &str) -> bool {
    haystack.match_indices(needle).any(|(index, matched)| {
        let before_free = haystack
            .get(..index)
            .and_then(|head| head.chars().next_back())
            .is_none_or(|ch| !is_identifier_char(ch));
        let after_free = haystack
            .get(index + matched.len()..)
            .and_then(|tail| tail.chars().next())
            .is_none_or(|ch| !is_identifier_char(ch));
        before_free && after_free
    })
}

fn is_identifier_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

/// True for `mod tests {` / `pub mod test {` style declarations, which own the
/// conventional Rust unit-test block even without a `#[cfg(test)]` attribute.
fn is_test_mod_declaration(code: &str) -> bool {
    let code = code.trim();
    if !code.contains('{') {
        return false;
    }
    let mut rest = code;
    if let Some(stripped) = rest.strip_prefix("pub") {
        rest = stripped.trim_start();
        if let Some(stripped) = rest.strip_prefix('(') {
            let Some((_, after)) = stripped.split_once(')') else {
                return false;
            };
            rest = after.trim_start();
        }
    }
    let Some(rest) = rest.strip_prefix("mod") else {
        return false;
    };
    let rest = rest.trim_start();
    let name: String = rest
        .chars()
        .take_while(|ch| is_identifier_char(*ch))
        .collect();
    matches!(name.as_str(), "test" | "tests")
}

/// Last line of the (possibly multi-line) attribute starting at `start`.
fn attribute_end_line(lines: &[CodeLine], start: usize) -> Option<usize> {
    let mut depth = 0_isize;
    for (offset, line) in lines.get(start..)?.iter().enumerate() {
        for ch in line.code.chars() {
            match ch {
                '[' => depth += 1,
                ']' => depth -= 1,
                _ => {}
            }
        }
        if depth <= 0 {
            return Some(start + offset);
        }
    }
    None
}

/// Last line of the item starting at `start`, using brace depth for items with
/// a body and the statement terminator for items without one.
fn item_end_line(lines: &[CodeLine], start: usize) -> Option<usize> {
    let base = lines.get(start)?.depth_before;
    let mut opened = false;
    for (offset, line) in lines.get(start..)?.iter().enumerate() {
        if line.depth_after > base {
            opened = true;
            continue;
        }
        if line.depth_after < base {
            return None;
        }
        if opened || line.code.contains('{') {
            return Some(start + offset);
        }
        if line.code.trim_end().ends_with(';') {
            return Some(start + offset);
        }
    }
    None
}

/// One source line with string/comment content blanked out, plus the brace
/// depth before and after it.
struct CodeLine {
    code: String,
    depth_before: usize,
    depth_after: usize,
}

enum ScanState {
    Code,
    BlockComment(usize),
    Str,
    RawStr(usize),
}

/// Blank out comments, string literals, and char literals so brace depth and
/// attribute detection only see real code. Returns `None` when the file does
/// not lex to a balanced, closed state.
fn rust_code_lines(content: &str) -> Option<Vec<CodeLine>> {
    let mut pieces: Vec<&str> = content.split('\n').collect();
    if pieces.last() == Some(&"") {
        pieces.pop();
    }
    let mut state = ScanState::Code;
    let mut depth = 0_usize;
    let mut out = Vec::with_capacity(pieces.len());
    for piece in pieces {
        let chars: Vec<char> = piece.chars().collect();
        let depth_before = depth;
        let mut code = String::with_capacity(chars.len());
        let mut index = 0;
        while index < chars.len() {
            let ch = *chars.get(index)?;
            match state {
                ScanState::BlockComment(nesting) => {
                    if ch == '*' && chars.get(index + 1) == Some(&'/') {
                        state = if nesting <= 1 {
                            ScanState::Code
                        } else {
                            ScanState::BlockComment(nesting - 1)
                        };
                        code.push_str("  ");
                        index += 2;
                        continue;
                    }
                    if ch == '/' && chars.get(index + 1) == Some(&'*') {
                        state = ScanState::BlockComment(nesting + 1);
                        code.push_str("  ");
                        index += 2;
                        continue;
                    }
                    code.push(' ');
                    index += 1;
                }
                ScanState::Str => {
                    if ch == '\\' {
                        code.push_str("  ");
                        index += 2;
                        continue;
                    }
                    if ch == '"' {
                        state = ScanState::Code;
                    }
                    code.push(' ');
                    index += 1;
                }
                ScanState::RawStr(hashes) => {
                    if ch == '"' && raw_string_closes_at(&chars, index + 1, hashes) {
                        state = ScanState::Code;
                        for _ in 0..=hashes {
                            code.push(' ');
                        }
                        index += 1 + hashes;
                        continue;
                    }
                    code.push(' ');
                    index += 1;
                }
                ScanState::Code => {
                    if ch == '/' && chars.get(index + 1) == Some(&'/') {
                        break;
                    }
                    if ch == '/' && chars.get(index + 1) == Some(&'*') {
                        state = ScanState::BlockComment(1);
                        code.push_str("  ");
                        index += 2;
                        continue;
                    }
                    if ch == '"' {
                        state = ScanState::Str;
                        code.push(' ');
                        index += 1;
                        continue;
                    }
                    if ch == 'r' && raw_string_prefix_allowed(&code) {
                        let mut hashes = 0_usize;
                        while chars.get(index + 1 + hashes) == Some(&'#') {
                            hashes += 1;
                        }
                        if chars.get(index + 1 + hashes) == Some(&'"') {
                            state = ScanState::RawStr(hashes);
                            for _ in 0..hashes + 2 {
                                code.push(' ');
                            }
                            index += hashes + 2;
                            continue;
                        }
                    }
                    if ch == '\'' {
                        if let Some(width) = char_literal_width(&chars, index) {
                            for _ in 0..width {
                                code.push(' ');
                            }
                            index += width;
                            continue;
                        }
                        // A lifetime, not a char literal.
                        code.push(' ');
                        index += 1;
                        continue;
                    }
                    if ch == '{' {
                        depth += 1;
                    }
                    if ch == '}' {
                        depth = depth.checked_sub(1)?;
                    }
                    code.push(ch);
                    index += 1;
                }
            }
        }
        out.push(CodeLine {
            code,
            depth_before,
            depth_after: depth,
        });
    }
    if !matches!(state, ScanState::Code) || depth != 0 {
        return None;
    }
    Some(out)
}

fn raw_string_closes_at(chars: &[char], from: usize, hashes: usize) -> bool {
    (0..hashes).all(|offset| chars.get(from + offset) == Some(&'#'))
}

/// A `r"…"` literal only starts where the preceding code character cannot be
/// part of an identifier, allowing the byte-string `br"…"` spelling.
fn raw_string_prefix_allowed(code: &str) -> bool {
    let mut tail = code.chars().rev();
    match tail.next() {
        None => true,
        Some('b') => !tail.next().is_some_and(is_identifier_char),
        Some(ch) => !is_identifier_char(ch),
    }
}

/// Total character width of a char literal starting at `start`, or `None` when
/// the quote begins a lifetime instead.
fn char_literal_width(chars: &[char], start: usize) -> Option<usize> {
    if chars.get(start) != Some(&'\'') {
        return None;
    }
    if chars.get(start + 1) == Some(&'\\') {
        // Skip the escaped character, then find the closing quote.
        let mut cursor = start + 3;
        while cursor <= start + 12 {
            match chars.get(cursor) {
                Some('\'') => return Some(cursor + 1 - start),
                Some(_) => cursor += 1,
                None => return None,
            }
        }
        return None;
    }
    if chars.get(start + 2) == Some(&'\'') {
        return Some(3);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn regions(content: &str) -> anyhow::Result<Vec<TestRegion>> {
        rust_test_regions(content).ok_or_else(|| anyhow::anyhow!("rust scan should succeed"))
    }

    #[test]
    fn rust_test_regions_finds_inline_cfg_test_module() -> anyhow::Result<()> {
        let content = "pub fn add(a: u8, b: u8) -> u8 {\n    a + b\n}\n\n#[cfg(test)]\nmod tests {\n    use super::*;\n\n    #[test]\n    fn adds() {\n        assert_eq!(add(1, 1), 2);\n    }\n}\n";
        let found = regions(content)?;
        assert_eq!(found, vec![TestRegion { start: 5, end: 13 }]);
        assert!(!line_in_test_regions(&found, 2));
        assert!(line_in_test_regions(&found, 10));
        Ok(())
    }

    #[test]
    fn rust_test_regions_finds_bare_test_function() -> anyhow::Result<()> {
        let content = "fn helper() {}\n\n#[test]\nfn checks() {\n    helper();\n}\n";
        assert_eq!(regions(content)?, vec![TestRegion { start: 3, end: 6 }]);
        Ok(())
    }

    #[test]
    fn rust_test_regions_finds_nested_test_module() -> anyhow::Result<()> {
        let content = "mod outer {\n    pub fn f() {}\n\n    #[cfg(test)]\n    mod tests {\n        #[test]\n        fn t() {}\n    }\n}\n";
        assert_eq!(regions(content)?, vec![TestRegion { start: 4, end: 8 }]);
        Ok(())
    }

    #[test]
    fn rust_test_regions_ignores_braces_in_strings_and_comments() -> anyhow::Result<()> {
        let content = "pub fn f() -> &'static str {\n    // } not a brace\n    /* nested /* } */ still */\n    let _ = \"} {\";\n    let _ = r#\"} {\"#;\n    let _ = '}';\n    let _ = '\\'';\n    \"ok\"\n}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn t() {}\n}\n";
        assert_eq!(regions(content)?, vec![TestRegion { start: 11, end: 15 }]);
        Ok(())
    }

    #[test]
    fn rust_test_regions_rejects_unbalanced_source() {
        assert!(rust_test_regions("fn f() {\n").is_none());
        assert!(rust_test_regions("fn f() { /* unterminated\n").is_none());
    }

    #[test]
    fn rust_test_regions_treats_inner_cfg_test_attribute_as_whole_file() -> anyhow::Result<()> {
        let content = "#![cfg(test)]\n\nfn helper() {}\n";
        assert_eq!(regions(content)?, vec![TestRegion { start: 1, end: 3 }]);
        Ok(())
    }

    #[test]
    fn rust_test_regions_handles_attribute_and_item_on_one_line() -> anyhow::Result<()> {
        let content = "fn prod() {}\n#[test] fn t() { prod(); }\nfn other() {}\n";
        assert_eq!(regions(content)?, vec![TestRegion { start: 2, end: 2 }]);
        Ok(())
    }

    #[test]
    fn test_attribute_detection_excludes_negated_cfg() {
        assert!(is_test_attribute("#[cfg(test)]"));
        assert!(is_test_attribute("#[cfg(all(test, unix))]"));
        assert!(is_test_attribute("#[test]"));
        assert!(is_test_attribute("#[tokio::test]"));
        assert!(!is_test_attribute("#[cfg(not(test))]"));
        assert!(!is_test_attribute("#[cfg(feature = \"latest\")]"));
        assert!(!is_test_attribute("#[derive(Debug)]"));
        assert!(!is_test_attribute("#[cfg(testing_only)]"));
    }

    #[test]
    fn test_mod_declaration_detection_is_narrow() {
        assert!(is_test_mod_declaration("mod tests {"));
        assert!(is_test_mod_declaration("pub mod test {"));
        assert!(is_test_mod_declaration("pub(crate) mod tests {"));
        assert!(!is_test_mod_declaration("mod testing_utils {"));
        assert!(!is_test_mod_declaration("mod tests;"));
    }

    #[test]
    fn multi_line_attributes_are_grouped_with_their_item() -> anyhow::Result<()> {
        let content = "fn prod() {}\n\n#[cfg(test)]\n#[expect(\n    clippy::pedantic,\n    reason = \"test\"\n)]\nmod tests {\n    #[test]\n    fn t() {}\n}\n";
        assert_eq!(regions(content)?, vec![TestRegion { start: 3, end: 11 }]);
        Ok(())
    }
}
