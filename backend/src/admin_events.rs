// SPDX-FileCopyrightText: © 2026 LEDGER SAS
// SPDX-License-Identifier: MIT

use axum::{
    extract::State,
    http::StatusCode,
    Json,
};
use chrono::NaiveDateTime;
use diesel::prelude::*;
use log::{error, info};
use serde::{Deserialize, Serialize};

use crate::schema::admin_events;
use crate::DbPool;

#[derive(Deserialize, Serialize)]
pub(crate) struct CreateAdminEvent {
    event_type: String,
    message: String,
    metadata: Option<serde_json::Value>,
}

#[derive(Deserialize, Serialize, Queryable, Selectable)]
#[diesel(table_name = admin_events)]
pub(crate) struct AdminEvent {
    id: i32,
    event_type: String,
    message: String,
    metadata: Option<String>,
    created_at: NaiveDateTime,
    processed: bool,
}

pub(crate) async fn create_admin_event(
    State(pool): State<DbPool>,
    Json(event): Json<CreateAdminEvent>,
) -> Result<StatusCode, (StatusCode, String)> {
    const MAX_EVENT_TYPE_SIZE: usize = 256;
    const MAX_MESSAGE_SIZE: usize = 64 * 1024;
    const MAX_METADATA_SIZE: usize = 1024 * 1024;

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

pub(crate) async fn get_unprocessed_admin_events(
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

pub(crate) async fn mark_admin_event_processed(
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
