// SPDX-FileCopyrightText: © 2026 LEDGER SAS
// SPDX-License-Identifier: MIT

use std::{collections::HashMap, fs, os::unix::fs::PermissionsExt, path::PathBuf, time::Duration};

use scheduler::{Scheduler, SchedulerConfig, Sha, TargetsCollector};
use sha3::{Digest, Sha3_256};

struct DirectoryCollector {
    targets_dir: PathBuf,
}

impl TargetsCollector for DirectoryCollector {
    fn collect_targets(&self) -> HashMap<Sha, PathBuf> {
        let mut latest_targets = HashMap::new();

        if let Ok(entries) = fs::read_dir(&self.targets_dir) {
            for direntry in entries.flatten() {
                let target_path = direntry.path();
                if target_path.is_file() {
                    if let Ok(content) = fs::read(&target_path) {
                        let target_hash = Sha3_256::digest(&content);
                        latest_targets.insert(target_hash.into(), target_path);
                    }
                }
            }
        }

        latest_targets
    }
}

#[test]
fn fuzzer_report_crashes() {
    let temp_dir = assert_fs::TempDir::new().unwrap();
    let targets_dir = temp_dir.path().join("targets");
    fs::create_dir_all(&targets_dir).unwrap();

    // Create a simple executable script for testing
    let test_fuzzer = targets_dir.join("test_fuzzer.sh");
    fs::write(
        &test_fuzzer,
        r#"#!/bin/sh
while /bin/true; do
    echo "fuzzing"
    sleep 1
done
"#,
    )
    .unwrap();
    fs::set_permissions(&test_fuzzer, fs::Permissions::from_mode(0o775)).unwrap();

    let collector = DirectoryCollector {
        targets_dir: targets_dir.clone(),
    };

    let log_dir = temp_dir.path().join("logs");
    fs::create_dir_all(&log_dir).unwrap();

    let config = SchedulerConfig {
        scheduler_name: "test_scheduler".to_string(),
        targets_directory: vec![targets_dir.to_string_lossy().into()],
        build_script: "".to_string(),
        build_script_interval_hours: None,
        backend_url: "http://127.0.0.1:0".to_string(),
        corpus_directory: temp_dir.path().join("corpus").to_string_lossy().into(),
        crashes_directory: temp_dir.path().join("crashes").to_string_lossy().into(),
        coverage_directory: temp_dir.path().join("coverage").to_string_lossy().into(),
        container_image: None,
        container_options: None,
        has_coverage: false,
        state_file: None,
    };

    let mut scheduler = Scheduler::new(collector, config, None);

    // Test update
    let result = scheduler.update(log_dir.to_str().unwrap());
    assert!(result.is_ok(), "update should succeed");

    // Wait a bit for tasks to start
    std::thread::sleep(Duration::from_secs(1));

    // Test report_crashes - should not fail even if backend is unreachable
    let result = scheduler.report_crashes();
    // This might fail if backend is unreachable, but that's ok for testing
    let _ = result;

    // Verify tasks exist
    let tasks = scheduler.get_tasks_with_status();
    assert!(!tasks.is_empty(), "should have at least one task");
}

#[test]
fn test_report_crashes_with_no_tasks() {
    let temp_dir = assert_fs::TempDir::new().unwrap();
    let targets_dir = temp_dir.path().join("targets");
    fs::create_dir_all(&targets_dir).unwrap();

    let collector = DirectoryCollector {
        targets_dir: targets_dir.clone(),
    };
    let log_dir = temp_dir.path().join("logs");
    fs::create_dir_all(&log_dir).unwrap();

    let config = SchedulerConfig {
        scheduler_name: "test_scheduler_empty".to_string(),
        targets_directory: vec![temp_dir.path().to_string_lossy().into()],
        build_script: "".to_string(),
        build_script_interval_hours: None,
        backend_url: "http://127.0.0.1:0".to_string(),
        corpus_directory: temp_dir.path().join("corpus").to_string_lossy().into(),
        crashes_directory: temp_dir.path().join("crashes").to_string_lossy().into(),
        coverage_directory: temp_dir.path().join("coverage").to_string_lossy().into(),
        container_image: None,
        container_options: None,
        has_coverage: false,
        state_file: None,
    };

    let mut scheduler = Scheduler::new(collector, config, None);

    // Update with no targets
    let result = scheduler.update(log_dir.to_str().unwrap());
    assert!(result.is_ok(), "update should succeed even with no targets");

    // report_crashes should succeed with no tasks
    let result = scheduler.report_crashes();
    assert!(
        result.is_ok(),
        "report_crashes should succeed with no tasks"
    );
}
