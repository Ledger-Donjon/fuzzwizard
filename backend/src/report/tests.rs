// SPDX-FileCopyrightText: © 2026 LEDGER SAS
// SPDX-License-Identifier: MIT

//! Unit tests for the report model and its HTML rendering.

use chrono::NaiveDateTime;

use super::builder::{
    health_from_age, percent, tail, HEARTBEAT_HEALTHY_SECS, HEARTBEAT_STALE_SECS, HISTORY_POINTS,
};
use super::html::{
    format_duration, render_coverage_files, render_delta, render_file_rows, render_report_html,
    shorten_path, sparkline,
};
use super::types::*;

fn ts(s: &str) -> NaiveDateTime {
    NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").unwrap()
}

fn point(at: &str, covered: i32, total: i32) -> CoverageHistoryPoint {
    CoverageHistoryPoint {
        captured_at: ts(at),
        covered,
        total,
        percent: percent(covered.into(), total.into()),
    }
}

fn sample_report() -> ReportResponse {
    ReportResponse {
        generated_at: ts("2026-08-06 16:00:00"),
        totals: ReportTotals {
            schedulers: 1,
            fuzzers: 3,
            running: 2,
            crashed: 1,
            stopped: 0,
            crashes: 12,
            crashes_last_24h: 3,
            coverage: vec![GlobalCoverageMetric {
                metric_name: "lines".to_string(),
                covered: 450,
                total: 1000,
                percent: 45.0,
                schedulers: 1,
            }],
        },
        schedulers: vec![SchedulerReport {
            scheduler_name: "sched-<main>".to_string(),
            health: "healthy".to_string(),
            last_seen_at: Some(ts("2026-08-06 15:59:48")),
            seconds_since_last_update: Some(12),
            fuzzers: FuzzerCounts {
                total: 3,
                running: 2,
                crashed: 1,
                stopped: 0,
            },
            fuzzer_details: vec![
                FuzzerReport {
                    sha: "abcdef0123456789".to_string(),
                    name: "fuzz_parser".to_string(),
                    path: "/targets/fuzz_parser".to_string(),
                    status: "running".to_string(),
                    started_at: Some(ts("2026-08-06 12:59:48")),
                    uptime_seconds: Some(10_800),
                    last_seen_at: ts("2026-08-06 15:59:48"),
                    seconds_since_last_update: 12,
                },
                FuzzerReport {
                    sha: "0011223344556677".to_string(),
                    name: "fuzz_decoder".to_string(),
                    path: "/targets/fuzz_decoder".to_string(),
                    status: "crashed".to_string(),
                    started_at: Some(ts("2026-08-06 14:00:00")),
                    uptime_seconds: None,
                    last_seen_at: ts("2026-08-06 15:59:48"),
                    seconds_since_last_update: 12,
                },
            ],
            coverage: Some(CoverageReport {
                captured_at: ts("2026-08-06 15:30:00"),
                fuzzer_engine: "libfuzzer".to_string(),
                build_fingerprint: "abcdef1234567890fedcba".to_string(),
                num_fuzzers: 3,
                schema_version: 1,
                previous_captured_at: Some(ts("2026-08-05 15:30:00")),
                snapshots_total: 42,
                distinct_builds: 7,
                first_captured_at: Some(ts("2026-05-01 09:00:00")),
                history_truncated: false,
                metrics: vec![
                    CoverageMetricReport {
                        metric_name: "functions".to_string(),
                        covered: 40,
                        total: 50,
                        percent: 80.0,
                        previous_covered: Some(41),
                        previous_percent: Some(82.0),
                        delta_covered: Some(-1),
                        delta_percent: Some(-2.0),
                        peak_percent: 82.0,
                        peak_captured_at: Some(ts("2026-08-05 15:30:00")),
                        history: vec![
                            point("2026-08-04 15:30:00", 39, 50),
                            point("2026-08-05 15:30:00", 41, 50),
                            point("2026-08-06 15:30:00", 40, 50),
                        ],
                    },
                    CoverageMetricReport {
                        metric_name: "lines".to_string(),
                        covered: 450,
                        total: 1000,
                        percent: 45.0,
                        previous_covered: Some(430),
                        previous_percent: Some(43.0),
                        delta_covered: Some(20),
                        delta_percent: Some(2.0),
                        peak_percent: 45.0,
                        peak_captured_at: Some(ts("2026-08-06 15:30:00")),
                        history: vec![
                            point("2026-08-04 15:30:00", 420, 1000),
                            point("2026-08-05 15:30:00", 430, 1000),
                            point("2026-08-06 15:30:00", 450, 1000),
                        ],
                    },
                ],
                files_total: 3,
                files: vec![
                    file_report("/src/parser/decode.rs", 120, 600, Some(1.5), false),
                    file_report("/src/parser/table.rs", 200, 300, None, true),
                    file_report("/src/util.rs", 130, 100, Some(-0.5), false),
                ],
            }),
            crashes: CrashSummary {
                total: 12,
                last_24h: 3,
                last_crash_at: Some(ts("2026-08-06 15:10:00")),
            },
        }],
    }
}

fn file_report(
    path: &str,
    lines_covered: i32,
    lines_total: i32,
    delta: Option<f64>,
    is_new: bool,
) -> CoverageFileReport {
    CoverageFileReport {
        path: path.to_string(),
        lines_covered,
        lines_total,
        lines_percent: percent(lines_covered.into(), lines_total.into()),
        functions_covered: lines_covered / 10,
        functions_total: lines_total / 10,
        regions_covered: lines_covered,
        regions_total: lines_total,
        branches_covered: 0,
        branches_total: 0,
        uncovered_lines: lines_total - lines_covered,
        delta_lines_percent: delta,
        is_new,
    }
}

#[test]
fn heartbeat_accepts_a_missing_started_at() {
    let payload: FuzzerStatusHeartbeat = serde_json::from_str(
        r#"{"scheduler_name":"s","fuzzers":[{"sha":"ab","path":"/t/f","status":"stopped"}]}"#,
    )
    .unwrap();
    assert_eq!(payload.fuzzers[0].started_at, None);

    let payload: FuzzerStatusHeartbeat = serde_json::from_str(
        r#"{"scheduler_name":"s","fuzzers":[{"sha":"ab","path":"/t/f","status":"running","started_at":"2026-08-06T12:59:48"}]}"#,
    )
    .unwrap();
    assert_eq!(
        payload.fuzzers[0].started_at,
        Some(ts("2026-08-06 12:59:48"))
    );
}

#[test]
fn long_paths_keep_their_tail() {
    assert_eq!(shorten_path("/src/a.rs"), "/src/a.rs");
    let long = format!("/very/long/prefix{}/deep/target.rs", "/pad".repeat(30));
    let short = shorten_path(&long);
    assert!(short.len() <= 58);
    assert!(short.ends_with("/deep/target.rs"));
}

#[test]
fn shortening_never_splits_a_character() {
    // A path made of multi-byte characters would panic on a naive byte slice
    let long: String = "é".repeat(200);
    assert!(shorten_path(&long).chars().count() <= 58);
}

#[test]
fn shortening_a_short_multibyte_path_is_a_no_op() {
    // 30 characters but 60 bytes: the path fits in the character budget while
    // exceeding it in bytes, which used to underflow the character offset.
    let path: String = "é".repeat(30);
    assert_eq!(shorten_path(&path), path);
}

#[test]
fn the_file_table_ranks_and_folds() {
    let report = sample_report();
    let cov = report.schedulers[0].coverage.as_ref().unwrap();
    let html = render_coverage_files(cov);
    assert!(html.contains("3 fichiers instrumentés"));
    assert!(html.contains("decode.rs"));
    // Marked as absent from the previous snapshot rather than showing a delta
    assert!(html.contains("nouveau"));
    // Everything fits under the fold here
    assert!(!html.contains("<details"));
}

#[test]
fn the_file_table_is_skipped_without_detail() {
    let mut report = sample_report();
    let cov = report.schedulers[0].coverage.as_mut().unwrap();
    cov.files.clear();
    cov.files_total = 0;
    assert_eq!(render_coverage_files(cov), "");
}

#[test]
fn file_paths_are_escaped() {
    let files = vec![file_report("/src/<script>.rs", 1, 2, None, false)];
    let html = render_file_rows(&files);
    assert!(!html.contains("<script>"));
    assert!(html.contains("&lt;script&gt;"));
}

#[test]
fn percent_handles_empty_totals() {
    assert_eq!(percent(0, 0), 0.0);
    assert_eq!(percent(450, 1000), 45.0);
    assert_eq!(percent(1, 3), 33.33);
}

#[test]
fn health_follows_heartbeat_age() {
    assert_eq!(health_from_age(0), "healthy");
    assert_eq!(health_from_age(HEARTBEAT_HEALTHY_SECS), "healthy");
    assert_eq!(health_from_age(HEARTBEAT_HEALTHY_SECS + 1), "stale");
    assert_eq!(health_from_age(HEARTBEAT_STALE_SECS), "stale");
    assert_eq!(health_from_age(HEARTBEAT_STALE_SECS + 1), "down");
}

#[test]
fn durations_are_human_readable() {
    assert_eq!(format_duration(45), "45s");
    assert_eq!(format_duration(125), "2m 5s");
    assert_eq!(format_duration(10_800), "3h 0m");
    assert_eq!(format_duration(90_000), "1j 1h");
}

#[test]
fn scheduler_names_are_escaped_in_html() {
    let html = render_report_html(&sample_report());
    assert!(html.contains("sched-&lt;main&gt;"));
    assert!(!html.contains("sched-<main>"));
}

#[test]
fn html_reports_status_and_coverage() {
    let html = render_report_html(&sample_report());
    assert!(html.starts_with("<!DOCTYPE html>"));
    assert!(html.contains("pill pill-healthy"));
    assert!(html.contains("pill pill-running"));
    assert!(html.contains("pill pill-crashed"));
    assert!(html.contains("dernier update il y a 12s"));
    assert!(html.contains("width:45%"));
    assert!(html.contains("450 / 1000"));
    // A running fuzzer shows its uptime, a crashed one does not
    assert!(html.contains("3h 0m"));
}

#[test]
fn html_shows_the_coverage_trend() {
    let html = render_report_html(&sample_report());
    assert!(html.contains("delta delta-up"));
    assert!(html.contains("+2.00 pts"));
    assert!(html.contains("delta delta-down"));
    assert!(html.contains("-2.00 pts"));
    assert!(html.contains("<svg class=\"spark\""));
    assert!(html.contains("<b>42</b> snapshots"));
    assert!(html.contains("<b>7</b> builds distincts"));
    assert!(html.contains("pic 82.00% le 2026-08-05"));
}

#[test]
fn delta_badges_reflect_direction() {
    assert!(render_delta(Some(1.5)).contains("delta-up"));
    assert!(render_delta(Some(-1.5)).contains("delta-down"));
    assert!(render_delta(Some(0.0)).contains("delta-flat"));
    assert_eq!(render_delta(None), "");
}

#[test]
fn sparkline_needs_at_least_two_points() {
    assert_eq!(sparkline(&[], "#fff"), "<span></span>");
    assert_eq!(sparkline(&[42.0], "#fff"), "<span></span>");
    assert!(sparkline(&[1.0, 2.0], "#fff").contains("<polyline"));
}

#[test]
fn sparkline_survives_a_flat_series() {
    // A constant series must not divide by zero
    let svg = sparkline(&[50.0, 50.0, 50.0], "#fff");
    assert!(svg.contains("<polyline"));
    assert!(!svg.contains("NaN"));
}

#[test]
fn history_keeps_the_most_recent_points() {
    let points: Vec<CoverageHistoryPoint> = (1..=60)
        .map(|i| point(&format!("2026-08-06 10:{:02}:00", i % 60), i, 100))
        .collect();
    let kept = tail(points, HISTORY_POINTS);
    assert_eq!(kept.len(), HISTORY_POINTS);
    assert_eq!(kept.last().unwrap().covered, 60);
}

#[test]
fn global_coverage_card_is_hidden_for_a_single_scheduler() {
    assert!(!render_report_html(&sample_report()).contains("Coverage global"));
}

#[test]
fn empty_report_renders_a_placeholder() {
    let report = ReportResponse {
        generated_at: ts("2026-08-06 16:00:00"),
        totals: ReportTotals {
            schedulers: 0,
            fuzzers: 0,
            running: 0,
            crashed: 0,
            stopped: 0,
            crashes: 0,
            crashes_last_24h: 0,
            coverage: vec![],
        },
        schedulers: vec![],
    };
    let html = render_report_html(&report);
    assert!(html.contains("Aucun scheduler connu"));
}
