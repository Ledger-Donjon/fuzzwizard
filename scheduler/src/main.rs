// SPDX-FileCopyrightText: © 2026 LEDGER SAS
// SPDX-License-Identifier: MIT

use std::env;
use std::fs;
use std::path::PathBuf;

use ftail::Ftail;
use log::{error, LevelFilter};
use scheduler::build::{BuildOutcome, Builder};
use scheduler::{DirectoryTargetsCollector, Scheduler};

fn main() {
    let config_path = env::args().nth(1);
    let config_path = match config_path {
        Some(ref path) if !path.trim().is_empty() => path.trim(),
        _ => {
            eprintln!("Error: Config path must be provided as the first argument.");
            std::process::exit(1);
        }
    };

    let config_contents = match fs::read_to_string(config_path) {
        Ok(contents) => contents,
        Err(e) => {
            eprintln!("Failed to read config file '{config_path}': {e}");
            std::process::exit(1);
        }
    };

    let logdir = env::args().nth(2);
    let logdir: String = match logdir {
        Some(ref path) if !path.trim().is_empty() => path.trim().to_string(),
        _ => {
            eprintln!("Log directory not provided, using default ./logs");
            "./logs".to_string()
        }
    };
    // If logdir does not exist, create it
    if let Err(e) = std::fs::create_dir_all(&logdir) {
        eprintln!("Failed to create log directory '{logdir}': {e}");
        std::process::exit(1);
    }

    // Configure ftail to write to a file
    let log_file_path = format!("{logdir}/scheduler.log");
    // Ensure the directory exists
    if let Some(parent) = std::path::Path::new(&log_file_path).parent() {
        if let Err(e) = fs::create_dir_all(parent) {
            eprintln!("Failed to create log directory '{}': {e}", parent.display());
            std::process::exit(1);
        }
    }

    if let Err(e) = Ftail::new()
        .single_file(
            std::path::Path::new(&log_file_path),
            true,
            LevelFilter::Info,
        )
        .init()
    {
        eprintln!("Failed to initialize logger: {e}");
        std::process::exit(1);
    }

    let container_binary = env::args().nth(3);

    let config_json: scheduler::SchedulerConfig = match serde_json::from_str(&config_contents) {
        Ok(config) => config,
        Err(e) => {
            eprintln!("Failed to parse config file as JSON: {e}");
            std::process::exit(1);
        }
    };

    let collector = DirectoryTargetsCollector::new(
        config_json
            .targets_directory
            .iter()
            .map(PathBuf::from)
            .collect(),
    );
    // Targets are built in the background: the first cycles may find none, but
    // the scheduler keeps reporting crashes and coverage while the build runs.
    let mut builder = Builder::new(
        config_json.scheduler_name.clone(),
        config_json.build_script.clone(),
        config_json.build_script_interval_hours,
    );
    let mut scheduler = Scheduler::new(
        collector,
        config_json.clone(),
        container_binary.map(PathBuf::from),
    );

    loop {
        if let Some(BuildOutcome::Failure(msg)) = builder.poll() {
            error!("{msg}");
        }
        let _ = scheduler.update(&logdir);
        std::thread::sleep(std::time::Duration::from_secs(60));
        scheduler.report_crashes().unwrap();
        scheduler.log_running_tasks();
        scheduler.collect_coverage();
        scheduler.snapshot_coverage_if_due();
        scheduler.report_status();
    }
}
