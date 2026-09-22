// SPDX-FileCopyrightText: © 2026 LEDGER SAS
// SPDX-License-Identifier: MIT

mod admin_events;
mod coverage_snapshots;
mod crashes;
mod maintenance;
pub(crate) mod report;
mod schema;

use std::env;
use std::time::Duration;

use axum::{
    extract::DefaultBodyLimit,
    routing::{get, post},
    Router,
};
use diesel::r2d2::{self, ConnectionManager, Pool};
use diesel::PgConnection;
use ftail::Ftail;
use log::{error, info, LevelFilter};
use tower_http::catch_panic::CatchPanicLayer;

pub(crate) type DbPool = Pool<ConnectionManager<PgConnection>>;

#[tokio::main]
async fn main() {
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

    let log_file_path = "./logs/backend.log";
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
        .route("/crashes", get(crashes::get_crashes))
        .route("/crashes/last", get(crashes::get_last_crash))
        .route("/crashes/last/{number}", get(crashes::get_last_n_crashes))
        .route("/crashes/count", get(crashes::get_crashes_count))
        .route("/crashes/ids", get(crashes::get_crash_ids))
        .route("/crashes", post(crashes::upload_crash))
        .route("/crashes/{id}", get(crashes::get_crash_by_id))
        .route("/collect_crashes", post(crashes::collect_crashes))
        .route("/admin_events", post(admin_events::create_admin_event))
        .route(
            "/admin_events/unprocessed",
            get(admin_events::get_unprocessed_admin_events),
        )
        .route(
            "/admin_events/{id}/mark_processed",
            post(admin_events::mark_admin_event_processed),
        )
        .route(
            "/coverage_snapshots",
            post(coverage_snapshots::create_coverage_snapshot)
                .get(coverage_snapshots::get_coverage_snapshots),
        )
        .route(
            "/coverage_snapshots/latest",
            get(coverage_snapshots::get_latest_coverage_snapshot),
        )
        .route("/fuzzer_status", post(report::report_fuzzer_status))
        .route("/report", get(report::get_report))
        .route("/report/html", get(report::get_report_html))
        .route("/maintenance/preview", post(maintenance::maintenance_preview))
        .route("/maintenance/delete", post(maintenance::maintenance_delete))
        .layer(DefaultBodyLimit::max(110 * 1024 * 1024))
        // Outermost layer, so a panic in any handler answers 500 and takes down
        // the single request instead of the connection serving it.
        .layer(CatchPanicLayer::new())
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
