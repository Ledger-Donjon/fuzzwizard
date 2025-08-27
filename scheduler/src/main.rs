use log::error;
use scheduler::{Scheduler, TargetsCollector, Sha};
use sha3::{Digest, Sha3_256};
use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf, collections::HashMap};
use std::time::{Duration, Instant};
use std::process::Command;
use log::info;
use std::env;

struct DirectoryTargetsCollector {
    targets_directory: PathBuf,
}

impl TargetsCollector for DirectoryTargetsCollector {
    fn collect_targets(&self) -> HashMap<Sha, PathBuf> {
        let mut targets = HashMap::new();
        for direntry in fs::read_dir(self.targets_directory.clone()).unwrap() {
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
        targets
    }
}
fn build_targets(config_json: serde_json::Value) {
     // Static to keep track of last build time
     static mut LAST_BUILD: Option<Instant> = None;

     // Get build_script from config
     let build_script = config_json.get("build_script")
         .expect("Missing 'build_script' in config")
         .as_str()
         .expect("'build_script' should be a string");

     // Run build_script every minute
     let now = Instant::now();
     let should_run = unsafe {
         match LAST_BUILD {
             Some(last) => now.duration_since(last) >= Duration::from_secs(60),
             None => true,
         }
     };
     if should_run {
         let status = Command::new("sh")
             .arg("-c")
             .arg(build_script)
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
         unsafe {
             LAST_BUILD = Some(now);
         }
     }
}
fn main() {
    env_logger::init();


    let config_path = env::args().nth(1);
    let config_path = match config_path {
        Some(ref path) if !path.trim().is_empty() => path.trim(),
        _ => {
            error!("Error: Config path must be provided as the first argument.");
            std::process::exit(1);
        }
    };

    let config_contents = fs::read_to_string(config_path)
        .expect("Failed to read config file");

    let config_json: serde_json::Value = serde_json::from_str(&config_contents)
        .expect("Failed to parse config file as JSON");
    let scheduler_name = config_json.get("scheduler_name")
        .expect("Missing 'scheduler_name' in config")
        .as_str()
        .expect("'scheduler_name' should be a string");
    let targets_directory = config_json.get("targets_directory")
        .expect("Missing 'targets_directory' in config")
        .as_str()
        .expect("'targets_directory' should be a string");
    let targets_directory = PathBuf::from(targets_directory);

    let collector = DirectoryTargetsCollector {
        targets_directory,
    };

    let backend_url = config_json.get("backend_url")
        .expect("Missing 'backend_url' in config")
        .as_str()
        .expect("'backend_url' should be a string");
    let corpus_directory = config_json.get("corpus_directory")
        .expect("Missing 'corpus_directory' in config")
        .as_str()
        .expect("'corpus_directory' should be a string");
    let crashes_directory = config_json.get("crashes_directory")
        .expect("Missing 'crashes_directory' in config")
        .as_str()
        .expect("'crashes_directory' should be a string");
    build_targets(config_json.clone()); 
    let mut scheduler = Scheduler::new(collector, corpus_directory.into(), crashes_directory.into(), backend_url.to_string(), scheduler_name.to_string());

    loop {
        build_targets(config_json.clone());
        let _ = scheduler.update();
        std::thread::sleep(std::time::Duration::from_secs(3));
        scheduler.report_crashes().unwrap();
        scheduler.log_running_tasks();
    }
}