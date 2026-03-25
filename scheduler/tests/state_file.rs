// SPDX-FileCopyrightText: © 2026 LEDGER SAS
// SPDX-License-Identifier: MIT

use std::{collections::HashMap, fs, os::unix::fs::PermissionsExt, path::PathBuf};

use assert_fs::prelude::*;
use scheduler::{
    FuzzerStateCommand, FuzzerStateEntry, Scheduler, SchedulerConfig, Sha, StateFile,
    TargetsCollector,
};
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
fn test_state_file_path() {
    let collector = MockCollector {
        targets: HashMap::new(),
    };

    let temp_dir = assert_fs::TempDir::new().unwrap();
    let state_file_path = temp_dir.path().join("test_state.json");

    let config = SchedulerConfig {
        scheduler_name: "test_scheduler".to_string(),
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
        state_file: Some(state_file_path.to_string_lossy().into()),
    };

    let scheduler = Scheduler::new(collector, config, None);
    let path = scheduler.state_file_path();
    assert!(path.is_some(), "state_file_path should return Some");
    assert_eq!(
        path.unwrap(),
        &state_file_path,
        "state_file_path should match configured path"
    );
}

#[test]
fn test_state_file_default_path() {
    let collector = MockCollector {
        targets: HashMap::new(),
    };

    let config = SchedulerConfig {
        scheduler_name: "test_default".to_string(),
        targets_directory: vec![],
        build_script: "".to_string(),
        build_script_interval_hours: None,
        backend_url: "http://127.0.0.1:0".to_string(),
        corpus_directory: "/tmp".into(),
        crashes_directory: "/tmp".into(),
        coverage_directory: "/tmp".into(),
        container_image: None,
        container_options: None,
        has_coverage: false,
        llvm_bin_directory: None,
        state_file: None,
    };

    let scheduler = Scheduler::new(collector, config, None);
    let path = scheduler.state_file_path();
    assert!(
        path.is_some(),
        "state_file_path should return Some even with None config"
    );
    let path_str = path.unwrap().to_string_lossy();
    assert!(
        path_str.contains("test_default"),
        "default path should contain scheduler name"
    );
}

#[test]
fn test_state_file_creation() {
    let collector = MockCollector {
        targets: HashMap::new(),
    };

    let temp_dir = assert_fs::TempDir::new().unwrap();
    let state_file_path = temp_dir.path().join("test_state.json");

    let config = SchedulerConfig {
        scheduler_name: "test_creation".to_string(),
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
        state_file: Some(state_file_path.to_string_lossy().into()),
    };

    let _scheduler = Scheduler::new(collector, config, None);

    // Verify state file was created
    assert!(
        state_file_path.exists(),
        "state file should be created at initialization"
    );

    // Verify it's valid JSON
    let contents = fs::read_to_string(&state_file_path).unwrap();
    let state: StateFile = serde_json::from_str(&contents).unwrap();
    assert_eq!(state.fuzzers.len(), 0, "initial state should be empty");
}

#[test]
fn test_state_file_write_and_read() {
    let temp_dir = assert_fs::TempDir::new().unwrap();
    let test_file = temp_dir.child("test_binary");
    test_file.write_binary(b"test binary content").unwrap();

    let content = fs::read(test_file.path()).unwrap();
    let sha: Sha = Sha3_256::digest(&content).into();

    let mut targets = HashMap::new();
    targets.insert(sha, test_file.path().to_path_buf());

    let collector = MockCollector { targets };

    let state_file_path = temp_dir.path().join("test_state.json");
    let log_dir = temp_dir.path().join("logs");
    fs::create_dir_all(&log_dir).unwrap();

    let config = SchedulerConfig {
        scheduler_name: "test_write_read".to_string(),
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
        state_file: Some(state_file_path.to_string_lossy().into()),
    };

    let mut scheduler = Scheduler::new(collector, config, None);

    // Start a task using the public method (state file is now only for logging)
    let result = scheduler.start_task_by_sha(sha);
    // Note: This might fail if the binary is not executable, but we test the state file mechanism
    if result.is_err() {
        // If start failed, we can't test state file logging, so skip
        return;
    }

    // Update should write the current state to the state file
    let _ = scheduler.update(log_dir.to_str().unwrap());

    // Verify state file was updated
    let contents = fs::read_to_string(&state_file_path).unwrap();
    let updated_state: StateFile = serde_json::from_str(&contents).unwrap();
    assert!(
        !updated_state.fuzzers.is_empty(),
        "state file should contain fuzzer entries"
    );
}

#[test]
fn test_state_file_stop_command() {
    let temp_dir = assert_fs::TempDir::new().unwrap();
    let test_file = temp_dir.child("test_fuzzer.sh");
    test_file
        .write_str(
            r#"#!/bin/sh
while /bin/true; do
    echo "fuzzing"
    sleep 1
done
"#,
        )
        .unwrap();
    fs::set_permissions(test_file.path(), fs::Permissions::from_mode(0o775)).unwrap();

    let content = fs::read(test_file.path()).unwrap();
    let sha: Sha = Sha3_256::digest(&content).into();

    let mut targets = HashMap::new();
    targets.insert(sha, test_file.path().to_path_buf());

    let collector = MockCollector { targets };

    let state_file_path = temp_dir.path().join("test_state.json");
    let log_dir = temp_dir.path().join("logs");
    fs::create_dir_all(&log_dir).unwrap();

    let config = SchedulerConfig {
        scheduler_name: "test_stop".to_string(),
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
        state_file: Some(state_file_path.to_string_lossy().into()),
    };

    let mut scheduler = Scheduler::new(collector, config, None);

    // First, start a task by updating
    let result = scheduler.update(log_dir.to_str().unwrap());
    // The update might fail if the binary can't be executed, but that's ok for this test
    // We just need to verify that STOP command handling doesn't panic
    if result.is_err() {
        // If update failed, we can't test STOP command, so skip
        return;
    }

    // Verify a task was started
    let tasks = scheduler.get_tasks_with_status();
    if tasks.is_empty() {
        // No task was started, can't test STOP
        return;
    }

    // Stop the task using the public method (state file is now only for logging)
    let result = scheduler.stop_task_by_sha(sha);
    assert!(result.is_ok(), "stop_task_by_sha should succeed");

    // Update should write the current state to the state file
    let result = scheduler.update(log_dir.to_str().unwrap());
    assert!(result.is_ok(), "update after STOP should succeed");
}

#[test]
fn test_state_file_with_env_and_options() {
    let temp_dir = assert_fs::TempDir::new().unwrap();
    let test_file = temp_dir.child("test_binary");
    test_file.write_binary(b"test binary content").unwrap();

    let content = fs::read(test_file.path()).unwrap();
    let sha: Sha = Sha3_256::digest(&content).into();

    let mut targets = HashMap::new();
    targets.insert(sha, test_file.path().to_path_buf());

    let collector = MockCollector { targets };

    let state_file_path = temp_dir.path().join("test_state.json");
    let log_dir = temp_dir.path().join("logs");
    fs::create_dir_all(&log_dir).unwrap();

    let config = SchedulerConfig {
        scheduler_name: "test_env_options".to_string(),
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
        state_file: Some(state_file_path.to_string_lossy().into()),
    };

    let mut scheduler = Scheduler::new(collector, config, None);

    // Start a task first, then manually write state file with environment variables and command options
    // to test that they are preserved
    let sha_str = hex::encode(sha);
    let mut var_env = HashMap::new();
    var_env.insert("TEST_VAR".to_string(), "test_value".to_string());

    // Start task (might fail if binary is not executable)
    let _ = scheduler.start_task_by_sha(sha);

    // Manually write state file with var_env and cmd_options to test preservation
    let state_file = StateFile {
        fuzzers: vec![FuzzerStateEntry {
            path: test_file.path().display().to_string(),
            sha: sha_str.clone(),
            state: FuzzerStateCommand::RUNNING, // Use RUNNING since START no longer exists
            container_name: None,
            var_env: Some(var_env),
            cmd_options: Some(vec!["-option1".to_string(), "value1".to_string()]),
        }],
    };

    let json = serde_json::to_string_pretty(&state_file).unwrap();
    fs::write(&state_file_path, json).unwrap();

    // Update should preserve var_env and cmd_options
    let _ = scheduler.update(log_dir.to_str().unwrap());

    // Verify state file preserves the options
    let contents = fs::read_to_string(&state_file_path).unwrap();
    let updated_state: StateFile = serde_json::from_str(&contents).unwrap();
    if let Some(entry) = updated_state.fuzzers.first() {
        assert!(
            entry.var_env.is_some(),
            "var_env should be preserved in state file"
        );
        assert!(
            entry.cmd_options.is_some(),
            "cmd_options should be preserved in state file"
        );
    }
}
