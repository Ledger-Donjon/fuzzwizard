// SPDX-FileCopyrightText: © 2026 LEDGER SAS
// SPDX-License-Identifier: MIT

use crate::schema::{admin_events, coverage_metrics, coverage_snapshots, crashes};
use axum::{
    extract::{DefaultBodyLimit, Query, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use chrono::NaiveDateTime;
use diesel::prelude::*;
use diesel::r2d2::{self, ConnectionManager, Pool};
use ftail::Ftail;
use log::LevelFilter;
use log::{error, info};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, env, fs, path::Path, time::Duration};

mod schema;

// Type alias pour le pool de connexions
type DbPool = Pool<ConnectionManager<PgConnection>>;

#[tokio::main]
async fn main() {
    // Read database URL from environment variable (preferred) or CLI argument
    let db_url = if let Ok(url) = env::var("DATABASE_URL") {
        url
    } else {
        let args: Vec<String> = env::args().collect();
        let mut url = String::new();
        for i in 0..args.len() {
            if args[i] == "--database-url" || args[i] == "--db-url" {
                if i + 1 < args.len() {
                    url = args[i + 1].clone();
                } else {
                    eprintln!("Error: {} requires a value", args[i]);
                    std::process::exit(1);
                }
            }
        }
        if url.is_empty() {
            eprintln!("Error: DATABASE_URL env var or --database-url argument is required");
            std::process::exit(1);
        }
        url
    };

    // Configure ftail to write to a file
    let log_file_path = "./logs/backend.log";
    // Ensure logs directory exists
    if let Err(e) = std::fs::create_dir_all("./logs") {
        error!("Failed to create log directory './logs': {e}");
        std::process::exit(1);
    }

    if let Err(e) = Ftail::new()
        .single_file(std::path::Path::new(log_file_path), true, LevelFilter::Info)
        .init()
    {
        error!("Failed to initialize logger: {e}");
        std::process::exit(1);
    }

    // Create a connection pool
    let manager = ConnectionManager::<PgConnection>::new(&db_url);
    let pool = match r2d2::Pool::builder()
        .max_size(10)
        .min_idle(Some(2))
        .connection_timeout(Duration::from_secs(5))
        .build(manager)
    {
        Ok(pool) => pool,
        Err(e) => {
            error!("Failed to create database pool: {e}");
            std::process::exit(1);
        }
    };

    info!("Database pool created with max_size=10");

    let app = Router::new()
        .route("/crashes", get(get_crashes))
        .route("/crashes/last", get(get_last_crash))
        .route("/crashes/last/{number}", get(get_last_n_crashes))
        .route("/crashes/count", get(get_crashes_count))
        .route("/crashes/ids", get(get_crash_ids))
        .route("/crashes", post(upload_crash))
        .route("/crashes/{id}", get(get_crash_by_id))
        .route("/collect_crashes", post(collect_crashes))
        .route("/admin_events", post(create_admin_event))
        .route(
            "/admin_events/unprocessed",
            get(get_unprocessed_admin_events),
        )
        .route(
            "/admin_events/{id}/mark_processed",
            post(mark_admin_event_processed),
        )
        .route(
            "/coverage_snapshots",
            post(create_coverage_snapshot).get(get_coverage_snapshots),
        )
        .route(
            "/coverage_snapshots/latest",
            get(get_latest_coverage_snapshot),
        )
        .route(
            "/maintenance/preview",
            post(maintenance_preview),
        )
        .route(
            "/maintenance/delete",
            post(maintenance_delete),
        )
        .layer(DefaultBodyLimit::max(110 * 1024 * 1024))
        .with_state(pool);

    let listener = match tokio::net::TcpListener::bind("127.0.0.1:3000").await {
        Ok(listener) => {
            info!("Backend listening on 127.0.0.1:3000");
            listener
        }
        Err(e) => {
            error!("Failed to bind to 127.0.0.1:3000: {e}");
            std::process::exit(1);
        }
    };

    if let Err(e) = axum::serve(listener, app).await {
        error!("Server error: {e}");
        std::process::exit(1);
    }
}

#[derive(Deserialize, Serialize, Insertable, Queryable, Clone)]
#[diesel(table_name = crashes)]
struct AddCrashReport {
    report: Vec<u8>,
    input: Vec<u8>,
    filename: String,
    program: Vec<u8>,
    scheduler_name: String,
}

#[derive(Deserialize, Serialize, Queryable, Selectable)]
#[diesel(table_name = crashes)]
struct CrashReportWithId {
    id: i32,
    report: Vec<u8>,
    input: Vec<u8>,
    filename: String,
    program: Vec<u8>,
    scheduler_name: String,
    created_at: NaiveDateTime,
}

#[derive(Deserialize, Serialize, Queryable, Selectable)]
#[diesel(table_name = crashes)]
struct CrashReport {
    report: Vec<u8>,
    input: Vec<u8>,
    filename: String,
    program: Vec<u8>,
    scheduler_name: String,
    created_at: NaiveDateTime,
}

async fn get_crashes(
    State(pool): State<DbPool>,
) -> Result<Json<Vec<CrashReportWithId>>, StatusCode> {
    // Obtenir une connexion du pool
    let mut conn = pool.get().map_err(|e| {
        error!("Failed to get DB connection from pool: {e}");
        StatusCode::SERVICE_UNAVAILABLE
    })?;

    let crashes = crashes::table
        .select(CrashReportWithId::as_select())
        .get_results::<CrashReportWithId>(&mut conn)
        .map_err(|e| {
            error!("Database query failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(Json(crashes))
}

async fn get_last_crash(State(pool): State<DbPool>) -> Result<Json<CrashReportWithId>, StatusCode> {
    let mut conn = pool.get().map_err(|e| {
        error!("Failed to get DB connection from pool: {e}");
        StatusCode::SERVICE_UNAVAILABLE
    })?;

    let crash = crashes::table
        .select(CrashReportWithId::as_select())
        .order(crashes::created_at.desc())
        .limit(1)
        .get_result::<CrashReportWithId>(&mut conn)
        .map_err(|e| {
            error!("Database query failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(Json(crash))
}

async fn get_last_n_crashes(
    State(pool): State<DbPool>,
    axum::extract::Path(number): axum::extract::Path<i64>,
) -> Result<Json<Vec<CrashReportWithId>>, StatusCode> {
    let mut conn = pool.get().map_err(|e| {
        error!("Failed to get DB connection from pool: {e}");
        StatusCode::SERVICE_UNAVAILABLE
    })?;
    info!("Getting last {number} crashes");

    let crashes = crashes::table
        .select(CrashReportWithId::as_select())
        .order(crashes::created_at.desc())
        .limit(number)
        .get_results::<CrashReportWithId>(&mut conn)
        .map_err(|e| {
            error!("Database query failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(Json(crashes))
}

async fn get_crashes_count(State(pool): State<DbPool>) -> Result<Json<i64>, StatusCode> {
    let mut conn = pool.get().map_err(|e| {
        error!("Failed to get DB connection from pool: {e}");
        StatusCode::SERVICE_UNAVAILABLE
    })?;

    let count = crashes::table
        .count()
        .get_result::<i64>(&mut conn)
        .map_err(|e| {
            error!("Database query failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(Json(count))
}

async fn upload_crash(
    State(pool): State<DbPool>,
    Json(report): Json<AddCrashReport>,
) -> Result<StatusCode, (StatusCode, String)> {
    // Validation des tailles AVANT d'obtenir la connexion
    const MAX_PROGRAM_SIZE: usize = 100 * 1024 * 1024; // 100MB
    const MAX_INPUT_SIZE: usize = 10 * 1024 * 1024; // 10MB
    const MAX_REPORT_SIZE: usize = 1024 * 1024; // 1MB

    if report.program.len() > MAX_PROGRAM_SIZE {
        return Err((
            StatusCode::PAYLOAD_TOO_LARGE,
            format!(
                "Program size {} exceeds {}MB limit",
                report.program.len(),
                MAX_PROGRAM_SIZE / 1024 / 1024
            ),
        ));
    }

    if report.input.len() > MAX_INPUT_SIZE {
        return Err((
            StatusCode::PAYLOAD_TOO_LARGE,
            format!(
                "Input size {} exceeds {}MB limit",
                report.input.len(),
                MAX_INPUT_SIZE / 1024 / 1024
            ),
        ));
    }

    if report.report.len() > MAX_REPORT_SIZE {
        return Err((
            StatusCode::PAYLOAD_TOO_LARGE,
            format!(
                "Report size {} exceeds {}MB limit",
                report.report.len(),
                MAX_REPORT_SIZE / 1024 / 1024
            ),
        ));
    }

    // Obtenir une connexion du pool
    let mut conn = pool.get().map_err(|e| {
        error!("Failed to get DB connection: {e}");
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "Database unavailable".to_string(),
        )
    })?;

    // Insert into the database
    match diesel::insert_into(crashes::table)
        .values(report)
        .returning(crashes::id)
        .get_results::<i32>(&mut conn)
    {
        Ok(ids) => {
            info!("Crash uploaded with ids: {ids:?}");
            Ok(StatusCode::CREATED)
        }
        Err(e) => {
            error!("Failed to insert crash: {e}");
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                "Database error".to_string(),
            ))
        }
    }
}

#[axum::debug_handler]
async fn get_crash_by_id(
    State(pool): State<DbPool>,
    axum::extract::Path(id): axum::extract::Path<i32>,
) -> Result<Json<CrashReportWithId>, StatusCode> {
    let mut conn = pool.get().map_err(|e| {
        error!("Failed to get DB connection from pool: {e}");
        StatusCode::SERVICE_UNAVAILABLE
    })?;

    match crashes::table
        .select(CrashReportWithId::as_select())
        .filter(crashes::id.eq(id))
        .get_result::<CrashReportWithId>(&mut conn)
    {
        Ok(crash) => Ok(Json(crash)),
        Err(diesel::result::Error::NotFound) => Err(StatusCode::NOT_FOUND),
        Err(e) => {
            error!("Database query failed: {e}");
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

async fn collect_crashes(
    State(pool): State<DbPool>,
    Json(id): Json<i32>,
) -> Result<(StatusCode, Json<String>), StatusCode> {
    let mut conn = pool.get().map_err(|e| {
        error!("Failed to get DB connection from pool: {e}");
        StatusCode::SERVICE_UNAVAILABLE
    })?;

    let crash = crashes::table
        .select(CrashReport::as_select())
        .filter(crashes::id.eq(id))
        .get_result::<CrashReport>(&mut conn)
        .optional()
        .map_err(|e| {
            error!("Database query failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let crash = match crash {
        Some(c) => c,
        None => {
            return Err(StatusCode::NOT_FOUND);
        }
    };

    let s_filename = Path::new(&crash.filename)
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();

    let stem = Path::new(&s_filename)
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
        .collect::<String>();

    if stem.is_empty() {
        error!("Invalid filename in crash record id={id}");
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }

    let created_fmt = crash.created_at.format("%Y-%m-%dT%H-%M-%S");
    let dir_name = format!("/tmp/tmp_{created_fmt}_{stem}");

    // Create directory
    fs::create_dir_all(&dir_name).map_err(|e| {
        error!("Failed to create directory {dir_name}: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let program_path = Path::new(&dir_name).join(&s_filename);
    let report_path = Path::new(&dir_name).join("crash_info.txt");
    let input_path = Path::new(&dir_name).join("crash_input.txt");

    // Write files
    fs::write(&program_path, &crash.program).map_err(|e| {
        error!("Failed to write program file: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    fs::write(&report_path, &crash.report).map_err(|e| {
        error!("Failed to write report file: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    fs::write(&input_path, &crash.input).map_err(|e| {
        error!("Failed to write input file: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    Ok((StatusCode::CREATED, Json(dir_name)))
}

// Admin Events structures and endpoints

#[derive(Deserialize, Serialize)]
struct CreateAdminEvent {
    event_type: String,
    message: String,
    metadata: Option<serde_json::Value>,
}

#[derive(Deserialize, Serialize, Queryable, Selectable)]
#[diesel(table_name = admin_events)]
struct AdminEvent {
    id: i32,
    event_type: String,
    message: String,
    metadata: Option<String>,
    created_at: NaiveDateTime,
    processed: bool,
}

async fn create_admin_event(
    State(pool): State<DbPool>,
    Json(event): Json<CreateAdminEvent>,
) -> Result<StatusCode, (StatusCode, String)> {
    // Validation des tailles AVANT d'obtenir la connexion
    const MAX_EVENT_TYPE_SIZE: usize = 256; // 256 characters for event type
    const MAX_MESSAGE_SIZE: usize = 64 * 1024; // 64KB for message
    const MAX_METADATA_SIZE: usize = 1024 * 1024; // 1MB for JSON metadata

    if event.event_type.len() > MAX_EVENT_TYPE_SIZE {
        return Err((
            StatusCode::PAYLOAD_TOO_LARGE,
            format!(
                "Event type size {} exceeds {} character limit",
                event.event_type.len(),
                MAX_EVENT_TYPE_SIZE
            ),
        ));
    }

    if event.message.len() > MAX_MESSAGE_SIZE {
        return Err((
            StatusCode::PAYLOAD_TOO_LARGE,
            format!(
                "Message size {} exceeds {}KB limit",
                event.message.len(),
                MAX_MESSAGE_SIZE / 1024
            ),
        ));
    }

    // Validate and serialize JSON metadata
    let metadata_str = if let Some(ref metadata) = event.metadata {
        let metadata_json = serde_json::to_string(metadata).map_err(|e| {
            error!("Failed to serialize metadata: {e}");
            (
                StatusCode::BAD_REQUEST,
                "Invalid metadata format".to_string(),
            )
        })?;

        if metadata_json.len() > MAX_METADATA_SIZE {
            return Err((
                StatusCode::PAYLOAD_TOO_LARGE,
                format!(
                    "Metadata size {} exceeds {}MB limit",
                    metadata_json.len(),
                    MAX_METADATA_SIZE / 1024 / 1024
                ),
            ));
        }

        Some(metadata_json)
    } else {
        None
    };

    let mut conn = pool.get().map_err(|e| {
        error!("Failed to get DB connection: {e}");
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "Database unavailable".to_string(),
        )
    })?;

    match diesel::insert_into(admin_events::table)
        .values((
            admin_events::event_type.eq(event.event_type),
            admin_events::message.eq(event.message),
            admin_events::metadata.eq(metadata_str.as_deref()),
        ))
        .returning(admin_events::id)
        .get_results::<i32>(&mut conn)
    {
        Ok(ids) => {
            info!("Admin event created with id: {ids:?}");
            Ok(StatusCode::CREATED)
        }
        Err(e) => {
            error!("Failed to insert admin event: {e}");
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                "Database error".to_string(),
            ))
        }
    }
}

async fn get_unprocessed_admin_events(
    State(pool): State<DbPool>,
) -> Result<Json<Vec<AdminEvent>>, StatusCode> {
    let mut conn = pool.get().map_err(|e| {
        error!("Failed to get DB connection from pool: {e}");
        StatusCode::SERVICE_UNAVAILABLE
    })?;

    let events = admin_events::table
        .select(AdminEvent::as_select())
        .filter(admin_events::processed.eq(false))
        .order(admin_events::created_at.asc())
        .get_results::<AdminEvent>(&mut conn)
        .map_err(|e| {
            error!("Database query failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(Json(events))
}

async fn mark_admin_event_processed(
    State(pool): State<DbPool>,
    axum::extract::Path(id): axum::extract::Path<i32>,
) -> Result<StatusCode, (StatusCode, String)> {
    let mut conn = pool.get().map_err(|e| {
        error!("Failed to get DB connection: {e}");
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "Database unavailable".to_string(),
        )
    })?;

    match diesel::update(admin_events::table.filter(admin_events::id.eq(id)))
        .set(admin_events::processed.eq(true))
        .execute(&mut conn)
    {
        Ok(0) => Err((StatusCode::NOT_FOUND, "Event not found".to_string())),
        Ok(_) => Ok(StatusCode::OK),
        Err(e) => {
            error!("Failed to update admin event: {e}");
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                "Database error".to_string(),
            ))
        }
    }
}

#[derive(Serialize)]
struct CrashId {
    id: i32,
    created_at: NaiveDateTime,
}

async fn get_crash_ids(
    State(pool): State<DbPool>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<Vec<CrashId>>, StatusCode> {
    let mut conn = pool.get().map_err(|e| {
        error!("Failed to get DB connection from pool: {e}");
        StatusCode::SERVICE_UNAVAILABLE
    })?;

    let mut query = crashes::table
        .select((crashes::id, crashes::created_at))
        .into_boxed();

    if let Some(after) = params.get("after") {
        match chrono::NaiveDateTime::parse_from_str(after, "%Y-%m-%d %H:%M:%S%.f") {
            Ok(after_date) => {
                query = query.filter(crashes::created_at.gt(after_date));
            }
            Err(_) => {
                return Err(StatusCode::BAD_REQUEST);
            }
        }
    }

    let crash_ids = query
        .order(crashes::created_at.asc())
        .get_results::<(i32, NaiveDateTime)>(&mut conn)
        .map_err(|e| {
            error!("Database query failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .into_iter()
        .map(|(id, created_at)| CrashId { id, created_at })
        .collect();

    Ok(Json(crash_ids))
}

// --- Coverage Snapshots ---

#[derive(Deserialize, Serialize)]
struct CreateCoverageSnapshot {
    scheduler_name: String,
    fuzzer_engine: String,
    build_fingerprint: String,
    num_fuzzers: i32,
    metrics: Vec<CoverageMetricInput>,
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
struct CoverageSnapshotWithMetrics {
    #[serde(flatten)]
    snapshot: CoverageSnapshot,
    metrics: Vec<CoverageMetric>,
}

async fn create_coverage_snapshot(
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
        return Err((
            StatusCode::BAD_REQUEST,
            "Invalid fuzzer_engine".to_string(),
        ));
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
        if m.total < 0 || m.covered < 0 || m.covered > m.total {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("Invalid metric values for '{}'", m.metric_name),
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

    // Deduplicate: skip if last snapshot for this scheduler has the same fingerprint
    let existing = coverage_snapshots::table
        .filter(coverage_snapshots::scheduler_name.eq(&payload.scheduler_name))
        .filter(coverage_snapshots::build_fingerprint.eq(&payload.build_fingerprint))
        .order(coverage_snapshots::captured_at.desc())
        .select(coverage_snapshots::id)
        .first::<i32>(&mut conn)
        .optional()
        .map_err(|e| {
            error!("Dedup query failed: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Database error".to_string(),
            )
        })?;

    if existing.is_some() {
        return Ok(StatusCode::OK);
    }

    let snapshot_id: i32 = diesel::insert_into(coverage_snapshots::table)
        .values((
            coverage_snapshots::scheduler_name.eq(&payload.scheduler_name),
            coverage_snapshots::fuzzer_engine.eq(&payload.fuzzer_engine),
            coverage_snapshots::build_fingerprint.eq(&payload.build_fingerprint),
            coverage_snapshots::num_fuzzers.eq(payload.num_fuzzers),
        ))
        .returning(coverage_snapshots::id)
        .get_result(&mut conn)
        .map_err(|e| {
            error!("Failed to insert coverage snapshot: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Database error".to_string(),
            )
        })?;

    for m in &payload.metrics {
        diesel::insert_into(coverage_metrics::table)
            .values((
                coverage_metrics::snapshot_id.eq(snapshot_id),
                coverage_metrics::metric_name.eq(&m.metric_name),
                coverage_metrics::covered.eq(m.covered),
                coverage_metrics::total.eq(m.total),
            ))
            .execute(&mut conn)
            .map_err(|e| {
                error!("Failed to insert coverage metric: {e}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Database error".to_string(),
                )
            })?;
    }

    info!("Coverage snapshot created id={snapshot_id} for scheduler={}", payload.scheduler_name);
    Ok(StatusCode::CREATED)
}

async fn get_coverage_snapshots(
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
        .and_then(|l| l.parse().ok())
        .unwrap_or(100);

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
            CoverageSnapshotWithMetrics { snapshot: s, metrics }
        })
        .collect();

    Ok(Json(result))
}

async fn get_latest_coverage_snapshot(
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

// --- Maintenance (cleanup) ---

#[derive(Deserialize)]
struct MaintenanceRequest {
    #[serde(default)]
    crashes_before: Option<String>,
    #[serde(default)]
    admin_events_before: Option<String>,
    #[serde(default)]
    coverage_before: Option<String>,
}

#[derive(Serialize)]
struct MaintenancePreviewResponse {
    crashes_count: i64,
    admin_events_count: i64,
    coverage_snapshots_count: i64,
}

fn parse_maintenance_date(s: &str) -> Option<NaiveDateTime> {
    chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S%.f")
        .or_else(|_| {
            chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d")
                .map(|d| d.and_hms_opt(0, 0, 0).unwrap())
        })
        .ok()
}

async fn maintenance_preview(
    State(pool): State<DbPool>,
    Json(req): Json<MaintenanceRequest>,
) -> Result<Json<MaintenancePreviewResponse>, (StatusCode, String)> {
    let mut conn = pool.get().map_err(|e| {
        error!("Failed to get DB connection: {e}");
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "Database unavailable".to_string(),
        )
    })?;

    let crashes_count = if let Some(ref before_str) = req.crashes_before {
        let dt = parse_maintenance_date(before_str).ok_or_else(|| {
            (StatusCode::BAD_REQUEST, "Invalid crashes_before date".to_string())
        })?;
        crashes::table
            .filter(crashes::created_at.lt(dt))
            .count()
            .get_result::<i64>(&mut conn)
            .unwrap_or(0)
    } else {
        0
    };

    let admin_events_count = if let Some(ref before_str) = req.admin_events_before {
        let dt = parse_maintenance_date(before_str).ok_or_else(|| {
            (StatusCode::BAD_REQUEST, "Invalid admin_events_before date".to_string())
        })?;
        admin_events::table
            .filter(admin_events::created_at.lt(dt))
            .filter(admin_events::processed.eq(true))
            .count()
            .get_result::<i64>(&mut conn)
            .unwrap_or(0)
    } else {
        0
    };

    let coverage_snapshots_count = if let Some(ref before_str) = req.coverage_before {
        let dt = parse_maintenance_date(before_str).ok_or_else(|| {
            (StatusCode::BAD_REQUEST, "Invalid coverage_before date".to_string())
        })?;
        coverage_snapshots::table
            .filter(coverage_snapshots::captured_at.lt(dt))
            .count()
            .get_result::<i64>(&mut conn)
            .unwrap_or(0)
    } else {
        0
    };

    Ok(Json(MaintenancePreviewResponse {
        crashes_count,
        admin_events_count,
        coverage_snapshots_count,
    }))
}

const BATCH_SIZE: i64 = 500;

async fn maintenance_delete(
    State(pool): State<DbPool>,
    Json(req): Json<MaintenanceRequest>,
) -> Result<Json<MaintenancePreviewResponse>, (StatusCode, String)> {
    let mut conn = pool.get().map_err(|e| {
        error!("Failed to get DB connection: {e}");
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "Database unavailable".to_string(),
        )
    })?;

    let mut total_crashes: i64 = 0;
    if let Some(ref before_str) = req.crashes_before {
        let dt = parse_maintenance_date(before_str).ok_or_else(|| {
            (StatusCode::BAD_REQUEST, "Invalid crashes_before date".to_string())
        })?;
        loop {
            let ids: Vec<i32> = crashes::table
                .filter(crashes::created_at.lt(dt))
                .select(crashes::id)
                .limit(BATCH_SIZE)
                .get_results(&mut conn)
                .unwrap_or_default();
            if ids.is_empty() {
                break;
            }
            let deleted = diesel::delete(crashes::table.filter(crashes::id.eq_any(&ids)))
                .execute(&mut conn)
                .unwrap_or(0);
            total_crashes += deleted as i64;
        }
    }

    let mut total_admin_events: i64 = 0;
    if let Some(ref before_str) = req.admin_events_before {
        let dt = parse_maintenance_date(before_str).ok_or_else(|| {
            (StatusCode::BAD_REQUEST, "Invalid admin_events_before date".to_string())
        })?;
        loop {
            let ids: Vec<i32> = admin_events::table
                .filter(admin_events::created_at.lt(dt))
                .filter(admin_events::processed.eq(true))
                .select(admin_events::id)
                .limit(BATCH_SIZE)
                .get_results(&mut conn)
                .unwrap_or_default();
            if ids.is_empty() {
                break;
            }
            let deleted =
                diesel::delete(admin_events::table.filter(admin_events::id.eq_any(&ids)))
                    .execute(&mut conn)
                    .unwrap_or(0);
            total_admin_events += deleted as i64;
        }
    }

    let mut total_coverage: i64 = 0;
    if let Some(ref before_str) = req.coverage_before {
        let dt = parse_maintenance_date(before_str).ok_or_else(|| {
            (StatusCode::BAD_REQUEST, "Invalid coverage_before date".to_string())
        })?;
        loop {
            let ids: Vec<i32> = coverage_snapshots::table
                .filter(coverage_snapshots::captured_at.lt(dt))
                .select(coverage_snapshots::id)
                .limit(BATCH_SIZE)
                .get_results(&mut conn)
                .unwrap_or_default();
            if ids.is_empty() {
                break;
            }
            // Metrics are cascade-deleted
            let deleted = diesel::delete(
                coverage_snapshots::table.filter(coverage_snapshots::id.eq_any(&ids)),
            )
            .execute(&mut conn)
            .unwrap_or(0);
            total_coverage += deleted as i64;
        }
    }

    info!(
        "Maintenance complete: deleted {} crashes, {} admin_events, {} coverage_snapshots",
        total_crashes, total_admin_events, total_coverage
    );

    Ok(Json(MaintenancePreviewResponse {
        crashes_count: total_crashes,
        admin_events_count: total_admin_events,
        coverage_snapshots_count: total_coverage,
    }))
}
