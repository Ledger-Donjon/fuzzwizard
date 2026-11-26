// SPDX-FileCopyrightText: © 2026 LEDGER SAS
// SPDX-License-Identifier: MIT

//! `POST /fuzzer_status` — scheduler heartbeat.

use axum::{extract::State, http::StatusCode, Json};
use chrono::NaiveDateTime;
use diesel::prelude::*;
use log::error;

use crate::schema::{fuzzer_runtime, fuzzer_status};
use crate::DbPool;

use super::db_now;
use super::types::{
    runtime_delta, FuzzerStatusHeartbeat, RuntimeRow, MAX_FUZZERS_PER_HEARTBEAT,
    MAX_SCHEDULER_INSTANCE_LEN, VALID_FUZZER_STATUSES,
};

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
    if payload
        .scheduler_instance
        .as_ref()
        .is_some_and(|i| i.is_empty() || i.len() > MAX_SCHEDULER_INSTANCE_LEN)
    {
        return Err((
            StatusCode::BAD_REQUEST,
            "Invalid scheduler_instance".to_string(),
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
            let started_at = f.started_at(now);
            diesel::insert_into(fuzzer_status::table)
                .values((
                    fuzzer_status::scheduler_name.eq(&payload.scheduler_name),
                    fuzzer_status::fuzzer_sha.eq(&f.sha),
                    fuzzer_status::fuzzer_path.eq(&f.path),
                    fuzzer_status::status.eq(&f.status),
                    fuzzer_status::started_at.eq(started_at),
                    fuzzer_status::last_seen_at.eq(now),
                ))
                .on_conflict((fuzzer_status::scheduler_name, fuzzer_status::fuzzer_sha))
                .do_update()
                .set((
                    fuzzer_status::fuzzer_path.eq(&f.path),
                    fuzzer_status::status.eq(&f.status),
                    fuzzer_status::started_at.eq(started_at),
                    fuzzer_status::last_seen_at.eq(now),
                ))
                .execute(conn)?;

            if let (Some(instance), Some(reported)) =
                (&payload.scheduler_instance, f.runtime_seconds)
            {
                record_runtime(
                    conn,
                    &payload.scheduler_name,
                    &f.sha,
                    instance,
                    reported,
                    now,
                )?;
            }
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

/// Add the growth of the runtime counter of a binary to its total.
fn record_runtime(
    conn: &mut PgConnection,
    scheduler_name: &str,
    sha: &str,
    instance: &str,
    reported: u64,
    now: NaiveDateTime,
) -> QueryResult<()> {
    let reported = i64::try_from(reported).unwrap_or(i64::MAX);
    let previous = fuzzer_runtime::table
        .filter(fuzzer_runtime::scheduler_name.eq(scheduler_name))
        .filter(fuzzer_runtime::fuzzer_sha.eq(sha))
        .select(RuntimeRow::as_select())
        .for_update()
        .first::<RuntimeRow>(conn)
        .optional()?;
    let delta = runtime_delta(previous.as_ref(), instance, reported);

    diesel::insert_into(fuzzer_runtime::table)
        .values((
            fuzzer_runtime::scheduler_name.eq(scheduler_name),
            fuzzer_runtime::fuzzer_sha.eq(sha),
            fuzzer_runtime::total_runtime_seconds.eq(delta),
            fuzzer_runtime::last_reported_seconds.eq(reported),
            fuzzer_runtime::scheduler_instance.eq(instance),
            fuzzer_runtime::updated_at.eq(now),
        ))
        .on_conflict((fuzzer_runtime::scheduler_name, fuzzer_runtime::fuzzer_sha))
        .do_update()
        .set((
            fuzzer_runtime::total_runtime_seconds.eq(fuzzer_runtime::total_runtime_seconds + delta),
            fuzzer_runtime::last_reported_seconds.eq(reported),
            fuzzer_runtime::scheduler_instance.eq(instance),
            fuzzer_runtime::updated_at.eq(now),
        ))
        .execute(conn)?;
    Ok(())
}
