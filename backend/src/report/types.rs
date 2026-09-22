// SPDX-FileCopyrightText: © 2026 LEDGER SAS
// SPDX-License-Identifier: MIT

use chrono::NaiveDateTime;
use diesel::prelude::*;
use serde::{Deserialize, Serialize};

use crate::schema::{coverage_metrics, coverage_snapshots, fuzzer_status};

// --- Fuzzer heartbeat types ---

pub(crate) const VALID_FUZZER_STATUSES: [&str; 3] = ["running", "crashed", "stopped"];
pub(crate) const MAX_FUZZERS_PER_HEARTBEAT: usize = 512;

#[derive(Deserialize, Serialize)]
pub struct FuzzerStatusHeartbeat {
    pub scheduler_name: String,
    pub fuzzers: Vec<FuzzerStatusInput>,
}

#[derive(Deserialize, Serialize, Clone)]
pub(crate) struct FuzzerStatusInput {
    pub sha: String,
    pub path: String,
    pub status: String,
    pub started_at: Option<NaiveDateTime>,
}

#[derive(Queryable, Selectable)]
#[diesel(table_name = fuzzer_status)]
pub(crate) struct FuzzerStatusRow {
    pub scheduler_name: String,
    pub fuzzer_sha: String,
    pub fuzzer_path: String,
    pub status: String,
    pub started_at: Option<NaiveDateTime>,
    pub last_seen_at: NaiveDateTime,
}

// --- Report model ---

#[derive(Serialize)]
pub struct ReportResponse {
    pub generated_at: NaiveDateTime,
    pub totals: ReportTotals,
    pub schedulers: Vec<SchedulerReport>,
}

#[derive(Serialize)]
pub(crate) struct ReportTotals {
    pub schedulers: usize,
    pub fuzzers: usize,
    pub running: usize,
    pub crashed: usize,
    pub stopped: usize,
    pub crashes: i64,
    pub crashes_last_24h: i64,
    pub coverage: Vec<GlobalCoverageMetric>,
}

#[derive(Serialize)]
pub(crate) struct GlobalCoverageMetric {
    pub metric_name: String,
    pub covered: i64,
    pub total: i64,
    pub percent: f64,
    pub schedulers: usize,
}

#[derive(Serialize)]
pub(crate) struct SchedulerReport {
    pub scheduler_name: String,
    pub health: String,
    pub last_seen_at: Option<NaiveDateTime>,
    pub seconds_since_last_update: Option<i64>,
    pub fuzzers: FuzzerCounts,
    pub fuzzer_details: Vec<FuzzerReport>,
    pub coverage: Option<CoverageReport>,
    pub crashes: CrashSummary,
}

#[derive(Serialize, Default)]
pub(crate) struct FuzzerCounts {
    pub total: usize,
    pub running: usize,
    pub crashed: usize,
    pub stopped: usize,
}

#[derive(Serialize)]
pub(crate) struct FuzzerReport {
    pub sha: String,
    pub name: String,
    pub path: String,
    pub status: String,
    pub started_at: Option<NaiveDateTime>,
    pub uptime_seconds: Option<i64>,
    pub last_seen_at: NaiveDateTime,
    pub seconds_since_last_update: i64,
}

#[derive(Serialize)]
pub(crate) struct CoverageReport {
    pub captured_at: NaiveDateTime,
    pub fuzzer_engine: String,
    pub build_fingerprint: String,
    pub num_fuzzers: i32,
    pub schema_version: i32,
    pub previous_captured_at: Option<NaiveDateTime>,
    pub snapshots_total: i64,
    pub distinct_builds: i64,
    pub first_captured_at: Option<NaiveDateTime>,
    pub history_truncated: bool,
    pub metrics: Vec<CoverageMetricReport>,
    pub files_total: usize,
    pub files: Vec<CoverageFileReport>,
}

#[derive(Serialize)]
pub(crate) struct CoverageFileReport {
    pub path: String,
    pub lines_covered: i32,
    pub lines_total: i32,
    pub lines_percent: f64,
    pub functions_covered: i32,
    pub functions_total: i32,
    pub regions_covered: i32,
    pub regions_total: i32,
    pub branches_covered: i32,
    pub branches_total: i32,
    pub uncovered_lines: i32,
    pub delta_lines_percent: Option<f64>,
    pub is_new: bool,
}

#[derive(Serialize)]
pub(crate) struct CoverageMetricReport {
    pub metric_name: String,
    pub covered: i32,
    pub total: i32,
    pub percent: f64,
    pub previous_covered: Option<i32>,
    pub previous_percent: Option<f64>,
    pub delta_covered: Option<i32>,
    pub delta_percent: Option<f64>,
    pub peak_percent: f64,
    pub peak_captured_at: Option<NaiveDateTime>,
    pub history: Vec<CoverageHistoryPoint>,
}

#[derive(Serialize, Clone)]
pub(crate) struct CoverageHistoryPoint {
    pub captured_at: NaiveDateTime,
    pub covered: i32,
    pub total: i32,
    pub percent: f64,
}

#[derive(Serialize)]
pub(crate) struct CrashSummary {
    pub total: i64,
    pub last_24h: i64,
    pub last_crash_at: Option<NaiveDateTime>,
}

// --- DB query types ---

#[derive(Queryable, Selectable)]
#[diesel(table_name = coverage_snapshots)]
pub(crate) struct SnapshotRow {
    pub id: i32,
    pub fuzzer_engine: String,
    pub build_fingerprint: String,
    pub num_fuzzers: i32,
    pub captured_at: NaiveDateTime,
    pub schema_version: i32,
}

#[derive(Queryable, Selectable)]
#[diesel(table_name = coverage_metrics)]
pub(crate) struct SnapshotMetric {
    pub snapshot_id: i32,
    pub metric_name: String,
    pub covered: i32,
    pub total: i32,
}

#[derive(Queryable)]
pub(crate) struct FileRow {
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
