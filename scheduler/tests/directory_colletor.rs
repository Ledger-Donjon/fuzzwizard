// SPDX-FileCopyrightText: © 2026 LEDGER SAS
// SPDX-License-Identifier: MIT

use std::{collections::HashMap, fs, os::unix::fs::PermissionsExt, path::PathBuf, time::Duration};

use assert_fs::prelude::{FileWriteStr, PathChild};
use scheduler::{Scheduler, SchedulerConfig, Sha, TargetsCollector, TaskStatus};
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
                // Only process files, not directories
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
fn directory_collector() {
    let temp_dir = assert_fs::TempDir::new().unwrap();

    let collector = DirectoryCollector {
        targets_dir: temp_dir.to_path_buf(),
    };

    let config = SchedulerConfig {
        targets_directory: vec![temp_dir.to_path_buf().to_string_lossy().into()],
        build_script: "".to_string(),
        scheduler_name: "test_scheduler".to_string(),
        corpus_directory: std::env::temp_dir().to_string_lossy().into(),
        crashes_directory: std::env::temp_dir()
            .join("crashes")
            .to_string_lossy()
            .into(),
        coverage_directory: std::env::temp_dir()
            .join("coverage")
            .to_string_lossy()
            .into(),
        container_image: None,
        container_options: None,
        backend_url: "http://127.0.0.1:0".to_string(),
        has_coverage: false,
        state_file: None,
    };

    let mut scheduler = Scheduler::new(collector, config, None);

    let a_sh = temp_dir.child("a.sh");
    a_sh.write_str(
        r#"#!/bin/sh

while /bin/true; do
    echo a
    sleep 1
done
"#,
    )
    .unwrap();
    fs::set_permissions(a_sh.path(), fs::Permissions::from_mode(0o775)).unwrap();

    let b_sh = temp_dir.child("b.sh");
    b_sh.write_str(
        r#"#!/bin/sh

while /bin/true; do
    echo b
    sleep 1
done
"#,
    )
    .unwrap();
    fs::set_permissions(b_sh.path(), fs::Permissions::from_mode(0o775)).unwrap();

    let log_dir = temp_dir.path().join("logs");
    fs::create_dir_all(&log_dir).unwrap();

    let result = scheduler.update(log_dir.to_str().unwrap());
    assert!(result.is_ok(), "first update should succeed");

    // Verify tasks were started
    let tasks = scheduler.get_tasks_with_status();
    assert_eq!(tasks.len(), 2, "should have 2 tasks after first update");

    // Wait for processes to complete (they exit immediately)
    // Use a timeout to avoid infinite loops
    let mut attempts = 0;
    while attempts < 20 {
        let tasks = scheduler.get_tasks_with_status();
        let running_count = tasks
            .iter()
            .filter(|t| t.status == TaskStatus::Running)
            .count();

        if running_count == 0 {
            break;
        }

        std::thread::sleep(Duration::from_millis(100));
        attempts += 1;
    }

    // Kill all tasks before removing files to avoid issues
    scheduler.kill_tasks();
    std::thread::sleep(Duration::from_millis(100));

    // Remove one target
    fs::remove_file(a_sh.path()).unwrap();

    // Add a new target
    let c_sh = temp_dir.child("c.sh");
    c_sh.write_str(
        r#"#!/bin/sh
echo c
exit 0
"#,
    )
    .unwrap();
    fs::set_permissions(c_sh.path(), fs::Permissions::from_mode(0o775)).unwrap();

    let result = scheduler.update(log_dir.to_str().unwrap());
    assert!(result.is_ok(), "second update should succeed");

    // Verify tasks were updated
    let tasks = scheduler.get_tasks_with_status();
    assert_eq!(
        tasks.len(),
        2,
        "should still have 2 tasks after second update"
    );

    // Wait for processes to complete (they exit immediately)
    // Use a timeout to avoid infinite loops
    let mut attempts = 0;
    while attempts < 20 {
        let tasks = scheduler.get_tasks_with_status();
        let running_count = tasks
            .iter()
            .filter(|t| t.status == TaskStatus::Running)
            .count();

        if running_count == 0 {
            break;
        }

        std::thread::sleep(Duration::from_millis(100));
        attempts += 1;
    }

    // Force kill all tasks before test ends
    scheduler.kill_tasks();

    // Wait for cleanup
    std::thread::sleep(Duration::from_millis(200));

    // Final kill to ensure everything is cleaned up
    scheduler.kill_tasks();
    std::thread::sleep(Duration::from_millis(100));

    // Clear all tasks to ensure TempDir can clean up
    // The scheduler will be dropped, but we want to make sure processes are killed
    drop(scheduler);

    // Give a moment for all processes to be fully cleaned up
    std::thread::sleep(Duration::from_millis(200));
}
