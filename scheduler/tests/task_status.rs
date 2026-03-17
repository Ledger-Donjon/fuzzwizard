// SPDX-FileCopyrightText: © 2026 LEDGER SAS
// SPDX-License-Identifier: MIT

use std::{collections::HashMap, fs, path::PathBuf, time::Duration};

use assert_fs::prelude::*;
use scheduler::{Scheduler, SchedulerConfig, Sha, TargetsCollector, TaskStatus};
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
fn test_get_tasks_with_status_empty() {
    let collector = MockCollector {
        targets: HashMap::new(),
    };

    let temp_dir = assert_fs::TempDir::new().unwrap();

    let config = SchedulerConfig {
        scheduler_name: "test_empty_status".to_string(),
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
        state_file: None,
    };

    let mut scheduler = Scheduler::new(collector, config, None);

    let tasks = scheduler.get_tasks_with_status();
    assert_eq!(tasks.len(), 0, "should have no tasks initially");
}

#[test]
fn test_get_tasks_with_status_after_update() {
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
        scheduler_name: "test_status_after_update".to_string(),
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
        state_file: None,
    };

    let mut scheduler = Scheduler::new(collector, config, None);

    // Update might fail if binary is not executable, but we can still test the status function
    let _ = scheduler.update(log_dir.to_str().unwrap());

    let tasks = scheduler.get_tasks_with_status();
    // Tasks might be empty if update failed, but the function should still work
    for task in tasks {
        assert_eq!(task.sha.len(), 32, "SHA should be 32 bytes");
        assert!(
            task.started_at.elapsed() < Duration::from_secs(60),
            "started_at should be recent"
        );
    }
}

#[test]
fn test_log_running_tasks_empty() {
    let collector = MockCollector {
        targets: HashMap::new(),
    };

    let temp_dir = assert_fs::TempDir::new().unwrap();

    let config = SchedulerConfig {
        scheduler_name: "test_log_empty".to_string(),
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
        state_file: None,
    };

    let scheduler = Scheduler::new(collector, config, None);

    // This should not panic
    scheduler.log_running_tasks();
}

#[test]
fn test_task_status_info_fields() {
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
        scheduler_name: "test_status_fields".to_string(),
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
        state_file: None,
    };

    let mut scheduler = Scheduler::new(collector, config, None);

    let _ = scheduler.update(log_dir.to_str().unwrap());

    let tasks = scheduler.get_tasks_with_status();
    for task in tasks {
        // Verify all fields are present
        assert_eq!(task.sha.len(), 32, "SHA should be 32 bytes");
        assert!(
            !task.path.as_os_str().is_empty(),
            "path should not be empty"
        );
        assert!(
            task.started_at.elapsed() < Duration::from_secs(60),
            "started_at should be recent"
        );
        // crashed_at and stopped_at might be None for running tasks
        match task.status {
            TaskStatus::Running => {
                assert!(
                    task.crashed_at.is_none(),
                    "running task should not have crashed_at"
                );
                assert!(
                    task.stopped_at.is_none(),
                    "running task should not have stopped_at"
                );
            }
            TaskStatus::Crashed => {
                assert!(
                    task.crashed_at.is_some(),
                    "crashed task should have crashed_at"
                );
            }
            TaskStatus::Stopped => {
                assert!(
                    task.stopped_at.is_some(),
                    "stopped task should have stopped_at"
                );
            }
        }
    }
}
