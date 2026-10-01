// SPDX-FileCopyrightText: © 2026 LEDGER SAS
// SPDX-License-Identifier: MIT

use std::collections::HashMap;

use axum::{
    extract::{Query, State},
    http::StatusCode,
    Json,
};
use chrono::NaiveDateTime;
use diesel::prelude::*;
use log::{error, info};
use serde::{Deserialize, Serialize};

use crate::schema::{coverage_files, coverage_metrics, coverage_paths, coverage_snapshots};
use crate::DbPool;

/// Upper bound on per-file rows accepted in one snapshot.
const MAX_COVERAGE_FILES: usize = 5000;

/// Minimum spacing between two snapshots of the same build.
const MIN_RESNAPSHOT_SECS: i64 = 300;

/// Snapshots returned when the caller does not ask for a specific page size.
const DEFAULT_SNAPSHOT_LIMIT: i64 = 100;

/// Hard cap on a snapshot listing.
const MAX_SNAPSHOT_LIMIT: i64 = 1000;

#[derive(Deserialize, Serialize)]
pub(crate) struct CreateCoverageSnapshot {
    scheduler_name: String,
    fuzzer_engine: String,
    build_fingerprint: String,
    num_fuzzers: i32,
    metrics: Vec<CoverageMetricInput>,
    #[serde(default)]
    files: Vec<CoverageFileInput>,
}

#[derive(Deserialize, Serialize, Clone)]
struct CoverageFileInput {
    path: String,
    lines_covered: i32,
    lines_total: i32,
    functions_covered: i32,
    functions_total: i32,
    regions_covered: i32,
    regions_total: i32,
    branches_covered: i32,
    branches_total: i32,
}

impl CoverageFileInput {
    fn pairs(&self) -> [(i32, i32); 4] {
        [
            (self.lines_covered, self.lines_total),
            (self.functions_covered, self.functions_total),
            (self.regions_covered, self.regions_total),
            (self.branches_covered, self.branches_total),
        ]
    }
}

#[derive(Deserialize, Serialize, Clone)]
struct CoverageMetricInput {
    metric_name: String,
    covered: i32,
    total: i32,
}

#[derive(Serialize, Queryable, Selectable)]
#[diesel(table_name = coverage_snapshots)]
struct CoverageSnapshot {
    id: i32,
    scheduler_name: String,
    fuzzer_engine: String,
    build_fingerprint: String,
    num_fuzzers: i32,
    captured_at: NaiveDateTime,
    schema_version: i32,
}

#[derive(Serialize, Queryable, Selectable)]
#[diesel(table_name = coverage_metrics)]
struct CoverageMetric {
    id: i32,
    snapshot_id: i32,
    metric_name: String,
    covered: i32,
    total: i32,
}

#[derive(Serialize)]
pub(crate) struct CoverageSnapshotWithMetrics {
    #[serde(flatten)]
    snapshot: CoverageSnapshot,
    metrics: Vec<CoverageMetric>,
}

pub(crate) async fn create_coverage_snapshot(
    State(pool): State<DbPool>,
    Json(payload): Json<CreateCoverageSnapshot>,
) -> Result<StatusCode, (StatusCode, String)> {
    if payload.scheduler_name.is_empty() || payload.scheduler_name.len() > 256 {
        return Err((
            StatusCode::BAD_REQUEST,
            "Invalid scheduler_name".to_string(),
        ));
    }
    if payload.fuzzer_engine.is_empty() || payload.fuzzer_engine.len() > 64 {
        return Err((StatusCode::BAD_REQUEST, "Invalid fuzzer_engine".to_string()));
    }
    if payload.build_fingerprint.is_empty() || payload.build_fingerprint.len() > 4096 {
        return Err((
            StatusCode::BAD_REQUEST,
            "Invalid build_fingerprint".to_string(),
        ));
    }
    if payload.metrics.len() > 20 {
        return Err((
            StatusCode::BAD_REQUEST,
            "Too many metrics (max 20)".to_string(),
        ));
    }
    for m in &payload.metrics {
        if m.metric_name.is_empty() || m.metric_name.len() > 64 {
            return Err((StatusCode::BAD_REQUEST, "Invalid metric_name".to_string()));
        }
        if m.total < 0 || m.covered < 0 || m.covered > m.total {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("Invalid metric values for '{}'", m.metric_name),
            ));
        }
    }
    if payload.files.len() > MAX_COVERAGE_FILES {
        return Err((
            StatusCode::BAD_REQUEST,
            format!("Too many files (max {MAX_COVERAGE_FILES})"),
        ));
    }
    for f in &payload.files {
        if f.path.is_empty() || f.path.len() > 4096 {
            return Err((StatusCode::BAD_REQUEST, "Invalid file path".to_string()));
        }
        if f.pairs()
            .iter()
            .any(|(covered, total)| *total < 0 || *covered < 0 || covered > total)
        {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("Invalid coverage values for '{}'", f.path),
            ));
        }
    }

    let mut conn = pool.get().map_err(|e| {
        error!("Failed to get DB connection: {e}");
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "Database unavailable".to_string(),
        )
    })?;

    let last_same_build = coverage_snapshots::table
        .filter(coverage_snapshots::scheduler_name.eq(&payload.scheduler_name))
        .filter(coverage_snapshots::build_fingerprint.eq(&payload.build_fingerprint))
        .order(coverage_snapshots::captured_at.desc())
        .select(coverage_snapshots::captured_at)
        .first::<NaiveDateTime>(&mut conn)
        .optional()
        .map_err(|e| {
            error!("Dedup query failed: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Database error".to_string(),
            )
        })?;

    if let Some(previous) = last_same_build {
        let age = crate::report::db_now(&mut conn)
            .signed_duration_since(previous)
            .num_seconds();
        if age < MIN_RESNAPSHOT_SECS {
            return Ok(StatusCode::OK);
        }
    }

    let snapshot_id = conn
        .transaction::<i32, diesel::result::Error, _>(|conn| {
            let snapshot_id: i32 = diesel::insert_into(coverage_snapshots::table)
                .values((
                    coverage_snapshots::scheduler_name.eq(&payload.scheduler_name),
                    coverage_snapshots::fuzzer_engine.eq(&payload.fuzzer_engine),
                    coverage_snapshots::build_fingerprint.eq(&payload.build_fingerprint),
                    coverage_snapshots::num_fuzzers.eq(payload.num_fuzzers),
                ))
                .returning(coverage_snapshots::id)
                .get_result(conn)?;

            for m in &payload.metrics {
                diesel::insert_into(coverage_metrics::table)
                    .values((
                        coverage_metrics::snapshot_id.eq(snapshot_id),
                        coverage_metrics::metric_name.eq(&m.metric_name),
                        coverage_metrics::covered.eq(m.covered),
                        coverage_metrics::total.eq(m.total),
                    ))
                    .execute(conn)?;
            }

            if !payload.files.is_empty() {
                let superseded = coverage_snapshots::table
                    .filter(coverage_snapshots::scheduler_name.eq(&payload.scheduler_name))
                    .filter(coverage_snapshots::build_fingerprint.eq(&payload.build_fingerprint))
                    .filter(coverage_snapshots::id.ne(snapshot_id))
                    .select(coverage_snapshots::id)
                    .load::<i32>(conn)?;
                diesel::delete(
                    coverage_files::table.filter(coverage_files::snapshot_id.eq_any(&superseded)),
                )
                .execute(conn)?;

                insert_coverage_files(conn, snapshot_id, &payload.files)?;
            }
            Ok(snapshot_id)
        })
        .map_err(|e| {
            error!("Failed to insert coverage snapshot: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Database error".to_string(),
            )
        })?;

    info!(
        "Coverage snapshot created id={snapshot_id} for scheduler={} ({} files)",
        payload.scheduler_name,
        payload.files.len()
    );
    Ok(StatusCode::CREATED)
}

fn insert_coverage_files(
    conn: &mut PgConnection,
    snapshot_id: i32,
    files: &[CoverageFileInput],
) -> Result<(), diesel::result::Error> {
    if files.is_empty() {
        return Ok(());
    }

    for chunk in files.chunks(1000) {
        let new_paths: Vec<_> = chunk
            .iter()
            .map(|f| coverage_paths::path.eq(&f.path))
            .collect();
        diesel::insert_into(coverage_paths::table)
            .values(new_paths)
            .on_conflict(coverage_paths::path)
            .do_nothing()
            .execute(conn)?;
    }

    let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
    let path_ids: HashMap<String, i32> = coverage_paths::table
        .filter(coverage_paths::path.eq_any(&paths))
        .select((coverage_paths::path, coverage_paths::id))
        .load::<(String, i32)>(conn)?
        .into_iter()
        .collect();

    for chunk in files.chunks(500) {
        let rows: Vec<_> = chunk
            .iter()
            .filter_map(|f| {
                let path_id = *path_ids.get(&f.path)?;
                Some((
                    coverage_files::snapshot_id.eq(snapshot_id),
                    coverage_files::path_id.eq(path_id),
                    coverage_files::lines_covered.eq(f.lines_covered),
                    coverage_files::lines_total.eq(f.lines_total),
                    coverage_files::functions_covered.eq(f.functions_covered),
                    coverage_files::functions_total.eq(f.functions_total),
                    coverage_files::regions_covered.eq(f.regions_covered),
                    coverage_files::regions_total.eq(f.regions_total),
                    coverage_files::branches_covered.eq(f.branches_covered),
                    coverage_files::branches_total.eq(f.branches_total),
                ))
            })
            .collect();
        diesel::insert_into(coverage_files::table)
            .values(rows)
            .on_conflict((coverage_files::snapshot_id, coverage_files::path_id))
            .do_nothing()
            .execute(conn)?;
    }

    Ok(())
}

pub(crate) async fn get_coverage_snapshots(
    State(pool): State<DbPool>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<Vec<CoverageSnapshotWithMetrics>>, StatusCode> {
    let mut conn = pool.get().map_err(|e| {
        error!("Failed to get DB connection: {e}");
        StatusCode::SERVICE_UNAVAILABLE
    })?;

    let mut query = coverage_snapshots::table
        .select(CoverageSnapshot::as_select())
        .into_boxed();

    if let Some(scheduler) = params.get("scheduler") {
        query = query.filter(coverage_snapshots::scheduler_name.eq(scheduler));
    }
    if let Some(after) = params.get("after") {
        if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(after, "%Y-%m-%d %H:%M:%S%.f") {
            query = query.filter(coverage_snapshots::captured_at.gt(dt));
        } else {
            return Err(StatusCode::BAD_REQUEST);
        }
    }
    if let Some(before) = params.get("before") {
        if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(before, "%Y-%m-%d %H:%M:%S%.f") {
            query = query.filter(coverage_snapshots::captured_at.lt(dt));
        } else {
            return Err(StatusCode::BAD_REQUEST);
        }
    }

    let limit: i64 = params
        .get("limit")
        .and_then(|l| l.parse::<i64>().ok())
        .unwrap_or(DEFAULT_SNAPSHOT_LIMIT)
        .clamp(1, MAX_SNAPSHOT_LIMIT);

    let snapshots = query
        .order(coverage_snapshots::captured_at.desc())
        .limit(limit)
        .get_results::<CoverageSnapshot>(&mut conn)
        .map_err(|e| {
            error!("Database query failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let snapshot_ids: Vec<i32> = snapshots.iter().map(|s| s.id).collect();
    let all_metrics = coverage_metrics::table
        .filter(coverage_metrics::snapshot_id.eq_any(&snapshot_ids))
        .select(CoverageMetric::as_select())
        .get_results::<CoverageMetric>(&mut conn)
        .map_err(|e| {
            error!("Database query failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let mut metrics_map: HashMap<i32, Vec<CoverageMetric>> = HashMap::new();
    for m in all_metrics {
        metrics_map.entry(m.snapshot_id).or_default().push(m);
    }

    let result: Vec<CoverageSnapshotWithMetrics> = snapshots
        .into_iter()
        .map(|s| {
            let metrics = metrics_map.remove(&s.id).unwrap_or_default();
            CoverageSnapshotWithMetrics {
                snapshot: s,
                metrics,
            }
        })
        .collect();

    Ok(Json(result))
}

pub(crate) async fn get_latest_coverage_snapshot(
    State(pool): State<DbPool>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<CoverageSnapshotWithMetrics>, StatusCode> {
    let mut conn = pool.get().map_err(|e| {
        error!("Failed to get DB connection: {e}");
        StatusCode::SERVICE_UNAVAILABLE
    })?;

    let mut query = coverage_snapshots::table
        .select(CoverageSnapshot::as_select())
        .into_boxed();

    if let Some(scheduler) = params.get("scheduler") {
        query = query.filter(coverage_snapshots::scheduler_name.eq(scheduler));
    }

    let snapshot = query
        .order(coverage_snapshots::captured_at.desc())
        .first::<CoverageSnapshot>(&mut conn)
        .map_err(|e| match e {
            diesel::result::Error::NotFound => StatusCode::NOT_FOUND,
            _ => {
                error!("Database query failed: {e}");
                StatusCode::INTERNAL_SERVER_ERROR
            }
        })?;

    let metrics = coverage_metrics::table
        .filter(coverage_metrics::snapshot_id.eq(snapshot.id))
        .select(CoverageMetric::as_select())
        .get_results::<CoverageMetric>(&mut conn)
        .map_err(|e| {
            error!("Database query failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(Json(CoverageSnapshotWithMetrics { snapshot, metrics }))
}
