// SPDX-FileCopyrightText: © 2026 LEDGER SAS
// SPDX-License-Identifier: MIT

use std::collections::HashMap;
use std::path::Path;
use std::{fs};

use axum::{
    extract::{Query, State},
    http::StatusCode,
    Json,
};
use chrono::NaiveDateTime;
use diesel::prelude::*;
use log::{error, info};
use serde::{Deserialize, Serialize};

use crate::schema::crashes;
use crate::DbPool;

#[derive(Deserialize, Serialize, Insertable, Queryable, Clone)]
#[diesel(table_name = crashes)]
pub(crate) struct AddCrashReport {
    report: Vec<u8>,
    input: Vec<u8>,
    filename: String,
    program: Vec<u8>,
    scheduler_name: String,
}

#[derive(Deserialize, Serialize, Queryable, Selectable)]
#[diesel(table_name = crashes)]
pub(crate) struct CrashReportWithId {
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

#[derive(Serialize)]
pub(crate) struct CrashId {
    id: i32,
    created_at: NaiveDateTime,
}

/// A crash without its `report` / `input` / `program` blobs.
///
/// The listing endpoints return this instead of the full record: a single
/// crash carries a program binary of up to 100 MB, so returning whole rows
/// made the response size grow with the size of the table.
#[derive(Serialize, Queryable, Selectable)]
#[diesel(table_name = crashes)]
pub(crate) struct CrashMetadata {
    id: i32,
    filename: String,
    scheduler_name: String,
    created_at: NaiveDateTime,
}

/// Crashes returned by a listing when the caller does not ask for a page size.
const DEFAULT_CRASH_LIMIT: i64 = 100;

/// Hard cap on a crash listing.
const MAX_CRASH_LIMIT: i64 = 1000;

pub(crate) async fn get_crashes(
    State(pool): State<DbPool>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<Vec<CrashMetadata>>, StatusCode> {
    let mut conn = pool.get().map_err(|e| {
        error!("Failed to get DB connection from pool: {e}");
        StatusCode::SERVICE_UNAVAILABLE
    })?;

    let limit: i64 = params
        .get("limit")
        .and_then(|l| l.parse::<i64>().ok())
        .unwrap_or(DEFAULT_CRASH_LIMIT)
        .clamp(1, MAX_CRASH_LIMIT);

    let crashes = crashes::table
        .select(CrashMetadata::as_select())
        .order(crashes::created_at.desc())
        .limit(limit)
        .get_results::<CrashMetadata>(&mut conn)
        .map_err(|e| {
            error!("Database query failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(Json(crashes))
}

pub(crate) async fn get_last_crash(
    State(pool): State<DbPool>,
) -> Result<Json<CrashReportWithId>, StatusCode> {
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

pub(crate) async fn get_last_n_crashes(
    State(pool): State<DbPool>,
    axum::extract::Path(number): axum::extract::Path<i64>,
) -> Result<Json<Vec<CrashMetadata>>, StatusCode> {
    let mut conn = pool.get().map_err(|e| {
        error!("Failed to get DB connection from pool: {e}");
        StatusCode::SERVICE_UNAVAILABLE
    })?;

    // Postgres rejects a negative LIMIT, and an unbounded one would walk the
    // whole table, so the requested count is clamped rather than trusted.
    let limit = number.clamp(1, MAX_CRASH_LIMIT);
    info!("Getting last {limit} crashes (requested {number})");

    let crashes = crashes::table
        .select(CrashMetadata::as_select())
        .order(crashes::created_at.desc())
        .limit(limit)
        .get_results::<CrashMetadata>(&mut conn)
        .map_err(|e| {
            error!("Database query failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(Json(crashes))
}

pub(crate) async fn get_crashes_count(
    State(pool): State<DbPool>,
) -> Result<Json<i64>, StatusCode> {
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

pub(crate) async fn upload_crash(
    State(pool): State<DbPool>,
    Json(report): Json<AddCrashReport>,
) -> Result<StatusCode, (StatusCode, String)> {
    const MAX_PROGRAM_SIZE: usize = 100 * 1024 * 1024;
    const MAX_INPUT_SIZE: usize = 10 * 1024 * 1024;
    const MAX_REPORT_SIZE: usize = 1024 * 1024;

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

    let mut conn = pool.get().map_err(|e| {
        error!("Failed to get DB connection: {e}");
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "Database unavailable".to_string(),
        )
    })?;

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
pub(crate) async fn get_crash_by_id(
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

pub(crate) async fn collect_crashes(
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

    fs::create_dir_all(&dir_name).map_err(|e| {
        error!("Failed to create directory {dir_name}: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let program_path = Path::new(&dir_name).join(&s_filename);
    let report_path = Path::new(&dir_name).join("crash_info.txt");
    let input_path = Path::new(&dir_name).join("crash_input.txt");

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

pub(crate) async fn get_crash_ids(
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
