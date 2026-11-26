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

use crate::report::types::MAX_FUZZERS_PER_HEARTBEAT;
use crate::schema::{
    coverage_files, coverage_metrics, coverage_paths, coverage_snapshots, fuzzer_coverage_points,
    fuzzer_runtime,
};
use crate::DbPool;

/// Upper bound on per-file rows accepted in one snapshot.
const MAX_COVERAGE_FILES: usize = 5000;

/// Minimum spacing between two snapshots of the same build.
const MIN_RESNAPSHOT_SECS: i64 = 300;

/// Snapshots returned when the caller does not ask for a specific page size.
const DEFAULT_SNAPSHOT_LIMIT: i64 = 100;

/// Hard cap on a snapshot listing.
const MAX_SNAPSHOT_LIMIT: i64 = 1000;

/// Upper bound on the metrics of the project and of each fuzzer.
const MAX_METRICS: usize = 20;

/// Upper bound on the file name of a fuzzer binary.
const MAX_FUZZER_NAME_LEN: usize = 256;

#[derive(Deserialize, Serialize)]
pub(crate) struct CreateCoverageSnapshot {
    scheduler_name: String,
    fuzzer_engine: String,
    build_fingerprint: String,
    num_fuzzers: i32,
    metrics: Vec<CoverageMetricInput>,
    #[serde(default)]
    files: Vec<CoverageFileInput>,
    /// Coverage of each fuzzer binary alone. Older schedulers send none.
    #[serde(default)]
    fuzzers: Vec<FuzzerCoverageInput>,
}

#[derive(Deserialize, Serialize, Clone)]
struct FuzzerCoverageInput {
    sha: String,
    /// File name of the binary. Older schedulers send none.
    #[serde(default)]
    name: Option<String>,
    metrics: Vec<CoverageMetricInput>,
}

/// Metrics stored in the columns of `fuzzer_coverage_points`, in column order.
const POINT_METRICS: [&str; 4] = ["lines", "functions", "regions", "branches"];

/// `(covered, total)` of each of [`POINT_METRICS`]. A missing metric is
/// `(0, 0)`, and a metric with another name is ignored.
fn point_counts(metrics: &[CoverageMetricInput]) -> [(i32, i32); 4] {
    POINT_METRICS.map(|name| {
        metrics
            .iter()
            .find(|m| m.metric_name == name)
            .map_or((0, 0), |m| (m.covered, m.total))
    })
}

fn validate_metrics(metrics: &[CoverageMetricInput]) -> Result<(), String> {
    if metrics.len() > MAX_METRICS {
        return Err(format!("Too many metrics (max {MAX_METRICS})"));
    }
    for m in metrics {
        if m.metric_name.is_empty() || m.metric_name.len() > 64 {
            return Err("Invalid metric_name".to_string());
        }
        if m.total < 0 || m.covered < 0 || m.covered > m.total {
            return Err(format!("Invalid metric values for '{}'", m.metric_name));
        }
    }
    Ok(())
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

/// One point of the coverage series of a fuzzer binary.
#[derive(Serialize, Queryable, Selectable)]
#[diesel(table_name = fuzzer_coverage_points)]
pub(crate) struct FuzzerCoveragePoint {
    scheduler_name: String,
    fuzzer_sha: String,
    /// File name of the binary, `null` for a point sent by an older scheduler.
    fuzzer_name: Option<String>,
    captured_at: NaiveDateTime,
    runtime_seconds: Option<i64>,
    lines_covered: i32,
    lines_total: i32,
    functions_covered: i32,
    functions_total: i32,
    regions_covered: i32,
    regions_total: i32,
    branches_covered: i32,
    branches_total: i32,
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
    validate_metrics(&payload.metrics).map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    if payload.fuzzers.len() > MAX_FUZZERS_PER_HEARTBEAT {
        return Err((
            StatusCode::BAD_REQUEST,
            format!("Too many fuzzers (max {MAX_FUZZERS_PER_HEARTBEAT})"),
        ));
    }
    for f in &payload.fuzzers {
        if f.sha.is_empty() || f.sha.len() > 128 {
            return Err((StatusCode::BAD_REQUEST, "Invalid fuzzer sha".to_string()));
        }
        if f.name
            .as_ref()
            .is_some_and(|n| n.is_empty() || n.len() > MAX_FUZZER_NAME_LEN)
        {
            return Err((StatusCode::BAD_REQUEST, "Invalid fuzzer name".to_string()));
        }
        validate_metrics(&f.metrics)
            .map_err(|e| (StatusCode::BAD_REQUEST, format!("Fuzzer {}: {e}", f.sha)))?;
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
            let (snapshot_id, captured_at): (i32, NaiveDateTime) =
                diesel::insert_into(coverage_snapshots::table)
                    .values((
                        coverage_snapshots::scheduler_name.eq(&payload.scheduler_name),
                        coverage_snapshots::fuzzer_engine.eq(&payload.fuzzer_engine),
                        coverage_snapshots::build_fingerprint.eq(&payload.build_fingerprint),
                        coverage_snapshots::num_fuzzers.eq(payload.num_fuzzers),
                    ))
                    .returning((coverage_snapshots::id, coverage_snapshots::captured_at))
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

            insert_fuzzer_points(
                conn,
                snapshot_id,
                captured_at,
                &payload.scheduler_name,
                &payload.fuzzers,
            )?;
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
        "Coverage snapshot created id={snapshot_id} for scheduler={} ({} files, {} fuzzers)",
        payload.scheduler_name,
        payload.files.len(),
        payload.fuzzers.len()
    );
    Ok(StatusCode::CREATED)
}

/// Losing that race twice in a row would take a prune per attempt: three
/// attempts is plenty.
const PATH_LOOKUP_ATTEMPTS: usize = 3;

fn insert_coverage_files(
    conn: &mut PgConnection,
    snapshot_id: i32,
    files: &[CoverageFileInput],
) -> Result<(), diesel::result::Error> {
    if files.is_empty() {
        return Ok(());
    }

    // Maintenance may prune an orphan path between its insert and its lookup,
    // so missing paths are inserted again. Once looked up, a path is locked
    // until the transaction ends: the prune cannot delete it any more.
    let mut path_ids: HashMap<String, i32> = HashMap::new();
    for _ in 0..PATH_LOOKUP_ATTEMPTS {
        let missing: Vec<&str> = files
            .iter()
            .map(|f| f.path.as_str())
            .filter(|path| !path_ids.contains_key(*path))
            .collect();
        if missing.is_empty() {
            break;
        }
        for chunk in missing.chunks(1000) {
            let new_paths: Vec<_> = chunk.iter().map(|p| coverage_paths::path.eq(*p)).collect();
            diesel::insert_into(coverage_paths::table)
                .values(new_paths)
                .on_conflict(coverage_paths::path)
                .do_nothing()
                .execute(conn)?;
        }
        path_ids.extend(
            coverage_paths::table
                .filter(coverage_paths::path.eq_any(&missing))
                .select((coverage_paths::path, coverage_paths::id))
                .for_key_share()
                .load::<(String, i32)>(conn)?,
        );
    }

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

/// One point of the coverage series of each fuzzer, dated like its snapshot.
fn insert_fuzzer_points(
    conn: &mut PgConnection,
    snapshot_id: i32,
    captured_at: NaiveDateTime,
    scheduler_name: &str,
    fuzzers: &[FuzzerCoverageInput],
) -> Result<(), diesel::result::Error> {
    if fuzzers.is_empty() {
        return Ok(());
    }

    let shas: Vec<&str> = fuzzers.iter().map(|f| f.sha.as_str()).collect();
    let runtimes: HashMap<String, i64> = fuzzer_runtime::table
        .filter(fuzzer_runtime::scheduler_name.eq(scheduler_name))
        .filter(fuzzer_runtime::fuzzer_sha.eq_any(&shas))
        .select((
            fuzzer_runtime::fuzzer_sha,
            fuzzer_runtime::total_runtime_seconds,
        ))
        .load::<(String, i64)>(conn)?
        .into_iter()
        .collect();

    let rows: Vec<_> = fuzzers
        .iter()
        .map(|f| {
            let [lines, functions, regions, branches] = point_counts(&f.metrics);
            (
                fuzzer_coverage_points::snapshot_id.eq(snapshot_id),
                fuzzer_coverage_points::scheduler_name.eq(scheduler_name),
                fuzzer_coverage_points::fuzzer_sha.eq(&f.sha),
                fuzzer_coverage_points::fuzzer_name.eq(f.name.as_deref()),
                fuzzer_coverage_points::captured_at.eq(captured_at),
                fuzzer_coverage_points::runtime_seconds.eq(runtimes.get(&f.sha).copied()),
                fuzzer_coverage_points::lines_covered.eq(lines.0),
                fuzzer_coverage_points::lines_total.eq(lines.1),
                fuzzer_coverage_points::functions_covered.eq(functions.0),
                fuzzer_coverage_points::functions_total.eq(functions.1),
                fuzzer_coverage_points::regions_covered.eq(regions.0),
                fuzzer_coverage_points::regions_total.eq(regions.1),
                fuzzer_coverage_points::branches_covered.eq(branches.0),
                fuzzer_coverage_points::branches_total.eq(branches.1),
            )
        })
        .collect();
    diesel::insert_into(fuzzer_coverage_points::table)
        .values(rows)
        .on_conflict((
            fuzzer_coverage_points::snapshot_id,
            fuzzer_coverage_points::fuzzer_sha,
        ))
        .do_nothing()
        .execute(conn)?;
    Ok(())
}

fn date_param(
    params: &HashMap<String, String>,
    key: &str,
) -> Result<Option<NaiveDateTime>, StatusCode> {
    params
        .get(key)
        .map(|value| {
            NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S%.f")
                .map_err(|_| StatusCode::BAD_REQUEST)
        })
        .transpose()
}

fn limit_param(params: &HashMap<String, String>) -> i64 {
    params
        .get("limit")
        .and_then(|l| l.parse::<i64>().ok())
        .unwrap_or(DEFAULT_SNAPSHOT_LIMIT)
        .clamp(1, MAX_SNAPSHOT_LIMIT)
}

/// Coverage series of the fuzzer binaries, newest point first.
pub(crate) async fn get_fuzzer_coverage_points(
    State(pool): State<DbPool>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<Vec<FuzzerCoveragePoint>>, StatusCode> {
    let mut conn = pool.get().map_err(|e| {
        error!("Failed to get DB connection: {e}");
        StatusCode::SERVICE_UNAVAILABLE
    })?;

    let mut query = fuzzer_coverage_points::table
        .select(FuzzerCoveragePoint::as_select())
        .into_boxed();

    if let Some(scheduler) = params.get("scheduler") {
        query = query.filter(fuzzer_coverage_points::scheduler_name.eq(scheduler));
    }
    if let Some(sha) = params.get("sha") {
        query = query.filter(fuzzer_coverage_points::fuzzer_sha.eq(sha));
    }
    if let Some(dt) = date_param(&params, "after")? {
        query = query.filter(fuzzer_coverage_points::captured_at.gt(dt));
    }
    if let Some(dt) = date_param(&params, "before")? {
        query = query.filter(fuzzer_coverage_points::captured_at.lt(dt));
    }

    let points = query
        .order((
            fuzzer_coverage_points::captured_at.desc(),
            fuzzer_coverage_points::fuzzer_sha.asc(),
        ))
        .limit(limit_param(&params))
        .load::<FuzzerCoveragePoint>(&mut conn)
        .map_err(|e| {
            error!("Database query failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(Json(points))
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
    if let Some(dt) = date_param(&params, "after")? {
        query = query.filter(coverage_snapshots::captured_at.gt(dt));
    }
    if let Some(dt) = date_param(&params, "before")? {
        query = query.filter(coverage_snapshots::captured_at.lt(dt));
    }

    let snapshots = query
        .order(coverage_snapshots::captured_at.desc())
        .limit(limit_param(&params))
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

#[cfg(test)]
mod tests {
    use super::{point_counts, validate_metrics, CoverageMetricInput, CreateCoverageSnapshot};

    fn metric(name: &str, covered: i32, total: i32) -> CoverageMetricInput {
        CoverageMetricInput {
            metric_name: name.to_string(),
            covered,
            total,
        }
    }

    #[test]
    fn valid_metrics_are_accepted() {
        assert!(validate_metrics(&[metric("lines", 3, 10), metric("branches", 0, 0)]).is_ok());
    }

    #[test]
    fn more_covered_than_total_is_refused() {
        assert!(validate_metrics(&[metric("lines", 11, 10)]).is_err());
        assert!(validate_metrics(&[metric("lines", -1, 10)]).is_err());
    }

    #[test]
    fn an_empty_metric_name_is_refused() {
        assert!(validate_metrics(&[metric("", 1, 10)]).is_err());
    }

    #[test]
    fn too_many_metrics_are_refused() {
        let metrics: Vec<_> = (0..21).map(|i| metric(&format!("m{i}"), 0, 1)).collect();
        assert!(validate_metrics(&metrics).is_err());
    }

    #[test]
    fn a_missing_metric_is_stored_as_zero_of_zero() {
        let counts = point_counts(&[
            metric("regions", 5, 8),
            metric("lines", 3, 10),
            metric("unknown", 1, 1),
        ]);
        assert_eq!(counts, [(3, 10), (0, 0), (5, 8), (0, 0)]);
    }

    #[test]
    fn a_snapshot_without_fuzzers_stays_valid() {
        let payload: CreateCoverageSnapshot = serde_json::from_str(
            r#"{"scheduler_name":"s","fuzzer_engine":"libfuzzer","build_fingerprint":"ab","num_fuzzers":1,"metrics":[]}"#,
        )
        .unwrap();
        assert!(payload.fuzzers.is_empty());
    }

    #[test]
    fn a_snapshot_carries_the_metrics_of_each_fuzzer() {
        let payload: CreateCoverageSnapshot = serde_json::from_str(
            r#"{"scheduler_name":"s","fuzzer_engine":"libfuzzer","build_fingerprint":"ab","num_fuzzers":1,"metrics":[],
                "fuzzers":[{"sha":"0a1b","name":"fuzz_parser","metrics":[{"metric_name":"lines","covered":4,"total":9}]}]}"#,
        )
        .unwrap();
        assert_eq!(payload.fuzzers[0].sha, "0a1b");
        assert_eq!(payload.fuzzers[0].name.as_deref(), Some("fuzz_parser"));
        assert_eq!(point_counts(&payload.fuzzers[0].metrics)[0], (4, 9));
    }

    #[test]
    fn a_fuzzer_without_name_stays_valid() {
        let payload: CreateCoverageSnapshot = serde_json::from_str(
            r#"{"scheduler_name":"s","fuzzer_engine":"libfuzzer","build_fingerprint":"ab","num_fuzzers":1,"metrics":[],
                "fuzzers":[{"sha":"0a1b","metrics":[]}]}"#,
        )
        .unwrap();
        assert_eq!(payload.fuzzers[0].name, None);
    }
}
