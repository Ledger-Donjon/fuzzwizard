// SPDX-FileCopyrightText: © 2026 LEDGER SAS
// SPDX-License-Identifier: MIT

//! Report assembly: joins heartbeats, coverage snapshots, and crashes into
//! [`ReportResponse`].

use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::Html,
    Json,
};
use chrono::NaiveDateTime;
use diesel::dsl::{count, min};
use diesel::prelude::*;
use log::error;

use crate::schema::{
    coverage_files, coverage_metrics, coverage_paths, coverage_snapshots, crashes, fuzzer_status,
};
use crate::DbPool;

use super::db_now;
use super::html::render_report_html;
use super::types::*;

pub(super) const HEARTBEAT_HEALTHY_SECS: i64 = 180;
pub(super) const HEARTBEAT_STALE_SECS: i64 = 900;
pub(super) const MAX_SNAPSHOTS_SCANNED: i64 = 500;
pub(super) const HISTORY_POINTS: usize = 40;
pub(super) const REPORTED_FILES: usize = 100;

pub(super) fn health_from_age(seconds: i64) -> &'static str {
    if seconds <= HEARTBEAT_HEALTHY_SECS {
        "healthy"
    } else if seconds <= HEARTBEAT_STALE_SECS {
        "stale"
    } else {
        "down"
    }
}

pub(crate) fn percent(covered: i64, total: i64) -> f64 {
    if total <= 0 {
        return 0.0;
    }
    let raw = covered as f64 * 100.0 / total as f64;
    (raw * 100.0).round() / 100.0
}

pub(super) fn load_snapshot_files(
    conn: &mut PgConnection,
    snapshot_id: i32,
) -> Result<Vec<FileRow>, diesel::result::Error> {
    coverage_files::table
        .inner_join(coverage_paths::table)
        .filter(coverage_files::snapshot_id.eq(snapshot_id))
        .select((
            coverage_paths::path,
            coverage_files::lines_covered,
            coverage_files::lines_total,
            coverage_files::functions_covered,
            coverage_files::functions_total,
            coverage_files::regions_covered,
            coverage_files::regions_total,
            coverage_files::branches_covered,
            coverage_files::branches_total,
        ))
        .load::<FileRow>(conn)
}

pub(super) fn tail(mut points: Vec<CoverageHistoryPoint>, keep: usize) -> Vec<CoverageHistoryPoint> {
    if points.len() > keep {
        points.drain(..points.len() - keep);
    }
    points
}

fn build_coverage(
    conn: &mut PgConnection,
    scheduler: &str,
) -> Result<Option<CoverageReport>, diesel::result::Error> {
    let snapshots: Vec<SnapshotRow> = coverage_snapshots::table
        .filter(coverage_snapshots::scheduler_name.eq(scheduler))
        .order(coverage_snapshots::captured_at.desc())
        .limit(MAX_SNAPSHOTS_SCANNED)
        .select(SnapshotRow::as_select())
        .load(conn)?;

    let Some(latest) = snapshots.first() else {
        return Ok(None);
    };

    let ids: Vec<i32> = snapshots.iter().map(|s| s.id).collect();
    let mut by_snapshot: HashMap<i32, Vec<SnapshotMetric>> = HashMap::new();
    for m in coverage_metrics::table
        .filter(coverage_metrics::snapshot_id.eq_any(&ids))
        .select(SnapshotMetric::as_select())
        .load::<SnapshotMetric>(conn)?
    {
        by_snapshot.entry(m.snapshot_id).or_default().push(m);
    }

    let snapshots_total = coverage_snapshots::table
        .filter(coverage_snapshots::scheduler_name.eq(scheduler))
        .count()
        .get_result::<i64>(conn)?;
    let distinct_builds = coverage_snapshots::table
        .filter(coverage_snapshots::scheduler_name.eq(scheduler))
        .select(count(coverage_snapshots::build_fingerprint).aggregate_distinct())
        .first::<i64>(conn)?;
    let first_captured_at = coverage_snapshots::table
        .filter(coverage_snapshots::scheduler_name.eq(scheduler))
        .select(min(coverage_snapshots::captured_at))
        .first::<Option<NaiveDateTime>>(conn)?;

    let mut series: HashMap<String, Vec<CoverageHistoryPoint>> = HashMap::new();
    for snapshot in snapshots.iter().rev() {
        for m in by_snapshot.get(&snapshot.id).into_iter().flatten() {
            series
                .entry(m.metric_name.clone())
                .or_default()
                .push(CoverageHistoryPoint {
                    captured_at: snapshot.captured_at,
                    covered: m.covered,
                    total: m.total,
                    percent: percent(m.covered.into(), m.total.into()),
                });
        }
    }

    let previous = snapshots.get(1);
    let previous_metrics: HashMap<&str, &SnapshotMetric> = previous
        .and_then(|p| by_snapshot.get(&p.id))
        .into_iter()
        .flatten()
        .map(|m| (m.metric_name.as_str(), m))
        .collect();

    let mut metrics: Vec<CoverageMetricReport> = by_snapshot
        .get(&latest.id)
        .into_iter()
        .flatten()
        .map(|m| {
            let current = percent(m.covered.into(), m.total.into());
            let prev = previous_metrics.get(m.metric_name.as_str());
            let previous_percent = prev.map(|p| percent(p.covered.into(), p.total.into()));
            let history = series.remove(&m.metric_name).unwrap_or_default();
            let peak = history
                .iter()
                .max_by(|a, b| a.percent.total_cmp(&b.percent));
            CoverageMetricReport {
                metric_name: m.metric_name.clone(),
                covered: m.covered,
                total: m.total,
                percent: current,
                previous_covered: prev.map(|p| p.covered),
                previous_percent,
                delta_covered: prev.map(|p| m.covered - p.covered),
                delta_percent: previous_percent.map(|p| ((current - p) * 100.0).round() / 100.0),
                peak_percent: peak.map(|p| p.percent).unwrap_or(current),
                peak_captured_at: peak.map(|p| p.captured_at),
                history: tail(history, HISTORY_POINTS),
            }
        })
        .collect();
    metrics.sort_by(|a, b| a.metric_name.cmp(&b.metric_name));

    let latest_files = load_snapshot_files(conn, latest.id)?;
    let previous_detail = coverage_files::table
        .inner_join(coverage_snapshots::table)
        .filter(coverage_snapshots::scheduler_name.eq(scheduler))
        .filter(coverage_snapshots::id.ne(latest.id))
        .filter(coverage_snapshots::captured_at.le(latest.captured_at))
        .order(coverage_snapshots::captured_at.desc())
        .select(coverage_files::snapshot_id)
        .first::<i32>(conn)
        .optional()?;

    let previous_lines: HashMap<String, f64> = match previous_detail {
        Some(id) => load_snapshot_files(conn, id)?
            .into_iter()
            .map(|f| {
                (
                    f.path,
                    percent(f.lines_covered.into(), f.lines_total.into()),
                )
            })
            .collect(),
        None => HashMap::new(),
    };
    let compare_files = !previous_lines.is_empty();

    let files_total = latest_files.len();
    let mut files: Vec<CoverageFileReport> = latest_files
        .into_iter()
        .map(|f| {
            let lines_percent = percent(f.lines_covered.into(), f.lines_total.into());
            let before = previous_lines.get(&f.path);
            CoverageFileReport {
                lines_percent,
                uncovered_lines: f.lines_total - f.lines_covered,
                delta_lines_percent: before.map(|p| ((lines_percent - p) * 100.0).round() / 100.0),
                is_new: compare_files && before.is_none(),
                path: f.path,
                lines_covered: f.lines_covered,
                lines_total: f.lines_total,
                functions_covered: f.functions_covered,
                functions_total: f.functions_total,
                regions_covered: f.regions_covered,
                regions_total: f.regions_total,
                branches_covered: f.branches_covered,
                branches_total: f.branches_total,
            }
        })
        .collect();
    files.sort_by(|a, b| {
        b.uncovered_lines
            .cmp(&a.uncovered_lines)
            .then_with(|| a.path.cmp(&b.path))
    });
    files.truncate(REPORTED_FILES);

    Ok(Some(CoverageReport {
        captured_at: latest.captured_at,
        fuzzer_engine: latest.fuzzer_engine.clone(),
        build_fingerprint: latest.build_fingerprint.clone(),
        num_fuzzers: latest.num_fuzzers,
        schema_version: latest.schema_version,
        previous_captured_at: previous.map(|p| p.captured_at),
        snapshots_total,
        distinct_builds,
        first_captured_at,
        history_truncated: snapshots_total > snapshots.len() as i64,
        metrics,
        files_total,
        files,
    }))
}

pub(super) fn build_report(
    conn: &mut PgConnection,
    scheduler_filter: Option<&str>,
) -> Result<ReportResponse, diesel::result::Error> {
    let generated_at = db_now(conn);
    let day_ago = generated_at - chrono::Duration::hours(24);

    let mut names: BTreeSet<String> = BTreeSet::new();
    names.extend(
        fuzzer_status::table
            .select(fuzzer_status::scheduler_name)
            .distinct()
            .load::<String>(conn)?,
    );
    names.extend(
        coverage_snapshots::table
            .select(coverage_snapshots::scheduler_name)
            .distinct()
            .load::<String>(conn)?,
    );
    names.extend(
        crashes::table
            .select(crashes::scheduler_name)
            .distinct()
            .load::<String>(conn)?,
    );
    if let Some(filter) = scheduler_filter {
        names.retain(|n| n == filter);
    }

    let mut status_rows: HashMap<String, Vec<FuzzerStatusRow>> = HashMap::new();
    for row in fuzzer_status::table
        .select(FuzzerStatusRow::as_select())
        .order((
            fuzzer_status::scheduler_name.asc(),
            fuzzer_status::fuzzer_path.asc(),
        ))
        .load::<FuzzerStatusRow>(conn)?
    {
        status_rows
            .entry(row.scheduler_name.clone())
            .or_default()
            .push(row);
    }

    let mut totals = ReportTotals {
        schedulers: 0,
        fuzzers: 0,
        running: 0,
        crashed: 0,
        stopped: 0,
        crashes: 0,
        crashes_last_24h: 0,
        coverage: Vec::new(),
    };
    let mut schedulers = Vec::with_capacity(names.len());
    let mut global_coverage: HashMap<String, (i64, i64, usize)> = HashMap::new();

    for name in names {
        let rows = status_rows.remove(&name).unwrap_or_default();

        let mut counts = FuzzerCounts::default();
        let mut fuzzer_details = Vec::with_capacity(rows.len());
        let mut last_seen_at: Option<NaiveDateTime> = None;

        for row in rows {
            counts.total += 1;
            match row.status.as_str() {
                "running" => counts.running += 1,
                "crashed" => counts.crashed += 1,
                _ => counts.stopped += 1,
            }
            last_seen_at = Some(match last_seen_at {
                Some(current) if current >= row.last_seen_at => current,
                _ => row.last_seen_at,
            });

            let display_name = Path::new(&row.fuzzer_path)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| row.fuzzer_path.clone());
            let uptime_seconds = match (row.status.as_str(), row.started_at) {
                ("running", Some(started)) => {
                    Some((row.last_seen_at - started).num_seconds().max(0))
                }
                _ => None,
            };

            fuzzer_details.push(FuzzerReport {
                sha: row.fuzzer_sha,
                name: display_name,
                path: row.fuzzer_path,
                status: row.status,
                started_at: row.started_at,
                uptime_seconds,
                seconds_since_last_update: (generated_at - row.last_seen_at).num_seconds().max(0),
                last_seen_at: row.last_seen_at,
            });
        }

        let seconds_since_last_update =
            last_seen_at.map(|seen| (generated_at - seen).num_seconds().max(0));
        let health = seconds_since_last_update
            .map(health_from_age)
            .unwrap_or("unknown")
            .to_string();

        let coverage = build_coverage(conn, &name)?;
        if let Some(cov) = &coverage {
            for m in &cov.metrics {
                let entry = global_coverage
                    .entry(m.metric_name.clone())
                    .or_insert((0, 0, 0));
                entry.0 += i64::from(m.covered);
                entry.1 += i64::from(m.total);
                entry.2 += 1;
            }
        }

        let crash_total = crashes::table
            .filter(crashes::scheduler_name.eq(&name))
            .count()
            .get_result::<i64>(conn)?;
        let crash_last_24h = crashes::table
            .filter(crashes::scheduler_name.eq(&name))
            .filter(crashes::created_at.ge(day_ago))
            .count()
            .get_result::<i64>(conn)?;
        let last_crash_at = crashes::table
            .filter(crashes::scheduler_name.eq(&name))
            .order(crashes::created_at.desc())
            .select(crashes::created_at)
            .first::<NaiveDateTime>(conn)
            .optional()?;

        totals.schedulers += 1;
        totals.fuzzers += counts.total;
        totals.running += counts.running;
        totals.crashed += counts.crashed;
        totals.stopped += counts.stopped;
        totals.crashes += crash_total;
        totals.crashes_last_24h += crash_last_24h;

        schedulers.push(SchedulerReport {
            scheduler_name: name,
            health,
            last_seen_at,
            seconds_since_last_update,
            fuzzers: counts,
            fuzzer_details,
            coverage,
            crashes: CrashSummary {
                total: crash_total,
                last_24h: crash_last_24h,
                last_crash_at,
            },
        });
    }

    totals.coverage = {
        let mut aggregated: Vec<GlobalCoverageMetric> = global_coverage
            .into_iter()
            .map(
                |(metric_name, (covered, total, schedulers))| GlobalCoverageMetric {
                    metric_name,
                    covered,
                    total,
                    percent: percent(covered, total),
                    schedulers,
                },
            )
            .collect();
        aggregated.sort_by(|a, b| a.metric_name.cmp(&b.metric_name));
        aggregated
    };

    Ok(ReportResponse {
        generated_at,
        totals,
        schedulers,
    })
}

pub async fn get_report(
    State(pool): State<DbPool>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<ReportResponse>, StatusCode> {
    let mut conn = pool.get().map_err(|e| {
        error!("Failed to get DB connection: {e}");
        StatusCode::SERVICE_UNAVAILABLE
    })?;

    let report =
        build_report(&mut conn, params.get("scheduler").map(String::as_str)).map_err(|e| {
            error!("Failed to build report: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(Json(report))
}

pub async fn get_report_html(
    State(pool): State<DbPool>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Html<String>, StatusCode> {
    let mut conn = pool.get().map_err(|e| {
        error!("Failed to get DB connection: {e}");
        StatusCode::SERVICE_UNAVAILABLE
    })?;

    let report =
        build_report(&mut conn, params.get("scheduler").map(String::as_str)).map_err(|e| {
            error!("Failed to build report: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(Html(render_report_html(&report)))
}
