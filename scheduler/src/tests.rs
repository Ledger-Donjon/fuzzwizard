// SPDX-FileCopyrightText: © 2026 LEDGER SAS
// SPDX-License-Identifier: MIT

//! Unit tests for the private helpers of the crate root.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use assert_fs::prelude::*;

use super::process::resolve_profraw_paths;
use super::{
    snapshot_due_at, snapshot_interval_from, DirectoryTargetsCollector, Scheduler, SchedulerConfig,
    DEFAULT_SNAPSHOT_INTERVAL_MINUTES, FIRST_SNAPSHOT_DELAY, SNAPSHOT_PROFDATA,
};

#[test]
fn resolves_llvm_online_merge_profiles() {
    let temp_dir = assert_fs::TempDir::new().unwrap();
    temp_dir
        .child("abc-123456_0.profraw")
        .write_binary(b"profile 1")
        .unwrap();
    temp_dir
        .child("abc-987654_0.profraw")
        .write_binary(b"profile 2")
        .unwrap();
    temp_dir
        .child("other-123456_0.profraw")
        .write_binary(b"other")
        .unwrap();

    let paths = resolve_profraw_paths(&temp_dir.path().join("abc-%m.profraw"));

    assert_eq!(paths.len(), 2);
    assert!(paths.iter().all(|path| {
        path.file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("abc-")
    }));
}

#[test]
fn an_absent_interval_falls_back_to_the_default() {
    assert_eq!(
        snapshot_interval_from(None),
        Some(Duration::from_secs(DEFAULT_SNAPSHOT_INTERVAL_MINUTES * 60))
    );
}

#[test]
fn zero_disables_the_periodic_snapshot() {
    assert_eq!(snapshot_interval_from(Some(0)), None);
}

#[test]
fn minutes_are_converted_to_a_duration() {
    assert_eq!(
        snapshot_interval_from(Some(15)),
        Some(Duration::from_secs(900))
    );
}

#[test]
fn an_absurd_interval_does_not_overflow() {
    let interval = snapshot_interval_from(Some(u64::MAX)).unwrap();
    let start = Instant::now();

    let first = snapshot_due_at(start, None, interval);
    assert_eq!(first, Some(start + FIRST_SNAPSHOT_DELAY));

    // The check after the first snapshot is the one that used to panic.
    assert_eq!(snapshot_due_at(start, first, interval), None);
}

fn test_scheduler(
    dir: &assert_fs::TempDir,
    container_image: Option<&str>,
    container_binary: Option<PathBuf>,
) -> Scheduler<DirectoryTargetsCollector> {
    scheduler_with_targets(dir, container_image, container_binary, vec![])
}

fn scheduler_with_targets(
    dir: &assert_fs::TempDir,
    container_image: Option<&str>,
    container_binary: Option<PathBuf>,
    targets_directories: Vec<PathBuf>,
) -> Scheduler<DirectoryTargetsCollector> {
    let path = |name: &str| dir.path().join(name).to_string_lossy().into_owned();
    let config = SchedulerConfig {
        scheduler_name: "unit".to_string(),
        targets_directory: vec![],
        build_script: String::new(),
        build_script_interval_hours: None,
        backend_url: "http://127.0.0.1:0".to_string(),
        corpus_directory: path("corpus"),
        crashes_directory: path("crashes"),
        coverage_directory: path("coverage"),
        container_image: container_image.map(str::to_string),
        container_options: None,
        has_coverage: false,
        llvm_bin_directory: None,
        state_file: Some(path("state.json")),
        coverage_snapshot_interval_minutes: None,
    };
    Scheduler::new(
        DirectoryTargetsCollector::new(targets_directories),
        config,
        container_binary,
    )
}

fn executable(dir: &assert_fs::TempDir, name: &str, body: &str) -> PathBuf {
    let file = dir.child(name);
    file.write_str(body).unwrap();
    fs::set_permissions(file.path(), fs::Permissions::from_mode(0o755)).unwrap();
    file.path().to_path_buf()
}

/// Everything after the image name goes to the fuzzer: an environment
/// variable placed there would never reach the container.
#[test]
fn container_environment_is_set_before_the_image() {
    let dir = assert_fs::TempDir::new().unwrap();
    let args_file = dir.path().join("args");
    let runtime = executable(
        &dir,
        "docker",
        &format!(
            "#!/bin/sh\nfor a in \"$@\"; do echo \"$a\"; done > '{}'\n",
            args_file.display()
        ),
    );
    let fuzzer = executable(&dir, "fuzzer", "#!/bin/sh\n");
    let scheduler = test_scheduler(&dir, Some("fuzz-image"), Some(runtime));

    let env = HashMap::from([("ASAN_OPTIONS".to_string(), "detect_leaks=0".to_string())]);
    let mut task = scheduler
        .start_task(fuzzer, [0u8; 32], Some(&env), None)
        .unwrap();
    task.child.wait().unwrap();

    let args: Vec<String> = fs::read_to_string(&args_file)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect();
    let image = args.iter().position(|a| a == "fuzz-image").unwrap();
    let value = args
        .iter()
        .position(|a| a == "ASAN_OPTIONS=detect_leaks=0")
        .unwrap();
    assert_eq!(args[value - 1], "-e");
    assert!(
        value < image,
        "the variable must precede the image: {args:?}"
    );
}

/// libFuzzer `-jobs` workers are descendants of the fuzzer, not the fuzzer
/// itself: stopping a task must not leave them running.
#[test]
fn killing_a_host_task_stops_its_descendants() {
    let dir = assert_fs::TempDir::new().unwrap();
    let pid_file = dir.path().join("worker.pid");
    let fuzzer = executable(
        &dir,
        "fuzzer",
        &format!(
            "#!/bin/sh\nsleep 300 &\necho $! > '{}'\nwait\n",
            pid_file.display()
        ),
    );
    let mut scheduler = test_scheduler(&dir, None, None);
    let mut task = scheduler.start_task(fuzzer, [0u8; 32], None, None).unwrap();

    let worker = wait_until(|| {
        fs::read_to_string(&pid_file)
            .ok()
            .and_then(|pid| pid.trim().parse::<u32>().ok())
    })
    .expect("the worker should have started");

    scheduler.kill_task(&mut task);
    task.child.wait().unwrap();

    let gone = wait_until(|| {
        let stat = fs::read_to_string(format!("/proc/{worker}/stat")).ok();
        // A zombie is dead, merely not reaped by an init that does not reap.
        let alive = stat.is_some_and(|s| !s.contains(") Z "));
        (!alive).then_some(())
    });
    assert!(gone.is_some(), "worker {worker} survived its fuzzer");
}

/// The state file sets the fuzzers' environment, so a copy that someone else
/// could have written must be refused rather than applied.
#[test]
fn a_state_file_writable_by_others_is_refused() {
    let dir = assert_fs::TempDir::new().unwrap();
    let scheduler = test_scheduler(&dir, None, None);
    let state = scheduler.state_file_path().unwrap().clone();

    let mode = fs::metadata(&state).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600, "the scheduler creates it private");
    assert!(scheduler.read_state_file().is_ok());

    fs::set_permissions(&state, fs::Permissions::from_mode(0o666)).unwrap();
    assert!(scheduler.read_state_file().is_err());
}

#[test]
fn a_state_file_symlink_is_refused() {
    let dir = assert_fs::TempDir::new().unwrap();
    let scheduler = test_scheduler(&dir, None, None);
    let state = scheduler.state_file_path().unwrap().clone();

    let planted = dir.child("planted.json");
    planted.write_str(r#"{"fuzzers":[]}"#).unwrap();
    fs::remove_file(&state).unwrap();
    std::os::unix::fs::symlink(planted.path(), &state).unwrap();

    assert!(scheduler.read_state_file().is_err());
    // Nor is anything written through it.
    scheduler.write_state_file().unwrap_err();
    assert_eq!(
        fs::read_to_string(planted.path()).unwrap(),
        r#"{"fuzzers":[]}"#
    );
}

fn wait_until<T>(mut probe: impl FnMut() -> Option<T>) -> Option<T> {
    for _ in 0..100 {
        if let Some(value) = probe() {
            return Some(value);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    None
}

#[test]
fn the_next_snapshot_is_one_interval_after_the_last() {
    let interval = Duration::from_secs(900);
    let start = Instant::now();
    let last = start + Duration::from_secs(60);
    assert_eq!(
        snapshot_due_at(start, Some(last), interval),
        Some(last + interval)
    );
}

/// Stop the tasks of this scheduler only: `kill_tasks` would also kill the
/// processes of the other tests, which share the scheduler name.
fn stop_tasks(scheduler: &mut Scheduler<DirectoryTargetsCollector>) {
    let shas: Vec<_> = scheduler.tasks.keys().copied().collect();
    for sha in shas {
        let mut task = scheduler.tasks.remove(&sha).unwrap();
        scheduler.kill_task(&mut task);
        let _ = task.child.wait();
    }
}

#[test]
fn the_runtime_of_a_binary_spans_its_restarts() {
    let dir = assert_fs::TempDir::new().unwrap();
    let targets = dir.child("targets");
    targets.create_dir_all().unwrap();
    let logs = dir.child("logs");
    logs.create_dir_all().unwrap();
    let binary = targets.child("fuzzer");
    binary.write_str("#!/bin/sh\nexec sleep 300\n").unwrap();
    fs::set_permissions(binary.path(), fs::Permissions::from_mode(0o755)).unwrap();

    let mut scheduler =
        scheduler_with_targets(&dir, None, None, vec![targets.path().to_path_buf()]);
    scheduler.update(logs.path().to_str().unwrap()).unwrap();
    let sha = *scheduler.tasks.keys().next().unwrap();

    std::thread::sleep(Duration::from_millis(600));
    scheduler.restart_task_by_sha(sha).unwrap();
    let first_run = scheduler.finished_runtime[&sha];
    assert!(first_run >= Duration::from_millis(600));

    std::thread::sleep(Duration::from_millis(300));
    assert!(scheduler.runtime_of(&sha) >= first_run + Duration::from_millis(300));
    stop_tasks(&mut scheduler);
}

#[test]
fn a_snapshot_measures_each_fuzzer_alone() {
    let dir = assert_fs::TempDir::new().unwrap();
    let targets = dir.child("targets");
    targets.create_dir_all().unwrap();
    let logs = dir.child("logs");
    logs.create_dir_all().unwrap();
    for name in ["fuzz_a", "fuzz_b"] {
        let binary = targets.child(name);
        binary
            .write_str(&format!("#!/bin/sh\n# {name}\nexec sleep 300\n"))
            .unwrap();
        fs::set_permissions(binary.path(), fs::Permissions::from_mode(0o755)).unwrap();
    }

    let mut scheduler =
        scheduler_with_targets(&dir, None, None, vec![targets.path().to_path_buf()]);
    scheduler.has_coverage = true;
    scheduler.update(logs.path().to_str().unwrap()).unwrap();
    let shas: HashSet<_> = scheduler.tasks.keys().copied().collect();

    let job = scheduler.snapshot_job(&shas).unwrap();
    assert_eq!(job.request.binaries.len(), 2);
    assert_eq!(job.fuzzers.len(), 2);
    for fuzzer in &job.fuzzers {
        let task = scheduler
            .tasks
            .iter()
            .find(|(sha, _)| hex::encode(sha) == fuzzer.sha)
            .map(|(_, task)| task)
            .unwrap();
        let name = task.path.file_name().unwrap();
        assert_eq!(fuzzer.request.binaries.len(), 1);
        assert_eq!(
            fuzzer.request.coverage_dir,
            dir.path().join("coverage").join(name)
        );
        assert_eq!(fuzzer.request.profdata_name, SNAPSHOT_PROFDATA);
    }
    stop_tasks(&mut scheduler);
}
