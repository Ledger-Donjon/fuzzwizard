// SPDX-FileCopyrightText: © 2026 LEDGER SAS
// SPDX-License-Identifier: MIT

use chrono::{Months, Utc};
use clap::Parser;
use dialoguer::Confirm;
use serde::{Deserialize, Serialize};

#[derive(Parser)]
#[command(name = "fuzzwizard-cleanup", about = "Interactive maintenance tool for the FuzzWizard backend")]
struct Cli {
    /// Backend URL (e.g. http://127.0.0.1:3000)
    #[arg(long, env = "FUZZWIZARD_BACKEND_URL", default_value = "http://127.0.0.1:3000")]
    backend_url: String,

    /// Delete crashes older than this many months (default: 6)
    #[arg(long, default_value = "6")]
    crashes_months: u32,

    /// Delete processed admin events older than this many days (default: 30)
    #[arg(long, default_value = "30")]
    admin_events_days: i64,

    /// Delete coverage snapshots older than this date (format: YYYY-MM-DD). Omitted by default.
    #[arg(long)]
    coverage_before: Option<String>,

    /// Skip interactive confirmation (dry-run only shows preview)
    #[arg(long)]
    dry_run: bool,
}

#[derive(Serialize)]
struct MaintenanceRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    crashes_before: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    admin_events_before: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    coverage_before: Option<String>,
}

#[derive(Deserialize)]
struct MaintenancePreviewResponse {
    crashes_count: i64,
    admin_events_count: i64,
    coverage_snapshots_count: i64,
}

fn main() {
    let cli = Cli::parse();

    let now = Utc::now().naive_utc();

    let crashes_before = now
        .checked_sub_months(Months::new(cli.crashes_months))
        .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string());

    let admin_events_before = now
        .checked_sub_signed(chrono::Duration::days(cli.admin_events_days))
        .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string());

    let coverage_before = cli.coverage_before.clone();

    let request = MaintenanceRequest {
        crashes_before: crashes_before.clone(),
        admin_events_before: admin_events_before.clone(),
        coverage_before: coverage_before.clone(),
    };

    println!("=== FuzzWizard Cleanup ===\n");
    println!("Backend: {}", cli.backend_url);
    println!(
        "Crashes: delete records older than {} months (before {})",
        cli.crashes_months,
        crashes_before.as_deref().unwrap_or("N/A")
    );
    println!(
        "Admin events: delete processed records older than {} days (before {})",
        cli.admin_events_days,
        admin_events_before.as_deref().unwrap_or("N/A")
    );
    if let Some(ref cov) = coverage_before {
        println!("Coverage snapshots: delete records before {cov}");
    } else {
        println!("Coverage snapshots: not targeted (kept indefinitely)");
    }
    println!();

    let client = reqwest::blocking::Client::new();

    println!("Fetching preview...");
    let preview_url = format!("{}/maintenance/preview", cli.backend_url);
    let resp = match client.post(&preview_url).json(&request).send() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Error contacting backend: {e}");
            std::process::exit(1);
        }
    };

    if !resp.status().is_success() {
        eprintln!(
            "Error: Backend returned status {} - {}",
            resp.status(),
            resp.text().unwrap_or_default()
        );
        std::process::exit(1);
    }

    let preview: MaintenancePreviewResponse = match resp.json() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Error parsing response: {e}");
            std::process::exit(1);
        }
    };

    println!("\n--- Preview ---");
    println!("  Crashes to delete:            {}", preview.crashes_count);
    println!("  Admin events to delete:       {}", preview.admin_events_count);
    println!(
        "  Coverage snapshots to delete: {}",
        preview.coverage_snapshots_count
    );

    let total = preview.crashes_count + preview.admin_events_count + preview.coverage_snapshots_count;
    if total == 0 {
        println!("\nNothing to delete.");
        return;
    }

    if cli.dry_run {
        println!("\n[dry-run] No deletions performed.");
        return;
    }

    println!();
    let confirmed = Confirm::new()
        .with_prompt(format!(
            "Proceed with deleting {total} records? This action is irreversible"
        ))
        .default(false)
        .interact()
        .unwrap_or(false);

    if !confirmed {
        println!("Aborted.");
        return;
    }

    println!("\nDeleting...");
    let delete_url = format!("{}/maintenance/delete", cli.backend_url);
    let resp = match client.post(&delete_url).json(&request).send() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Error contacting backend: {e}");
            std::process::exit(1);
        }
    };

    if !resp.status().is_success() {
        eprintln!(
            "Error: Backend returned status {} - {}",
            resp.status(),
            resp.text().unwrap_or_default()
        );
        std::process::exit(1);
    }

    let result: MaintenancePreviewResponse = match resp.json() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Error parsing response: {e}");
            std::process::exit(1);
        }
    };

    println!("\n--- Deletion Complete ---");
    println!("  Crashes deleted:            {}", result.crashes_count);
    println!("  Admin events deleted:       {}", result.admin_events_count);
    println!(
        "  Coverage snapshots deleted: {}",
        result.coverage_snapshots_count
    );
    println!("\nDone.");
}
