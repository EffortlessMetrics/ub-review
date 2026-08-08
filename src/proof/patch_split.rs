//! Unified-diff parsing and hunk splitting for the base+tests proof patch.
//!
//! The base+tests patch has to carry a PR's test changes without carrying its
//! production changes, and in Rust those live in the same file. That means the
//! patch is assembled from a subset of a file's hunks, so this module parses
//! `git diff` output into addressable pieces, splits each hunk into the
//! independently applicable change runs that `git add -p` would offer, and
//! re-emits a chosen subset with recomputed hunk headers.
//!
//! Parsing is strict: anything unexpected is an error, because a mis-parsed
//! patch is how production code leaks into the base run.

use std::ops::Range;

use anyhow::{Result, bail, ensure};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PatchLineKind {
    Context,
    Deletion,
    Addition,
}

#[derive(Clone, Debug)]
pub(crate) struct PatchLine {
    pub(crate) kind: PatchLineKind,
    /// Raw diff line including its leading marker, without the line terminator.
    pub(crate) raw: String,
    /// `\ No newline at end of file` marker that followed this line.
    pub(crate) no_newline: bool,
    /// 1-based line number on the old side, when the line exists there.
    pub(crate) old_line: Option<usize>,
    /// 1-based line number on the new side, when the line exists there.
    pub(crate) new_line: Option<usize>,
}

#[derive(Clone, Debug)]
pub(crate) struct PatchHunk {
    pub(crate) old_start: usize,
    pub(crate) new_start: usize,
    /// Line counts the hunk header declared, checked against the body.
    declared_old_count: usize,
    declared_new_count: usize,
    /// Text following the closing `@@`, kept verbatim.
    pub(crate) section: String,
    pub(crate) lines: Vec<PatchLine>,
}

#[derive(Clone, Debug)]
pub(crate) struct PatchFile {
    /// Old-side path with its `a/` prefix removed.
    pub(crate) old_path: String,
    /// New-side path with its `b/` prefix removed.
    pub(crate) new_path: String,
    /// Every line before the first hunk, kept verbatim.
    pub(crate) header: Vec<String>,
    pub(crate) hunks: Vec<PatchHunk>,
    /// The whole file section exactly as `git diff` emitted it.
    pub(crate) raw: String,
}

impl PatchFile {
    pub(crate) fn header_has(&self, prefix: &str) -> bool {
        self.header.iter().any(|line| line.starts_with(prefix))
    }

    /// Changed-line text of the whole file section, used to decide whether a
    /// file we could not classify was nevertheless touching test code.
    pub(crate) fn changed_line_texts(&self) -> Vec<&str> {
        self.hunks
            .iter()
            .flat_map(|hunk| hunk.lines.iter())
            .filter(|line| line.kind != PatchLineKind::Context)
            .map(|line| line.raw.as_str())
            .collect()
    }
}

/// Parse `git diff` output into per-file sections.
pub(crate) fn parse_unified_diff(patch: &str) -> Result<Vec<PatchFile>> {
    let mut files: Vec<PatchFile> = Vec::new();
    let mut current: Option<PatchFile> = None;
    for line in patch_lines(patch) {
        if line.starts_with("diff --git ") {
            if let Some(file) = current.take() {
                files.push(finish_file(file)?);
            }
            let (old_path, new_path) = parse_diff_git_paths(line)?;
            current = Some(PatchFile {
                old_path,
                new_path,
                header: vec![line.to_owned()],
                hunks: Vec::new(),
                raw: format!("{line}\n"),
            });
            continue;
        }
        let Some(file) = current.as_mut() else {
            bail!("unified diff did not start with a `diff --git` header");
        };
        file.raw.push_str(line);
        file.raw.push('\n');
        if let Some(rest) = line.strip_prefix("@@") {
            file.hunks.push(parse_hunk_header(rest)?);
            continue;
        }
        let Some(hunk) = file.hunks.last_mut() else {
            file.header.push(line.to_owned());
            continue;
        };
        push_hunk_body_line(hunk, line)?;
    }
    if let Some(file) = current.take() {
        files.push(finish_file(file)?);
    }
    Ok(files)
}

fn patch_lines(patch: &str) -> impl Iterator<Item = &str> {
    let mut pieces: Vec<&str> = patch.split('\n').collect();
    if pieces.last() == Some(&"") {
        pieces.pop();
    }
    pieces.into_iter()
}

fn parse_diff_git_paths(line: &str) -> Result<(String, String)> {
    let rest = line
        .strip_prefix("diff --git ")
        .unwrap_or_default()
        .trim_end();
    // `git diff` quotes unusual paths; refuse rather than guess at the quoting.
    ensure!(
        !rest.contains('"'),
        "quoted paths in `diff --git {rest}` are not supported for hunk splitting"
    );
    let Some((old, new)) = rest.split_once(" b/") else {
        bail!("could not split `diff --git {rest}` into old and new paths");
    };
    let old = old.strip_prefix("a/").unwrap_or(old).trim_end().to_owned();
    Ok((old, new.trim().to_owned()))
}

fn parse_hunk_header(rest: &str) -> Result<PatchHunk> {
    let Some((ranges, section)) = rest.split_once("@@") else {
        bail!("hunk header `@@{rest}` had no closing `@@`");
    };
    let mut old_range = None;
    let mut new_range = None;
    for token in ranges.split_whitespace() {
        if let Some(value) = token.strip_prefix('-') {
            old_range = Some(parse_hunk_range(value)?);
        } else if let Some(value) = token.strip_prefix('+') {
            new_range = Some(parse_hunk_range(value)?);
        } else {
            bail!("unexpected token `{token}` in hunk header `@@{rest}`");
        }
    }
    let Some((old_start, declared_old_count)) = old_range else {
        bail!("hunk header `@@{rest}` had no old range");
    };
    let Some((new_start, declared_new_count)) = new_range else {
        bail!("hunk header `@@{rest}` had no new range");
    };
    Ok(PatchHunk {
        old_start,
        new_start,
        declared_old_count,
        declared_new_count,
        section: section.to_owned(),
        lines: Vec::new(),
    })
}

fn parse_hunk_range(value: &str) -> Result<(usize, usize)> {
    let (start, count) = match value.split_once(',') {
        Some((start, count)) => (start, count),
        None => (value, "1"),
    };
    let start = start
        .parse::<usize>()
        .map_err(|error| anyhow::anyhow!("hunk range start `{start}` is not a number: {error}"))?;
    let count = count
        .parse::<usize>()
        .map_err(|error| anyhow::anyhow!("hunk range count `{count}` is not a number: {error}"))?;
    Ok((start, count))
}

fn push_hunk_body_line(hunk: &mut PatchHunk, line: &str) -> Result<()> {
    if line.starts_with('\\') {
        let Some(last) = hunk.lines.last_mut() else {
            bail!("`{line}` appeared before any hunk body line");
        };
        last.no_newline = true;
        return Ok(());
    }
    let kind = match line.chars().next() {
        Some(' ') | None => PatchLineKind::Context,
        Some('-') => PatchLineKind::Deletion,
        Some('+') => PatchLineKind::Addition,
        Some(_) => bail!("unexpected hunk body line `{line}`"),
    };
    let next_old = hunk.old_start + count_side_lines(&hunk.lines, PatchLineKind::Deletion);
    let next_new = hunk.new_start + count_side_lines(&hunk.lines, PatchLineKind::Addition);
    let (old_line, new_line) = match kind {
        PatchLineKind::Context => (Some(next_old), Some(next_new)),
        PatchLineKind::Deletion => (Some(next_old), None),
        PatchLineKind::Addition => (None, Some(next_new)),
    };
    hunk.lines.push(PatchLine {
        kind,
        raw: line.to_owned(),
        no_newline: false,
        old_line,
        new_line,
    });
    Ok(())
}

/// Number of lines already present on the side that `changed` does not belong
/// to, i.e. context lines plus lines of that side's own kind.
fn count_side_lines(lines: &[PatchLine], changed: PatchLineKind) -> usize {
    lines
        .iter()
        .filter(|line| line.kind == PatchLineKind::Context || line.kind == changed)
        .count()
}

fn finish_file(mut file: PatchFile) -> Result<PatchFile> {
    // The `--- a/x` / `+++ b/y` lines each name one path, so they resolve the
    // `diff --git a/x b/y` ambiguity that paths containing " b/" would create.
    for line in &file.header {
        if let Some(path) = line.strip_prefix("--- ") {
            file.old_path = strip_diff_path_prefix(path, "a/");
        } else if let Some(path) = line.strip_prefix("+++ ") {
            file.new_path = strip_diff_path_prefix(path, "b/");
        }
    }
    for hunk in &file.hunks {
        ensure!(
            hunk.lines
                .iter()
                .any(|line| line.kind != PatchLineKind::Context),
            "hunk at old line {} of {} had no changed lines",
            hunk.old_start,
            file.new_path
        );
        let old_count = hunk
            .lines
            .iter()
            .filter(|line| line.kind != PatchLineKind::Addition)
            .count();
        let new_count = hunk
            .lines
            .iter()
            .filter(|line| line.kind != PatchLineKind::Deletion)
            .count();
        ensure!(
            old_count == hunk.declared_old_count && new_count == hunk.declared_new_count,
            "hunk at old line {} of {} declared -{},{} +{},{} but its body holds -{old_count} +{new_count} lines",
            hunk.old_start,
            file.new_path,
            hunk.old_start,
            hunk.declared_old_count,
            hunk.new_start,
            hunk.declared_new_count
        );
    }
    Ok(file)
}

/// `--- /dev/null` and `+++ /dev/null` mean the file is absent on that side, so
/// the path becomes empty.
fn strip_diff_path_prefix(path: &str, prefix: &str) -> String {
    let path = path.trim_end();
    let path = path
        .rsplit_once('\t')
        .map_or(path, |(head, _)| head)
        .trim_end();
    if path == "/dev/null" {
        return String::new();
    }
    path.strip_prefix(prefix).unwrap_or(path).to_owned()
}

/// Split a hunk into the change runs that can be applied independently: one run
/// of consecutive changed lines plus the context blocks on either side. This is
/// the split `git add -p` performs, and it keeps context shared between
/// neighbours so every produced fragment still matches the old file.
pub(crate) fn hunk_change_slices(hunk: &PatchHunk) -> Vec<Range<usize>> {
    let mut runs: Vec<Range<usize>> = Vec::new();
    let mut start: Option<usize> = None;
    for (index, line) in hunk.lines.iter().enumerate() {
        match (line.kind, start) {
            (PatchLineKind::Context, Some(begin)) => {
                runs.push(begin..index);
                start = None;
            }
            (PatchLineKind::Context, None) => {}
            (_, None) => start = Some(index),
            (_, Some(_)) => {}
        }
    }
    if let Some(begin) = start {
        runs.push(begin..hunk.lines.len());
    }
    let mut slices = Vec::with_capacity(runs.len());
    for position in 0..runs.len() {
        let begin = position
            .checked_sub(1)
            .and_then(|previous| runs.get(previous))
            .map_or(0, |previous| previous.end);
        let end = runs
            .get(position + 1)
            .map_or(hunk.lines.len(), |next| next.start);
        slices.push(begin..end);
    }
    slices
}

/// One chosen fragment: a hunk index within a file and a slice of that hunk.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SelectedSlice {
    pub(crate) hunk: usize,
    pub(crate) lines: Range<usize>,
}

/// Re-emit a file section carrying only the selected slices, with hunk headers
/// recomputed so the result is a self-consistent patch against the old tree.
pub(crate) fn render_selected_slices(
    file: &PatchFile,
    selected: &[SelectedSlice],
) -> Result<String> {
    if selected.is_empty() {
        return Ok(String::new());
    }
    let mut out = String::new();
    for line in &file.header {
        out.push_str(line);
        out.push('\n');
    }
    let mut delta = 0_isize;
    for slice in selected {
        let Some(hunk) = file.hunks.get(slice.hunk) else {
            bail!("selected hunk {} is out of range", slice.hunk);
        };
        let Some(lines) = hunk.lines.get(slice.lines.clone()) else {
            bail!(
                "selected lines {:?} are out of range for hunk {}",
                slice.lines,
                slice.hunk
            );
        };
        let old_before = hunk
            .lines
            .get(..slice.lines.start)
            .unwrap_or_default()
            .iter()
            .filter(|line| line.kind != PatchLineKind::Addition)
            .count();
        let old_count = lines
            .iter()
            .filter(|line| line.kind != PatchLineKind::Addition)
            .count();
        let new_count = lines
            .iter()
            .filter(|line| line.kind != PatchLineKind::Deletion)
            .count();
        let old_start = hunk.old_start + old_before;
        let new_start = if new_count == 0 {
            offset_line(old_start.saturating_sub(1), delta)?
        } else if old_count == 0 {
            offset_line(old_start + 1, delta)?
        } else {
            offset_line(old_start, delta)?
        };
        out.push_str(&format!(
            "@@ -{old_start},{old_count} +{new_start},{new_count} @@{}\n",
            hunk.section
        ));
        for line in lines {
            out.push_str(&line.raw);
            out.push('\n');
            if line.no_newline {
                out.push_str("\\ No newline at end of file\n");
            }
        }
        delta += isize::try_from(new_count)? - isize::try_from(old_count)?;
    }
    Ok(out)
}

fn offset_line(line: usize, delta: isize) -> Result<usize> {
    let shifted = isize::try_from(line)? + delta;
    usize::try_from(shifted)
        .map_err(|error| anyhow::anyhow!("hunk header line {line} shifted out of range: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TWO_HUNK_PATCH: &str = "diff --git a/src/lib.rs b/src/lib.rs\nindex 1111111..2222222 100644\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1,4 +1,4 @@\n fn head() {}\n-old production\n+new production\n line six\n line seven\n@@ -20,3 +20,5 @@\n mod tests {\n     fn existing() {}\n+\n+    fn added() {}\n }\n";

    fn only_file(patch: &str) -> Result<PatchFile> {
        let mut files = parse_unified_diff(patch)?;
        ensure!(files.len() == 1, "expected exactly one file section");
        files
            .pop()
            .ok_or_else(|| anyhow::anyhow!("expected one file section"))
    }

    #[test]
    fn parses_paths_headers_and_line_numbers() -> Result<()> {
        let file = only_file(TWO_HUNK_PATCH)?;
        assert_eq!(file.old_path, "src/lib.rs");
        assert_eq!(file.new_path, "src/lib.rs");
        assert_eq!(file.header.len(), 4);
        assert_eq!(file.hunks.len(), 2);
        let first = file
            .hunks
            .first()
            .ok_or_else(|| anyhow::anyhow!("missing hunk"))?;
        assert_eq!(first.lines.len(), 5);
        let deletion = first
            .lines
            .get(1)
            .ok_or_else(|| anyhow::anyhow!("missing deletion"))?;
        assert_eq!(deletion.kind, PatchLineKind::Deletion);
        assert_eq!(deletion.old_line, Some(2));
        assert_eq!(deletion.new_line, None);
        let addition = first
            .lines
            .get(2)
            .ok_or_else(|| anyhow::anyhow!("missing addition"))?;
        assert_eq!(addition.kind, PatchLineKind::Addition);
        assert_eq!(addition.old_line, None);
        assert_eq!(addition.new_line, Some(2));
        Ok(())
    }

    #[test]
    fn change_slices_carry_surrounding_context() -> Result<()> {
        let file = only_file(TWO_HUNK_PATCH)?;
        let first = file
            .hunks
            .first()
            .ok_or_else(|| anyhow::anyhow!("missing hunk"))?;
        assert_eq!(hunk_change_slices(first), vec![0..5]);
        let second = file
            .hunks
            .get(1)
            .ok_or_else(|| anyhow::anyhow!("missing hunk"))?;
        assert_eq!(hunk_change_slices(second), vec![0..5]);
        Ok(())
    }

    #[test]
    fn change_slices_split_at_context_between_runs() -> Result<()> {
        let patch = "diff --git a/a.txt b/a.txt\n--- a/a.txt\n+++ b/a.txt\n@@ -1,5 +1,5 @@\n one\n-two\n+TWO\n three\n-four\n+FOUR\n six\n";
        let file = only_file(patch)?;
        let hunk = file
            .hunks
            .first()
            .ok_or_else(|| anyhow::anyhow!("missing hunk"))?;
        assert_eq!(hunk_change_slices(hunk), vec![0..4, 3..7]);
        Ok(())
    }

    #[test]
    fn rendering_one_slice_recomputes_headers() -> Result<()> {
        let file = only_file(TWO_HUNK_PATCH)?;
        let rendered = render_selected_slices(
            &file,
            &[SelectedSlice {
                hunk: 1,
                lines: 0..5,
            }],
        )?;
        assert_eq!(
            rendered,
            "diff --git a/src/lib.rs b/src/lib.rs\nindex 1111111..2222222 100644\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -20,3 +20,5 @@\n mod tests {\n     fn existing() {}\n+\n+    fn added() {}\n }\n",
            "{rendered}"
        );
        Ok(())
    }

    #[test]
    fn rendering_accumulates_line_offsets_across_slices() -> Result<()> {
        let patch = "diff --git a/a.txt b/a.txt\n--- a/a.txt\n+++ b/a.txt\n@@ -1,3 +1,4 @@\n one\n+added\n two\n three\n@@ -10,2 +11,3 @@\n ten\n+also\n eleven\n";
        let file = only_file(patch)?;
        let rendered = render_selected_slices(
            &file,
            &[
                SelectedSlice {
                    hunk: 0,
                    lines: 0..4,
                },
                SelectedSlice {
                    hunk: 1,
                    lines: 0..3,
                },
            ],
        )?;
        assert!(rendered.contains("@@ -1,3 +1,4 @@\n"), "{rendered}");
        assert!(rendered.contains("@@ -10,2 +11,3 @@\n"), "{rendered}");
        Ok(())
    }

    #[test]
    fn rendering_is_byte_stable_for_repeated_calls() -> Result<()> {
        let file = only_file(TWO_HUNK_PATCH)?;
        let selected = [SelectedSlice {
            hunk: 1,
            lines: 0..5,
        }];
        assert_eq!(
            render_selected_slices(&file, &selected)?,
            render_selected_slices(&file, &selected)?
        );
        Ok(())
    }

    #[test]
    fn parsing_rejects_unexpected_body_lines() {
        let patch = "diff --git a/a.txt b/a.txt\n--- a/a.txt\n+++ b/a.txt\n@@ -1,1 +1,1 @@\n-one\n+two\nnot a diff line\n";
        assert!(parse_unified_diff(patch).is_err());
    }

    #[test]
    fn parsing_rejects_quoted_paths() {
        let patch = "diff --git \"a/odd name.txt\" \"b/odd name.txt\"\n";
        assert!(parse_unified_diff(patch).is_err());
    }

    #[test]
    fn parsing_keeps_binary_sections_hunkless() -> Result<()> {
        let patch = "diff --git a/blob.bin b/blob.bin\nindex 1111111..2222222 100644\nBinary files a/blob.bin and b/blob.bin differ\n";
        let file = only_file(patch)?;
        assert!(file.hunks.is_empty());
        assert_eq!(file.raw, patch);
        Ok(())
    }

    #[test]
    fn parsing_tracks_rename_paths() -> Result<()> {
        let patch = "diff --git a/old.rs b/new.rs\nsimilarity index 90%\nrename from old.rs\nrename to new.rs\n--- a/old.rs\n+++ b/new.rs\n@@ -1,1 +1,1 @@\n-one\n+two\n";
        let file = only_file(patch)?;
        assert_eq!(file.old_path, "old.rs");
        assert_eq!(file.new_path, "new.rs");
        assert!(file.header_has("rename from "));
        Ok(())
    }

    #[test]
    fn parsing_attaches_no_newline_marker_to_its_line() -> Result<()> {
        let patch = "diff --git a/a.txt b/a.txt\n--- a/a.txt\n+++ b/a.txt\n@@ -1,1 +1,1 @@\n-one\n\\ No newline at end of file\n+two\n";
        let file = only_file(patch)?;
        let hunk = file
            .hunks
            .first()
            .ok_or_else(|| anyhow::anyhow!("missing hunk"))?;
        let deletion = hunk
            .lines
            .first()
            .ok_or_else(|| anyhow::anyhow!("missing deletion"))?;
        assert!(deletion.no_newline);
        Ok(())
    }
}
