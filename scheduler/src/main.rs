// SPDX-FileCopyrightText: © 2026 LEDGER SAS
// SPDX-License-Identifier: MIT

use ftail::Ftail;
use log::error;
use log::info;
use log::LevelFilter;
use scheduler::{DirectoryTargetsCollector, Scheduler};
use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
fn build_targets(config_json: scheduler::SchedulerConfig) {
    // Validate build script for security
    let build_script = &config_json.build_script;

    // Check script is not empty
    if build_script.trim().is_empty() {
        error!("Build script is empty, refusing to execute");
        return;
    }

    // Check for null bytes
    if build_script.contains('\0') {
        error!("Build script contains null bytes, refusing to execute");
        return;
    }

    // Check script length (max 64KB)
    if build_script.len() > 65536 {
        error!(
            "Build script too long ({} bytes), refusing to execute",
            build_script.len()
        );
        return;
    }

    // Check for dangerous shell patterns that could indicate command injection
    // Note: This is defense in depth - the config file should be trusted, but we validate anyway
    const DANGEROUS_PATTERNS: &[&str] = &[
        // Command chaining that could be abused
        "&&", "||", ";", // Command substitution
        "`", "$(", // Redirection that could overwrite files
        ">>", "<<",
    ];

    // Only warn for dangerous patterns since build scripts may legitimately use some of these
    for pattern in DANGEROUS_PATTERNS {
        if build_script.contains(pattern) {
            info!(
                "Warning: Build script contains potentially dangerous pattern '{pattern}'. \
                 Ensure the script is from a trusted source."
            );
        }
    }

    // Log the script being executed for audit purposes
    info!("Executing build script: {build_script}");

    let status = Command::new("sh")
        .arg("-c")
        .arg(build_script)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    match status {
        Ok(s) if s.success() => {
            info!("build_script ran successfully");
        }
        Ok(s) => {
            error!("build_script exited with status: {s}");
        }
        Err(e) => {
            error!("Failed to run build_script: {e}");
        }
    }
}
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
    build_targets(config_json.clone());
    let mut scheduler = Scheduler::new(
        collector,
        config_json.clone(),
        container_binary.map(PathBuf::from),
    );

    loop {
        build_targets(config_json.clone());
        let _ = scheduler.update(&logdir);
        std::thread::sleep(std::time::Duration::from_secs(60));
        scheduler.report_crashes().unwrap();
        scheduler.log_running_tasks();
        scheduler.collect_coverage();
    }
}
