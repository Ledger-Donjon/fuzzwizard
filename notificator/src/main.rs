// SPDX-FileCopyrightText: © 2026 LEDGER SAS
// SPDX-License-Identifier: MIT

mod config;
mod providers;
mod traits;

use anyhow::{Context, Result};
use chrono::NaiveDateTime;
use fs2::FileExt;
use ftail::Ftail;
use log::LevelFilter;
use log::{error, info};
use reqwest::header::{HeaderMap, CONTENT_TYPE};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use config::RootConfig;
use providers::{file::FileNotificationProvider, slack::SlackNotificationProvider};
use traits::{AdminEvent, CrashReport, NotificationProvider};

#[derive(Serialize, Deserialize)]
struct CrashId {
    id: i32,
    created_at: NaiveDateTime,
}

/// Fetch the last crash from the backend
fn fetch_last_crash(client: &reqwest::blocking::Client) -> Result<CrashReport> {
    let url = Url::parse("http://127.0.0.1:3000/crashes/last").context("Invalid backend URL")?;

    let mut headers_backend = HeaderMap::new();
    headers_backend.insert(
        CONTENT_TYPE,
        "application/json"
            .parse()
            .context("Failed to parse content type header")?,
    );

    let response = client
        .get(url)
        .headers(headers_backend)
        .send()
        .context("Failed to send request to backend")?;

    if !response.status().is_success() {
        anyhow::bail!("Backend returned error: {}", response.status());
    }

    let crash: CrashReport = response
        .json()
        .context("Failed to parse response as JSON")?;

    Ok(crash)
}

/// Load the last notified crash date from the state file, or initialize with the last crash from DB
fn load_or_init_last_crash_date(
    state_file_path: &str,
    client: &reqwest::blocking::Client,
) -> Result<NaiveDateTime> {
    // Create parent directory if necessary
    if let Some(parent) = PathBuf::from(state_file_path).parent() {
        fs::create_dir_all(parent).context("Failed to create state directory")?;
    }

    // Check if file exists and is not empty
    let file_exists = PathBuf::from(state_file_path).exists();
    let mut contents = String::new();

    if file_exists {
        let mut file = OpenOptions::new()
            .read(true)
            .open(state_file_path)
            .context("Failed to open state file")?;

        // Lock file (exclusive)
        file.lock_exclusive().context("Failed to lock state file")?;

        // Read contents
        file.read_to_string(&mut contents)
            .context("Failed to read state file")?;
    }

    // Parse date or initialize from last crash
    let date = if contents.trim().is_empty() {
        // State file doesn't exist or is empty, fetch last crash from DB
        info!("State file is empty or doesn't exist, fetching last crash from database");
        match fetch_last_crash(client) {
            Ok(crash) => {
                info!("Last crash ID: {} (date: {})", crash.id, crash.created_at);
                crash.created_at
            }
            Err(e) => {
                // If we can't fetch the last crash, use a default date
                error!("Failed to fetch last crash: {}, using default date", e);
                chrono::NaiveDate::from_ymd_opt(2025, 1, 1)
                    .and_then(|d| d.and_hms_opt(0, 0, 0))
                    .context("Failed to create default date")?
            }
        }
    } else {
        // Parse date from state file
        NaiveDateTime::parse_from_str(contents.trim(), "%Y-%m-%d %H:%M:%S%.f")
            .context("Failed to parse date from state file")?
    };

    // If we initialized from last crash, save it to the state file
    if contents.trim().is_empty() {
        if let Err(e) = save_last_crash_date(date, state_file_path) {
            error!("Failed to save initial crash date: {}", e);
        }
    }

    Ok(date)
}

/// Save the last notified crash date to the state file
fn save_last_crash_date(date: NaiveDateTime, state_file_path: &str) -> Result<()> {
    // Create parent directory if necessary
    if let Some(parent) = PathBuf::from(state_file_path).parent() {
        fs::create_dir_all(parent).context("Failed to create state directory")?;
    }

    let mut file = OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(state_file_path)
        .context("Failed to open state file for writing")?;

    // Lock
    file.lock_exclusive().context("Failed to lock state file")?;

    // Write
    let date_str = date.format("%Y-%m-%d %H:%M:%S%.f").to_string();
    file.write_all(date_str.as_bytes())
        .context("Failed to write state")?;

    // Lock is automatically released
    Ok(())
}

/// Fetch crash by ID
fn fetch_crash_by_id(client: &reqwest::blocking::Client, crash_id: i32) -> Result<CrashReport> {
    let url = Url::parse(&format!("http://127.0.0.1:3000/crashes/{}", crash_id))
        .context("Invalid backend URL")?;

    let mut headers_backend = HeaderMap::new();
    headers_backend.insert(
        CONTENT_TYPE,
        "application/json"
            .parse()
            .context("Failed to parse content type header")?,
    );

    let response = client
        .get(url)
        .headers(headers_backend)
        .send()
        .context("Failed to send request to backend")?;

    if !response.status().is_success() {
        anyhow::bail!("Backend returned error: {}", response.status());
    }

    let crash: CrashReport = response
        .json()
        .context("Failed to parse response as JSON")?;

    Ok(crash)
}

/// Fetch crash IDs since a given date
fn fetch_crash_ids_since(
    client: &reqwest::blocking::Client,
    after: NaiveDateTime,
) -> Result<Vec<CrashId>> {
    let mut url = Url::parse("http://127.0.0.1:3000/crashes/ids").context("Invalid backend URL")?;

    url.query_pairs_mut()
        .append_pair("after", &after.format("%Y-%m-%d %H:%M:%S%.f").to_string());

    let mut headers_backend = HeaderMap::new();
    headers_backend.insert(
        CONTENT_TYPE,
        "application/json"
            .parse()
            .context("Failed to parse content type header")?,
    );

    let response = client
        .get(url)
        .headers(headers_backend)
        .send()
        .context("Failed to send request to backend")?;

    if !response.status().is_success() {
        anyhow::bail!("Backend returned error: {}", response.status());
    }

    let crash_ids: Vec<CrashId> = response
        .json()
        .context("Failed to parse response as JSON")?;

    Ok(crash_ids)
}

/// Fetch unprocessed admin events from the backend
fn fetch_unprocessed_admin_events(client: &reqwest::blocking::Client) -> Result<Vec<AdminEvent>> {
    let url = Url::parse("http://127.0.0.1:3000/admin_events/unprocessed")
        .context("Invalid backend URL")?;

    let mut headers_backend = HeaderMap::new();
    headers_backend.insert(
        CONTENT_TYPE,
        "application/json"
            .parse()
            .context("Failed to parse content type header")?,
    );

    let response = client
        .get(url)
        .headers(headers_backend)
        .send()
        .context("Failed to send request to backend")?;

    if !response.status().is_success() {
        anyhow::bail!("Backend returned error: {}", response.status());
    }

    let events: Vec<AdminEvent> = response
        .json()
        .context("Failed to parse response as JSON")?;

    Ok(events)
}

/// Mark an admin event as processed
fn mark_admin_event_processed(client: &reqwest::blocking::Client, event_id: i32) -> Result<()> {
    let url = Url::parse(&format!(
        "http://127.0.0.1:3000/admin_events/{}/mark_processed",
        event_id
    ))
    .context("Invalid backend URL")?;

    let response = client
        .post(url)
        .send()
        .context("Failed to send request to backend")?;

    if !response.status().is_success() {
        anyhow::bail!("Backend returned error: {}", response.status());
    }

    Ok(())
}

fn main() -> Result<()> {
    // Read config path from arguments or use default value
    let config_path = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("config.json"));

    // Charger la configuration
    let config = RootConfig::from_file(&config_path).context("Failed to load configuration")?;

    // Configure ftail to write to a file using logdir from config
    let log_file_path = PathBuf::from(&config.logdir).join("notificator.log");
    // Ensure logs directory exists
    if let Err(e) = std::fs::create_dir_all(&config.logdir) {
        eprintln!("Failed to create log directory '{}': {}", config.logdir, e);
        std::process::exit(1);
    }

    Ftail::new()
        .single_file(&log_file_path, true, LevelFilter::Info)
        .init()
        .context("Failed to initialize logger")?;

    info!("Notificator started with config from: {:?}", config_path);
    info!("Using logdir: {}", config.logdir);
    info!(
        "Using state file: {}",
        config.notifications.notificator_state_file
    );

    // Initialize notification providers
    let mut providers: Vec<Arc<dyn NotificationProvider>> = Vec::new();

    // Add Slack if enabled
    if config.notifications.providers.slack.enabled {
        let slack_provider = Arc::new(SlackNotificationProvider::new(
            config.notifications.providers.slack.token.clone(),
            config.notifications.providers.slack.channel.clone(),
            true,
        ));
        providers.push(slack_provider);
        info!("Slack notification provider enabled");
    } else {
        info!("Slack notification provider disabled");
    }

    // Add File provider if enabled
    if config.notifications.providers.file.enabled {
        let file_provider = Arc::new(FileNotificationProvider::new(
            config.notifications.providers.file.output_file.clone(),
            true,
        ));
        providers.push(file_provider);
        info!(
            "File notification provider enabled (output: {})",
            config.notifications.providers.file.output_file
        );
    } else {
        info!("File notification provider disabled");
    }

    if providers.is_empty() {
        info!("No notification providers enabled, but service will continue running");
    }

    let state_file_path = config.notifications.notificator_state_file.clone();
    let client = reqwest::blocking::Client::new();

    // Load or initialize the last crash date
    let mut last_notified_crash_date = load_or_init_last_crash_date(&state_file_path, &client)
        .context("Failed to load or initialize last crash date")?;

    info!("Starting from crash date: {}", last_notified_crash_date);

    loop {
        // Fetch crash IDs since the last notified crash date
        match fetch_crash_ids_since(&client, last_notified_crash_date) {
            Ok(crash_ids) => {
                if !crash_ids.is_empty() {
                    info!("Found {} new crashes to process", crash_ids.len());

                    let mut latest_date = last_notified_crash_date;

                    for crash_id_info in &crash_ids {
                        // Fetch the full crash data
                        match fetch_crash_by_id(&client, crash_id_info.id) {
                            Ok(crash) => {
                                // Send notifications via all enabled providers
                                for provider in &providers {
                                    if provider.is_enabled() {
                                        if let Err(e) = provider.send_crash_notification(&crash) {
                                            error!(
                                                "Failed to send notification for crash {}: {}",
                                                crash.id, e
                                            );
                                            // Continue with other providers
                                        }
                                    }
                                }

                                // Update latest date
                                if crash.created_at > latest_date {
                                    latest_date = crash.created_at;
                                }
                            }
                            Err(e) => {
                                error!("Failed to fetch crash {}: {}", crash_id_info.id, e);
                                // Continue with next crash
                            }
                        }
                    }

                    // Update last notified crash date if we processed any crashes
                    if latest_date > last_notified_crash_date {
                        last_notified_crash_date = latest_date;
                        if let Err(e) = save_last_crash_date(latest_date, &state_file_path) {
                            error!("Failed to save last crash date: {}", e);
                            // Continue anyway
                        } else {
                            info!("Updated last notified crash date to: {}", latest_date);
                        }
                    }
                }
            }
            Err(e) => {
                error!("Failed to fetch crash IDs: {}", e);
                // Continue loop
            }
        }

        // Fetch unprocessed admin events
        match fetch_unprocessed_admin_events(&client) {
            Ok(events) => {
                if !events.is_empty() {
                    info!("Received {} unprocessed admin events", events.len());

                    for event in &events {
                        // Send notifications via all enabled providers
                        for provider in &providers {
                            if provider.is_enabled() {
                                if let Err(e) = provider.send_admin_notification(event) {
                                    error!(
                                        "Failed to send notification for admin event {}: {}",
                                        event.id, e
                                    );
                                    // Continue with other providers
                                }
                            }
                        }

                        // Mark event as processed
                        if let Err(e) = mark_admin_event_processed(&client, event.id) {
                            error!(
                                "Failed to mark admin event {} as processed: {}",
                                event.id, e
                            );
                            // Continue anyway
                        }
                    }
                }
            }
            Err(e) => {
                error!("Failed to fetch admin events: {}", e);
                // Continue loop
            }
        }

        std::thread::sleep(Duration::from_secs(10));
    }
}
