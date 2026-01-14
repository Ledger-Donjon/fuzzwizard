// SPDX-FileCopyrightText: © 2026 LEDGER SAS
// SPDX-License-Identifier: MIT

use std::{collections::HashMap, fs, path::PathBuf};

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
fn test_collect_coverage_disabled() {
    let collector = MockCollector {
        targets: HashMap::new(),
    };

    let temp_dir = assert_fs::TempDir::new().unwrap();

    let config = SchedulerConfig {
        scheduler_name: "test_coverage_disabled".to_string(),
        targets_directory: vec![],
        build_script: "".to_string(),
        backend_url: "http://127.0.0.1:0".to_string(),
        corpus_directory: temp_dir.path().join("corpus").to_string_lossy().into(),
        crashes_directory: temp_dir.path().join("crashes").to_string_lossy().into(),
        coverage_directory: temp_dir.path().join("coverage").to_string_lossy().into(),
        container_image: None,
        container_options: None,
        has_coverage: false,
        state_file: None,
    };

    let scheduler = Scheduler::new(collector, config, None);

    // Should return early and not do anything
    scheduler.collect_coverage();
}

#[test]
fn test_collect_coverage_enabled_no_tasks() {
    let collector = MockCollector {
        targets: HashMap::new(),
    };

    let temp_dir = assert_fs::TempDir::new().unwrap();

    let config = SchedulerConfig {
        scheduler_name: "test_coverage_no_tasks".to_string(),
        targets_directory: vec![],
        build_script: "".to_string(),
        backend_url: "http://127.0.0.1:0".to_string(),
        corpus_directory: temp_dir.path().join("corpus").to_string_lossy().into(),
        crashes_directory: temp_dir.path().join("crashes").to_string_lossy().into(),
        coverage_directory: temp_dir.path().join("coverage").to_string_lossy().into(),
        container_image: None,
        container_options: None,
        has_coverage: true,
        state_file: None,
    };

    let scheduler = Scheduler::new(collector, config, None);

    // Should not panic with no tasks
    scheduler.collect_coverage();
}

#[test]
fn test_collect_coverage_enabled_with_tasks() {
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
        scheduler_name: "test_coverage_with_tasks".to_string(),
        targets_directory: vec![],
        build_script: "".to_string(),
        backend_url: "http://127.0.0.1:0".to_string(),
        corpus_directory: temp_dir.path().join("corpus").to_string_lossy().into(),
        crashes_directory: temp_dir.path().join("crashes").to_string_lossy().into(),
        coverage_directory: temp_dir.path().join("coverage").to_string_lossy().into(),
        container_image: None,
        container_options: None,
        has_coverage: true,
        state_file: None,
    };

    let scheduler = Scheduler::new(collector, config, None);

    // collect_coverage should handle the case where tasks might not be running
    // or where pkill might fail gracefully
    scheduler.collect_coverage();
}

#[test]
fn test_collect_coverage_with_container() {
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
        scheduler_name: "test_coverage_container".to_string(),
        targets_directory: vec![],
        build_script: "".to_string(),
        backend_url: "http://127.0.0.1:0".to_string(),
        corpus_directory: temp_dir.path().join("corpus").to_string_lossy().into(),
        crashes_directory: temp_dir.path().join("crashes").to_string_lossy().into(),
        coverage_directory: temp_dir.path().join("coverage").to_string_lossy().into(),
        container_image: Some("test-image".to_string()),
        container_options: None,
        has_coverage: true,
        state_file: None,
    };

    let scheduler = Scheduler::new(collector, config, None);

    // Should handle container case gracefully even if container_binary is None
    scheduler.collect_coverage();
}
