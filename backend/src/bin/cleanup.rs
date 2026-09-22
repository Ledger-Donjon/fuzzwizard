// SPDX-FileCopyrightText: © 2026 LEDGER SAS
// SPDX-License-Identifier: MIT

use std::process::exit;
use std::time::Duration;

use chrono::{Months, Utc};
use clap::Parser;
use dialoguer::Confirm;
use serde::{Deserialize, Serialize};

/// A maintenance sweep walks whole tables, so the request outlives the usual
/// request/response round trip by a lot.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(600);

#[derive(Parser)]
#[command(
    name = "fuzzwizard-cleanup",
    about = "Interactive maintenance tool for the FuzzWizard backend"
)]
struct Cli {
    /// Backend URL (e.g. http://127.0.0.1:3000)
    #[arg(
        long,
        env = "FUZZWIZARD_BACKEND_URL",
        default_value = "http://127.0.0.1:3000"
    )]
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

    /// Drop the per-file coverage detail older than this many days, keeping the
    /// aggregated snapshots. Use 0 to keep every file row.
    #[arg(long, default_value = "30")]
    coverage_files_days: i64,

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
    #[serde(skip_serializing_if = "Option::is_none")]
    coverage_files_before: Option<String>,
}

/// Row counts, either about to be deleted (preview) or just deleted.
#[derive(Deserialize)]
struct MaintenanceCounts {
    crashes_count: i64,
    admin_events_count: i64,
    coverage_snapshots_count: i64,
    #[serde(default)]
    coverage_files_count: i64,
}

impl MaintenanceCounts {
    fn total(&self) -> i64 {
        self.crashes_count
            + self.admin_events_count
            + self.coverage_snapshots_count
            + self.coverage_files_count
    }

    fn print(&self, verb: &str) {
        for (label, count) in [
            ("Crashes", self.crashes_count),
            ("Admin events", self.admin_events_count),
            ("Coverage snapshots", self.coverage_snapshots_count),
            ("Coverage file rows", self.coverage_files_count),
        ] {
            println!("  {:<32} {count}", format!("{label} {verb}:"));
        }
    }
}

/// Send `request` to a maintenance endpoint, exiting on any failure.
///
/// This is a one-shot admin tool: there is nothing useful to do after a failed
/// preview or a failed deletion but report it and stop.
fn post_maintenance(
    client: &reqwest::blocking::Client,
    backend_url: &str,
    endpoint: &str,
    request: &MaintenanceRequest,
) -> MaintenanceCounts {
    let url = format!(
        "{}/maintenance/{endpoint}",
        backend_url.trim_end_matches('/')
    );

    let response = client
        .post(&url)
        .json(request)
        .send()
        .unwrap_or_else(|e| fail(&format!("Error contacting backend at {url}: {e}")));

    let status = response.status();
    if !status.is_success() {
        let body = response.text().unwrap_or_default();
        fail(&format!("Error: backend returned status {status} - {body}"));
    }

    response
        .json()
        .unwrap_or_else(|e| fail(&format!("Error parsing response from {url}: {e}")))
}

fn fail(message: &str) -> ! {
    eprintln!("{message}");
    exit(1);
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

    let coverage_files_before = if cli.coverage_files_days > 0 {
        now.checked_sub_signed(chrono::Duration::days(cli.coverage_files_days))
            .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
    } else {
        None
    };

    let request = MaintenanceRequest {
        crashes_before: crashes_before.clone(),
        admin_events_before: admin_events_before.clone(),
        coverage_before: coverage_before.clone(),
        coverage_files_before: coverage_files_before.clone(),
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
    match coverage_files_before {
        Some(ref before) => println!(
            "Coverage per-file detail: delete rows older than {} days (before {}); aggregated snapshots are kept",
            cli.coverage_files_days, before
        ),
        None => println!("Coverage per-file detail: not targeted"),
    }
    println!();

    let client = reqwest::blocking::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .unwrap_or_else(|e| fail(&format!("Error building HTTP client: {e}")));

    println!("Fetching preview...");
    let preview = post_maintenance(&client, &cli.backend_url, "preview", &request);

    println!("\n--- Preview ---");
    preview.print("to delete");

    let total = preview.total();
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
    let deleted = post_maintenance(&client, &cli.backend_url, "delete", &request);

    println!("\n--- Deletion Complete ---");
    deleted.print("deleted");
    println!("\nDone.");
}
