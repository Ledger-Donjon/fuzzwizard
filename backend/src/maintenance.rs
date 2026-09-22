// SPDX-FileCopyrightText: © 2026 LEDGER SAS
// SPDX-License-Identifier: MIT

use axum::{
    extract::State,
    http::StatusCode,
    Json,
};
use chrono::NaiveDateTime;
use diesel::dsl::{exists, not};
use diesel::prelude::*;
use log::{error, info};
use serde::{Deserialize, Serialize};

use crate::schema::{admin_events, coverage_files, coverage_paths, coverage_snapshots, crashes};
use crate::DbPool;

#[derive(Deserialize)]
pub(crate) struct MaintenanceRequest {
    #[serde(default)]
    crashes_before: Option<String>,
    #[serde(default)]
    admin_events_before: Option<String>,
    #[serde(default)]
    coverage_before: Option<String>,
    #[serde(default)]
    coverage_files_before: Option<String>,
}

#[derive(Serialize)]
pub(crate) struct MaintenancePreviewResponse {
    crashes_count: i64,
    admin_events_count: i64,
    coverage_snapshots_count: i64,
    coverage_files_count: i64,
}

fn prune_orphan_paths(conn: &mut PgConnection) -> i64 {
    let orphans = coverage_paths::table.filter(not(exists(
        coverage_files::table.filter(coverage_files::path_id.eq(coverage_paths::id)),
    )));

    match diesel::delete(orphans).execute(conn) {
        Ok(deleted) => deleted as i64,
        Err(e) => {
            error!("Failed to prune orphan coverage paths: {e}");
            0
        }
    }
}

fn parse_maintenance_date(s: &str) -> Option<NaiveDateTime> {
    chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S%.f")
        .or_else(|_| {
            chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d")
                .map(|d| d.and_hms_opt(0, 0, 0).unwrap())
        })
        .ok()
}

struct MaintenanceCutoffs {
    crashes: Option<NaiveDateTime>,
    admin_events: Option<NaiveDateTime>,
    coverage: Option<NaiveDateTime>,
    coverage_files: Option<NaiveDateTime>,
}

impl MaintenanceCutoffs {
    fn parse(req: &MaintenanceRequest) -> Result<Self, (StatusCode, String)> {
        Ok(Self {
            crashes: parse_cutoff(&req.crashes_before, "crashes_before")?,
            admin_events: parse_cutoff(&req.admin_events_before, "admin_events_before")?,
            coverage: parse_cutoff(&req.coverage_before, "coverage_before")?,
            coverage_files: parse_cutoff(&req.coverage_files_before, "coverage_files_before")?,
        })
    }
}

fn parse_cutoff(
    value: &Option<String>,
    field: &str,
) -> Result<Option<NaiveDateTime>, (StatusCode, String)> {
    match value {
        None => Ok(None),
        Some(raw) => parse_maintenance_date(raw)
            .map(Some)
            .ok_or_else(|| (StatusCode::BAD_REQUEST, format!("Invalid {field} date"))),
    }
}

fn maintenance_db_error(e: diesel::result::Error) -> (StatusCode, String) {
    error!("Maintenance query failed: {e}");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        "Database query failed".to_string(),
    )
}

fn delete_in_batches(
    conn: &mut PgConnection,
    mut next_batch: impl FnMut(&mut PgConnection) -> QueryResult<Vec<i32>>,
    mut delete_batch: impl FnMut(&mut PgConnection, &[i32]) -> QueryResult<usize>,
) -> Result<i64, (StatusCode, String)> {
    let mut total = 0;
    loop {
        let ids = next_batch(conn).map_err(maintenance_db_error)?;
        if ids.is_empty() {
            return Ok(total);
        }
        match delete_batch(conn, &ids).map_err(maintenance_db_error)? {
            0 => return Ok(total),
            deleted => total += deleted as i64,
        }
    }
}

pub(crate) async fn maintenance_preview(
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

    let cutoffs = MaintenanceCutoffs::parse(&req)?;

    let crashes_count = match cutoffs.crashes {
        Some(dt) => crashes::table
            .filter(crashes::created_at.lt(dt))
            .count()
            .get_result::<i64>(&mut conn)
            .map_err(maintenance_db_error)?,
        None => 0,
    };

    let admin_events_count = match cutoffs.admin_events {
        Some(dt) => admin_events::table
            .filter(admin_events::created_at.lt(dt))
            .filter(admin_events::processed.eq(true))
            .count()
            .get_result::<i64>(&mut conn)
            .map_err(maintenance_db_error)?,
        None => 0,
    };

    let coverage_snapshots_count = match cutoffs.coverage {
        Some(dt) => coverage_snapshots::table
            .filter(coverage_snapshots::captured_at.lt(dt))
            .count()
            .get_result::<i64>(&mut conn)
            .map_err(maintenance_db_error)?,
        None => 0,
    };

    let coverage_files_count = match cutoffs.coverage_files {
        Some(dt) => coverage_files::table
            .inner_join(coverage_snapshots::table)
            .filter(coverage_snapshots::captured_at.lt(dt))
            .count()
            .get_result::<i64>(&mut conn)
            .map_err(maintenance_db_error)?,
        None => 0,
    };

    Ok(Json(MaintenancePreviewResponse {
        crashes_count,
        admin_events_count,
        coverage_snapshots_count,
        coverage_files_count,
    }))
}

const BATCH_SIZE: i64 = 500;

pub(crate) async fn maintenance_delete(
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

    let cutoffs = MaintenanceCutoffs::parse(&req)?;

    let mut total_crashes: i64 = 0;
    if let Some(dt) = cutoffs.crashes {
        total_crashes = delete_in_batches(
            &mut conn,
            |c| {
                crashes::table
                    .filter(crashes::created_at.lt(dt))
                    .select(crashes::id)
                    .limit(BATCH_SIZE)
                    .get_results(c)
            },
            |c, ids| diesel::delete(crashes::table.filter(crashes::id.eq_any(ids))).execute(c),
        )?;
    }

    let mut total_admin_events: i64 = 0;
    if let Some(dt) = cutoffs.admin_events {
        total_admin_events = delete_in_batches(
            &mut conn,
            |c| {
                admin_events::table
                    .filter(admin_events::created_at.lt(dt))
                    .filter(admin_events::processed.eq(true))
                    .select(admin_events::id)
                    .limit(BATCH_SIZE)
                    .get_results(c)
            },
            |c, ids| {
                diesel::delete(admin_events::table.filter(admin_events::id.eq_any(ids))).execute(c)
            },
        )?;
    }

    let mut total_coverage: i64 = 0;
    if let Some(dt) = cutoffs.coverage {
        total_coverage = delete_in_batches(
            &mut conn,
            |c| {
                coverage_snapshots::table
                    .filter(coverage_snapshots::captured_at.lt(dt))
                    .select(coverage_snapshots::id)
                    .limit(BATCH_SIZE)
                    .get_results(c)
            },
            |c, ids| {
                diesel::delete(coverage_snapshots::table.filter(coverage_snapshots::id.eq_any(ids)))
                    .execute(c)
            },
        )?;
    }

    let mut total_coverage_files: i64 = 0;
    if let Some(dt) = cutoffs.coverage_files {
        total_coverage_files = delete_in_batches(
            &mut conn,
            |c| {
                coverage_files::table
                    .inner_join(coverage_snapshots::table)
                    .filter(coverage_snapshots::captured_at.lt(dt))
                    .select(coverage_files::id)
                    .limit(BATCH_SIZE)
                    .get_results(c)
            },
            |c, ids| {
                diesel::delete(coverage_files::table.filter(coverage_files::id.eq_any(ids)))
                    .execute(c)
            },
        )?;
    }

    if total_coverage_files > 0 || total_coverage > 0 {
        let pruned = prune_orphan_paths(&mut conn);
        info!("Pruned {pruned} orphan coverage paths");
    }

    info!(
        "Maintenance complete: deleted {} crashes, {} admin_events, {} coverage_snapshots, {} coverage_files",
        total_crashes, total_admin_events, total_coverage, total_coverage_files
    );

    Ok(Json(MaintenancePreviewResponse {
        crashes_count: total_crashes,
        admin_events_count: total_admin_events,
        coverage_snapshots_count: total_coverage,
        coverage_files_count: total_coverage_files,
    }))
}
