// SPDX-FileCopyrightText: © 2026 LEDGER SAS
// SPDX-License-Identifier: MIT

use super::render::wrap_line;
use super::App;

#[test]
fn wrap_line_keeps_short_lines_intact() {
    assert_eq!(wrap_line("short", 20), vec!["short"]);
    assert_eq!(wrap_line("", 20), vec![""]);
}

#[test]
fn wrap_line_without_width_is_a_no_op() {
    assert_eq!(wrap_line("anything at all", 0), vec!["anything at all"]);
}

#[test]
fn wrap_line_breaks_on_whitespace() {
    assert_eq!(
        wrap_line("alpha beta gamma delta", 12),
        vec!["alpha beta ", "gamma delta"]
    );
}

/// A word longer than the view has no break point, so it is cut mid-word rather
/// than overflowing.
#[test]
fn wrap_line_splits_unbreakable_word() {
    assert_eq!(wrap_line("aaaaaaaaaa", 4), vec!["aaaa", "aaaa", "aa"]);
}

#[test]
fn wrap_line_preserves_leading_indentation() {
    let chunks = wrap_line("    if (x) { return; }", 10);
    assert!(
        chunks[0].starts_with("    "),
        "indentation carries the nesting of annotated source: {chunks:?}"
    );
    assert_eq!(chunks.concat(), "    if (x) { return; }");
}

/// Chunks must be cut on character boundaries: slicing a multi-byte character
/// in half would panic.
#[test]
fn wrap_line_splits_multibyte_text() {
    let line = "éléments accentués répartis";
    let chunks = wrap_line(line, 8);
    assert_eq!(chunks.concat(), line);
    for chunk in chunks {
        assert!(chunk.chars().count() <= 8, "chunk too wide: {chunk:?}");
    }
}

#[test]
fn parse_coverage_summary_extracts_files_and_total() {
    let report = "\
Coverage Analysis for: 0xdeadbeef

Filename    Regions  Missed  Cover  Lines  Cover
src/a.c     100      10      90.00% 50     92.00%
src/b.c     20       20      0.00%  10     0.00%
TOTAL       120      30      75.00% 60     76.66%
";

    let (rows, paths, file_col) = App::parse_coverage_summary(report);

    assert_eq!(file_col, Some(0));
    assert_eq!(rows.len(), 4, "header, two files and the total: {rows:?}");
    assert_eq!(rows[0][0], "Filename");
    assert_eq!(
        paths,
        vec![
            None,
            Some("src/a.c".to_string()),
            Some("src/b.c".to_string()),
            None,
        ],
        "only real files are selectable, not the header or the TOTAL row"
    );
}

#[test]
fn parse_coverage_summary_of_error_text_is_empty() {
    let (rows, paths, file_col) = App::parse_coverage_summary("Error: Scheduler not found");
    assert!(rows.is_empty());
    assert!(paths.is_empty());
    assert_eq!(file_col, None);
}
