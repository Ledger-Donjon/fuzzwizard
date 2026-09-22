// SPDX-FileCopyrightText: © 2026 LEDGER SAS
// SPDX-License-Identifier: MIT

//! `POST /fuzzer_status` — scheduler heartbeat.

use axum::{
    extract::State,
    http::StatusCode,
    Json,
};
use diesel::prelude::*;
use log::error;

use crate::schema::fuzzer_status;
use crate::DbPool;

use super::db_now;
use super::types::{FuzzerStatusHeartbeat, MAX_FUZZERS_PER_HEARTBEAT, VALID_FUZZER_STATUSES};

pub async fn report_fuzzer_status(
    State(pool): State<DbPool>,
    Json(payload): Json<FuzzerStatusHeartbeat>,
) -> Result<StatusCode, (StatusCode, String)> {
    if payload.scheduler_name.is_empty() || payload.scheduler_name.len() > 256 {
        return Err((
            StatusCode::BAD_REQUEST,
            "Invalid scheduler_name".to_string(),
        ));
    }
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
        if f.path.is_empty() || f.path.len() > 4096 {
            return Err((StatusCode::BAD_REQUEST, "Invalid fuzzer path".to_string()));
        }
        if !VALID_FUZZER_STATUSES.contains(&f.status.as_str()) {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("Invalid status '{}' for fuzzer {}", f.status, f.sha),
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

    let now = db_now(&mut conn);
    let seen_shas: Vec<&str> = payload.fuzzers.iter().map(|f| f.sha.as_str()).collect();

    conn.transaction::<_, diesel::result::Error, _>(|conn| {
        for f in &payload.fuzzers {
            diesel::insert_into(fuzzer_status::table)
                .values((
                    fuzzer_status::scheduler_name.eq(&payload.scheduler_name),
                    fuzzer_status::fuzzer_sha.eq(&f.sha),
                    fuzzer_status::fuzzer_path.eq(&f.path),
                    fuzzer_status::status.eq(&f.status),
                    fuzzer_status::started_at.eq(f.started_at),
                    fuzzer_status::last_seen_at.eq(now),
                ))
                .on_conflict((fuzzer_status::scheduler_name, fuzzer_status::fuzzer_sha))
                .do_update()
                .set((
                    fuzzer_status::fuzzer_path.eq(&f.path),
                    fuzzer_status::status.eq(&f.status),
                    fuzzer_status::started_at.eq(f.started_at),
                    fuzzer_status::last_seen_at.eq(now),
                ))
                .execute(conn)?;
        }

        diesel::delete(
            fuzzer_status::table
                .filter(fuzzer_status::scheduler_name.eq(&payload.scheduler_name))
                .filter(fuzzer_status::fuzzer_sha.ne_all(&seen_shas)),
        )
        .execute(conn)?;

        Ok(())
    })
    .map_err(|e| {
        error!("Failed to record fuzzer status heartbeat: {e}");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Database error".to_string(),
        )
    })?;

    Ok(StatusCode::NO_CONTENT)
}
