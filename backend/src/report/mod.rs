// SPDX-FileCopyrightText: © 2026 LEDGER SAS
// SPDX-License-Identifier: MIT

//! Fuzzer status heartbeats and the aggregated report built from them.
//!
//! Schedulers push the state of their fuzzers to `POST /fuzzer_status` on every
//! loop iteration. `GET /report` then joins that live state with the coverage
//! snapshots and the crashes already stored by the other endpoints, and
//! `GET /report/html` renders the same data as a standalone page.

#[cfg(test)]
mod tests;

mod builder;
mod heartbeat;
pub(crate) mod html;
pub(crate) mod types;

use chrono::NaiveDateTime;
use diesel::prelude::*;
use log::error;

pub use builder::{get_report, get_report_html};
pub use heartbeat::report_fuzzer_status;

/// Read the current time from the database so that every timestamp comparison
/// uses the same clock as the `NOW()` defaults used by the other tables.
pub(crate) fn db_now(conn: &mut PgConnection) -> NaiveDateTime {
    diesel::select(diesel::dsl::now)
        .get_result::<NaiveDateTime>(conn)
        .unwrap_or_else(|e| {
            error!("Failed to read database time, falling back to local clock: {e}");
            chrono::Local::now().naive_local()
        })
}
