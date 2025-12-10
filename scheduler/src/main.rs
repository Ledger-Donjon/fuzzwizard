use ftail::Ftail;
use log::LevelFilter;
use log::error;
use log::info;
use scheduler::{Scheduler, Sha, TargetsCollector};
use sha3::{Digest, Sha3_256};
use std::env;
use std::process::Command;
use std::{collections::HashMap, fs, os::unix::fs::PermissionsExt, path::PathBuf};

struct DirectoryTargetsCollector {
    targets_directories: Vec<PathBuf>,
}

impl TargetsCollector for DirectoryTargetsCollector {
    fn collect_targets(&self) -> HashMap<Sha, PathBuf> {
        let mut targets = HashMap::new();
        for directory in self.targets_directories.iter().cloned() {
            for direntry in fs::read_dir(directory).unwrap() {
                let direntry = direntry.unwrap();
                let target_path = direntry.path();
                if !target_path.is_file() {
                    continue;
                }
                if target_path.metadata().unwrap().permissions().mode() & 0o111 == 0 {
                    continue;
                }

                let target_hash = Sha3_256::digest(fs::read(&target_path).unwrap());
                targets.insert(target_hash.into(), target_path);
            }
        }
        targets
    }
}
fn build_targets(config_json: scheduler::SchedulerConfig) {
    // Static to keep track of last build time
    let status = Command::new("sh")
        .arg("-c")
        .arg(config_json.build_script)
        .status();
    match status {
        Ok(s) if s.success() => {
            info!("build_script ran successfully");
        }
        Ok(s) => {
            error!("build_script exited with status: {}", s);
        }
        Err(e) => {
            error!("Failed to run build_script: {}", e);
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

    let config_contents = fs::read_to_string(config_path).expect("Failed to read config file");

    let logdir = env::args().nth(2);
    let logdir = match logdir {
        Some(ref path) if !path.trim().is_empty() => path.trim(),
        _ => {
            eprintln!("Log directory not provided, using default ./logs");
            "./logs"
        }
    };
    // If logdir does not exist, create it
    if let Err(e) = std::fs::create_dir_all(logdir) {
        eprintln!("Failed to create log directory '{}': {}", logdir, e);
        std::process::exit(1);
    }

    // Configure ftail to write to a file
    let log_file_path = format!("{}/scheduler.log", logdir);
    // Ensure the directory exists
    if let Some(parent) = std::path::Path::new(&log_file_path).parent() {
        fs::create_dir_all(parent).expect("Failed to create log directory");
    }

    Ftail::new()
        .single_file(
            std::path::Path::new(&log_file_path),
            true,
            LevelFilter::Info,
        )
        .init()
        .expect("Failed to initialize logger");

    let container_binary = env::args().nth(3);

    let config_json: scheduler::SchedulerConfig =
        serde_json::from_str(&config_contents).expect("Failed to parse config file as JSON");

    let collector = DirectoryTargetsCollector {
        targets_directories: config_json
            .targets_directory
            .iter()
            .map(|p| PathBuf::from(p))
            .collect(),
    };
    build_targets(config_json.clone());
    let mut scheduler = Scheduler::new(
        collector,
        config_json.clone(),
        container_binary.map(PathBuf::from),
    );

    loop {
        build_targets(config_json.clone());
        let _ = scheduler.update(logdir);
        std::thread::sleep(std::time::Duration::from_secs(60));
        scheduler.report_crashes().unwrap();
        scheduler.log_running_tasks();
        scheduler.collect_coverage();
    }
}
