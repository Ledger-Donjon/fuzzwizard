// SPDX-FileCopyrightText: © 2026 LEDGER SAS
// SPDX-License-Identifier: MIT

use std::{collections::HashMap, fs, path::PathBuf, time::Duration};

use assert_fs::prelude::*;
use scheduler::{Scheduler, SchedulerConfig, Sha, TargetsCollector};
use sha3::{Digest, Sha3_256};

struct MockCollector {
    targets: HashMap<Sha, PathBuf>,
}

impl TargetsCollector for MockCollector {
    fn collect_targets(&self) -> HashMap<Sha, PathBuf> {
        self.targets.clone()
    }
}

#[test]
fn test_kill_tasks_empty() {
    let collector = MockCollector {
        targets: HashMap::new(),
    };

    let temp_dir = assert_fs::TempDir::new().unwrap();

    let config = SchedulerConfig {
        scheduler_name: "test_kill_empty".to_string(),
        targets_directory: vec![],
        build_script: "".to_string(),
        build_script_interval_hours: None,
        backend_url: "http://127.0.0.1:0".to_string(),
        corpus_directory: temp_dir.path().join("corpus").to_string_lossy().into(),
        crashes_directory: temp_dir.path().join("crashes").to_string_lossy().into(),
        coverage_directory: temp_dir.path().join("coverage").to_string_lossy().into(),
        container_image: None,
        container_options: None,
        has_coverage: false,
        llvm_bin_directory: None,
        state_file: None,
    };

    let mut scheduler = Scheduler::new(collector, config, None);

    // Should not panic with no tasks
    scheduler.kill_tasks();
}

#[test]
fn test_kill_tasks_with_running_tasks() {
    let temp_dir = assert_fs::TempDir::new().unwrap();
    let test_file = temp_dir.child("test_binary");
    test_file.write_binary(b"test binary content").unwrap();

    let content = fs::read(test_file.path()).unwrap();
    let sha: Sha = Sha3_256::digest(&content).into();

    let mut targets = HashMap::new();
    targets.insert(sha, test_file.path().to_path_buf());

    let collector = MockCollector { targets };

    let log_dir = temp_dir.path().join("logs");
    fs::create_dir_all(&log_dir).unwrap();

    let config = SchedulerConfig {
        scheduler_name: "test_kill_running".to_string(),
        targets_directory: vec![],
        build_script: "".to_string(),
        build_script_interval_hours: None,
        backend_url: "http://127.0.0.1:0".to_string(),
        corpus_directory: temp_dir.path().join("corpus").to_string_lossy().into(),
        crashes_directory: temp_dir.path().join("crashes").to_string_lossy().into(),
        coverage_directory: temp_dir.path().join("coverage").to_string_lossy().into(),
        container_image: None,
        container_options: None,
        has_coverage: false,
        llvm_bin_directory: None,
        state_file: None,
    };

    let mut scheduler = Scheduler::new(collector, config, None);

    // Try to start tasks
    let _ = scheduler.update(log_dir.to_str().unwrap());

    std::thread::sleep(Duration::from_secs(1));

    // Kill tasks - should not panic
    scheduler.kill_tasks();

    // Wait a bit for cleanup
    std::thread::sleep(Duration::from_secs(1));
}

#[test]
fn test_kill_tasks_with_container() {
    let temp_dir = assert_fs::TempDir::new().unwrap();
    let test_file = temp_dir.child("test_binary");
    test_file.write_binary(b"test binary content").unwrap();

    let content = fs::read(test_file.path()).unwrap();
    let sha: Sha = Sha3_256::digest(&content).into();

    let mut targets = HashMap::new();
    targets.insert(sha, test_file.path().to_path_buf());

    let collector = MockCollector { targets };

    let log_dir = temp_dir.path().join("logs");
    fs::create_dir_all(&log_dir).unwrap();

    // Test with container configuration (even if container binary doesn't exist)
    let config = SchedulerConfig {
        scheduler_name: "test_kill_container".to_string(),
        targets_directory: vec![],
        build_script: "".to_string(),
        build_script_interval_hours: None,
        backend_url: "http://127.0.0.1:0".to_string(),
        corpus_directory: temp_dir.path().join("corpus").to_string_lossy().into(),
        crashes_directory: temp_dir.path().join("crashes").to_string_lossy().into(),
        coverage_directory: temp_dir.path().join("coverage").to_string_lossy().into(),
        container_image: Some("test-image".to_string()),
        container_options: None,
        has_coverage: false,
        llvm_bin_directory: None,
        state_file: None,
    };

    // Note: This will fail if container_binary is None, but we test the kill logic
    let mut scheduler = Scheduler::new(collector, config, None);

    // kill_tasks should handle the case gracefully
    scheduler.kill_tasks();
}
