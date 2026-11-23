// SPDX-FileCopyrightText: © 2026 LEDGER SAS
// SPDX-License-Identifier: MIT

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::render::COVERAGE_METRIC_NAMES;

/// A single coverage metric (engine-agnostic).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoverageMetricEntry {
    pub metric_name: String,
    pub covered: i32,
    pub total: i32,
}

/// Per-source-file coverage, as reported by `llvm-cov export`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoverageFileEntry {
    pub path: String,
    pub lines_covered: i32,
    pub lines_total: i32,
    pub functions_covered: i32,
    pub functions_total: i32,
    pub regions_covered: i32,
    pub regions_total: i32,
    pub branches_covered: i32,
    pub branches_total: i32,
}

/// Aggregated coverage summary ready to be sent to the backend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoverageSummary {
    pub metrics: Vec<CoverageMetricEntry>,
    pub files: Vec<CoverageFileEntry>,
}

/// Upper bound on per-file rows shipped in a snapshot, so a huge project cannot
/// blow up the payload or the database.
///
/// Must stay in sync with the limit the backend enforces on `/coverage_snapshots`.
pub const MAX_COVERAGE_FILES: usize = 5000;

/// Read `{covered, count}` out of one section of an `llvm-cov` summary.
fn section_counts(summary: &Value, name: &str) -> (i32, i32) {
    let Some(section) = summary.get(name) else {
        return (0, 0);
    };
    (count_of(section, "covered"), count_of(section, "count"))
}

/// Read one counter of an `llvm-cov` summary section.
///
/// The database columns are 32-bit, so an implausibly large count is clamped
/// rather than wrapped around into a negative number.
fn count_of(section: &Value, field: &str) -> i32 {
    section
        .get(field)
        .and_then(Value::as_i64)
        .unwrap_or(0)
        .clamp(0, i32::MAX as i64) as i32
}

/// Extract the per-file breakdown that `llvm-cov export` already returns
/// alongside the totals.
///
/// Files without any coverage mapping are skipped, and the result is capped to
/// [`MAX_COVERAGE_FILES`], keeping the files with the most uncovered lines.
pub fn extract_file_metrics(json: &Value) -> Vec<CoverageFileEntry> {
    let Some(data) = json.get("data").and_then(|d| d.as_array()) else {
        return Vec::new();
    };

    let mut entries: Vec<CoverageFileEntry> = data
        .iter()
        .filter_map(|entry| entry.get("files").and_then(|f| f.as_array()))
        .flatten()
        .filter_map(|file| {
            let path = file.get("filename")?.as_str()?.to_string();
            let summary = file.get("summary")?;
            let (lines_covered, lines_total) = section_counts(summary, "lines");
            let (functions_covered, functions_total) = section_counts(summary, "functions");
            let (regions_covered, regions_total) = section_counts(summary, "regions");
            let (branches_covered, branches_total) = section_counts(summary, "branches");
            if lines_total == 0 && functions_total == 0 && regions_total == 0 {
                return None;
            }
            Some(CoverageFileEntry {
                path,
                lines_covered,
                lines_total,
                functions_covered,
                functions_total,
                regions_covered,
                regions_total,
                branches_covered,
                branches_total,
            })
        })
        .collect();

    if entries.len() > MAX_COVERAGE_FILES {
        entries.sort_by_key(|e| std::cmp::Reverse(e.lines_total - e.lines_covered));
        entries.truncate(MAX_COVERAGE_FILES);
    }
    entries
}

/// Extract structured coverage metrics from `llvm-cov export --summary-only` JSON.
///
/// The LLVM JSON format has `data[0].totals.{lines,functions,regions,branches}` each
/// with `count` and `covered` fields. We map these to our universal metric model.
pub fn extract_summary_metrics(json: &Value) -> Option<CoverageSummary> {
    let totals = json.get("data")?.as_array()?.first()?.get("totals")?;

    let mut metrics = Vec::new();

    for metric_name in COVERAGE_METRIC_NAMES {
        let Some(section) = totals.get(metric_name) else {
            continue;
        };
        let (covered, total) = (count_of(section, "covered"), count_of(section, "count"));
        if total > 0 {
            metrics.push(CoverageMetricEntry {
                metric_name: metric_name.to_string(),
                covered,
                total,
            });
        }
    }

    if metrics.is_empty() {
        return None;
    }

    Some(CoverageSummary {
        metrics,
        files: extract_file_metrics(json),
    })
}
