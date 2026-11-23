// SPDX-FileCopyrightText: © 2026 LEDGER SAS
// SPDX-License-Identifier: MIT

use std::collections::HashSet;
use std::path::Path;

use serde_json::Value;

/// First line of the coverage view rendered when `llvm-cov` cannot read the
/// sources and only region-level data can be shown.
///
/// The TUI keys off this marker to style the panel as a degraded view.
pub const COVERAGE_SOURCE_UNAVAILABLE_MARKER: &str = "SOURCE NOT AVAILABLE";

/// `kind` value identifying an ordinary code region in `llvm-cov export` output.
const REGION_KIND_CODE: u64 = 0;

/// Metrics `llvm-cov` reports in the totals of an export, in display order.
pub(super) const COVERAGE_METRIC_NAMES: [&str; 4] = ["lines", "functions", "regions", "branches"];

/// Extract `"filename":"…"` values from `llvm-cov export` JSON output.
pub(super) fn parse_filenames_from_export(json: &Value) -> Vec<String> {
    let mut result = HashSet::new();
    collect_filenames(json, &mut result);
    result.into_iter().collect()
}

fn collect_filenames(v: &Value, out: &mut HashSet<String>) {
    match v {
        Value::Object(map) => {
            if let Some(Value::String(s)) = map.get("filename") {
                if !s.is_empty() {
                    out.insert(s.clone());
                }
            }
            for val in map.values() {
                collect_filenames(val, out);
            }
        }
        Value::Array(arr) => {
            for val in arr {
                collect_filenames(val, out);
            }
        }
        _ => {}
    }
}

/// A code region from `llvm-cov export`, whose JSON encoding is the array
/// `[lineStart, colStart, lineEnd, colEnd, count, fileID, expandedFileID, kind]`.
pub(super) struct Region {
    pub line_start: u64,
    pub line_end: u64,
    pub count: u64,
}

/// Per-function coverage extracted from `llvm-cov export`.
pub(super) struct FunctionCoverage {
    pub name: String,
    pub count: u64,
    pub regions: Vec<Region>,
}

impl FunctionCoverage {
    pub(super) fn line_span(&self) -> Option<(u64, u64)> {
        let start = self.regions.iter().map(|r| r.line_start).min()?;
        let end = self.regions.iter().map(|r| r.line_end).max()?;
        Some((start, end))
    }

    pub(super) fn covered_regions(&self) -> usize {
        self.regions.iter().filter(|r| r.count > 0).count()
    }
}

/// Compare two source paths, falling back to a basename match so that
/// container/host path differences still line up.
pub(super) fn same_source_file(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    fn base(p: &str) -> Option<&str> {
        Path::new(p).file_name().and_then(|n| n.to_str())
    }
    match (base(a), base(b)) {
        (Some(x), Some(y)) => x == y,
        _ => false,
    }
}

fn parse_region(v: &Value) -> Option<Region> {
    let a = v.as_array()?;
    let num = |i: usize| a.get(i).and_then(Value::as_u64);
    if num(7).unwrap_or(REGION_KIND_CODE) != REGION_KIND_CODE {
        return None;
    }
    Some(Region {
        line_start: num(0)?,
        line_end: num(2)?,
        count: num(4)?,
    })
}

/// Collect per-function coverage from `llvm-cov export` JSON, restricted to
/// `file` when given, ordered by position in the source.
pub(super) fn parse_functions(json: &Value, file: Option<&str>) -> Vec<FunctionCoverage> {
    let mut out: Vec<FunctionCoverage> = Vec::new();
    let Some(data) = json.get("data").and_then(|d| d.as_array()) else {
        return out;
    };

    for entry in data {
        let Some(functions) = entry.get("functions").and_then(|f| f.as_array()) else {
            continue;
        };
        for f in functions {
            if let Some(target) = file {
                let belongs = f
                    .get("filenames")
                    .and_then(|n| n.as_array())
                    .is_some_and(|names| {
                        names
                            .iter()
                            .filter_map(Value::as_str)
                            .any(|n| same_source_file(n, target))
                    });
                if !belongs {
                    continue;
                }
            }
            let regions = f
                .get("regions")
                .and_then(|r| r.as_array())
                .map(|r| r.iter().filter_map(parse_region).collect::<Vec<_>>())
                .unwrap_or_default();
            if regions.is_empty() {
                continue;
            }
            out.push(FunctionCoverage {
                name: f
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("<unknown>")
                    .to_string(),
                count: f.get("count").and_then(Value::as_u64).unwrap_or(0),
                regions,
            });
        }
    }

    out.sort_by_key(|f| f.line_span().map(|(start, _)| start).unwrap_or(0));
    out
}

/// Merge overlapping or adjacent `(start, end)` line ranges.
pub(super) fn merge_ranges(mut ranges: Vec<(u64, u64)>) -> Vec<(u64, u64)> {
    ranges.sort_unstable();
    let mut merged: Vec<(u64, u64)> = Vec::new();
    for (start, end) in ranges {
        match merged.last_mut() {
            Some(last) if start <= last.1.saturating_add(1) => last.1 = last.1.max(end),
            _ => merged.push((start, end)),
        }
    }
    merged
}

fn format_range((start, end): (u64, u64)) -> String {
    if start == end {
        start.to_string()
    } else {
        format!("{start}-{end}")
    }
}

/// Shorten an over-long symbol name so the function table stays aligned.
fn truncate_name(name: &str, max: usize) -> String {
    if name.chars().count() <= max {
        return name.to_string();
    }
    let head: String = name.chars().take(max.saturating_sub(3)).collect();
    format!("{head}...")
}

/// Render the `Lines`/`Functions`/`Regions` totals of a file, if the export
/// carries a summary block for it.
fn render_file_summary(json: &Value, file: &str) -> Option<String> {
    let files = json
        .get("data")
        .and_then(|d| d.as_array())?
        .iter()
        .filter_map(|e| e.get("files").and_then(|f| f.as_array()))
        .flatten();
    let summary = files
        .filter(|f| {
            f.get("filename")
                .and_then(Value::as_str)
                .is_some_and(|n| same_source_file(n, file))
        })
        .find_map(|f| f.get("summary"))?;

    let mut out = String::new();
    for key in COVERAGE_METRIC_NAMES {
        let Some(stat) = summary.get(key) else {
            continue;
        };
        let covered = stat.get("covered").and_then(Value::as_u64);
        let count = stat.get("count").and_then(Value::as_u64);
        let percent = stat.get("percent").and_then(Value::as_f64);
        if let (Some(covered), Some(count), Some(percent)) = (covered, count, percent) {
            let label = format!("{}{}", key[..1].to_uppercase(), &key[1..]);
            out.push_str(&format!(
                "  {label:<10} {covered:>8} / {count:<8} ({percent:.2}%)\n"
            ));
        }
    }
    (!out.is_empty()).then_some(out)
}

/// Render the per-function table and the uncovered line ranges.
fn render_functions(functions: &[FunctionCoverage]) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "\nFUNCTIONS ({} total, {} executed)\n\n",
        functions.len(),
        functions.iter().filter(|f| f.count > 0).count()
    ));
    out.push_str(&format!(
        "  {:>12}  {:>13}  {:>9}  {}\n",
        "EXECUTIONS", "LINES", "REGIONS", "NAME"
    ));
    for f in functions {
        let lines = f.line_span().map(format_range).unwrap_or_default();
        let regions = format!("{}/{}", f.covered_regions(), f.regions.len());
        out.push_str(&format!(
            "  {:>12}  {:>13}  {:>9}  {}\n",
            f.count,
            lines,
            regions,
            truncate_name(&f.name, 80)
        ));
    }

    let uncovered = merge_ranges(
        functions
            .iter()
            .flat_map(|f| f.regions.iter())
            .filter(|r| r.count == 0)
            .map(|r| (r.line_start, r.line_end))
            .collect(),
    );
    if !uncovered.is_empty() {
        let plural = if uncovered.len() == 1 {
            "range"
        } else {
            "ranges"
        };
        out.push_str(&format!(
            "\nUNCOVERED LINES ({} {plural})\n\n",
            uncovered.len()
        ));
        let rendered: Vec<String> = uncovered.into_iter().map(format_range).collect();
        for chunk in rendered.chunks(8) {
            out.push_str(&format!("  {}\n", chunk.join(", ")));
        }
    }

    out
}

/// Build the view shown when `llvm-cov` cannot read the sources.
///
/// `llvm-cov report` and `llvm-cov export` only need the coverage mapping, so
/// region-level data remains available even with no source tree on disk; we
/// show that rather than an empty panel.
pub(super) fn render_missing_source_view(
    requested: Option<&Path>,
    export: Option<&Value>,
) -> String {
    let mut out = format!("{COVERAGE_SOURCE_UNAVAILABLE_MARKER}\n\n");

    let requested_str = requested.map(|p| p.to_string_lossy().into_owned());
    match &requested_str {
        Some(path) => out.push_str(&format!(
            "llvm-cov could not read the source file recorded in the binary:\n  {path}\n"
        )),
        None => out
            .push_str("llvm-cov could not read any of the source files recorded in the binary.\n"),
    }
    out.push_str("\nRegion-level coverage from the profile data is shown below.\n");

    let Some(json) = export else {
        out.push_str("\nNo region data available: llvm-cov export produced no usable output.\n");
        return out;
    };

    match &requested_str {
        Some(path) => out.push_str(&render_file_section(json, path, false)),
        None => {
            let files = export_file_names(json);
            if files.is_empty() {
                out.push_str("\nNo region data available in the coverage mapping.\n");
            }
            for file in files {
                out.push_str(&render_file_section(json, &file, true));
            }
        }
    }

    out
}

/// File names carried by an `llvm-cov export` payload.
fn export_file_names(json: &Value) -> Vec<String> {
    let Some(data) = json.get("data").and_then(|d| d.as_array()) else {
        return Vec::new();
    };
    let mut names: Vec<String> = data
        .iter()
        .filter_map(|e| e.get("files").and_then(|f| f.as_array()))
        .flatten()
        .filter_map(|f| f.get("filename").and_then(Value::as_str))
        .map(str::to_string)
        .collect();
    names.sort();
    names.dedup();
    names
}

/// Render the summary and function table of a single file.
///
/// `with_header` prefixes the file name, which the all-files view needs to keep
/// the per-file sections apart.
fn render_file_section(json: &Value, file: &str, with_header: bool) -> String {
    let mut out = String::new();
    if with_header {
        out.push_str(&format!("\n{}\n{file}\n", "=".repeat(72)));
    }
    if let Some(summary) = render_file_summary(json, file) {
        out.push_str("\nSUMMARY\n\n");
        out.push_str(&summary);
    }
    let functions = parse_functions(json, Some(file));
    if functions.is_empty() {
        out.push_str("\nNo function-level region data found for this file.\n");
    } else {
        out.push_str(&render_functions(&functions));
    }
    out
}
