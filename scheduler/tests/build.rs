// SPDX-FileCopyrightText: © 2026 LEDGER SAS
// SPDX-License-Identifier: MIT

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::thread::sleep;
use std::time::Duration;

use assert_fs::prelude::*;
use scheduler::build::{BuildOutcome, Builder};

/// Write an executable script and return its absolute path, since the validator
/// only accepts absolute paths to executable files.
fn executable_script(dir: &assert_fs::TempDir, name: &str, body: &str) -> String {
    let script = dir.child(name);
    script.write_str(body).unwrap();
    std::fs::set_permissions(script.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    script.path().to_string_lossy().into_owned()
}

/// Block until the build reports, so the assertions do not race the thread.
fn wait_for_outcome(builder: &mut Builder) -> BuildOutcome {
    for _ in 0..200 {
        if let Some(outcome) = builder.poll() {
            return outcome;
        }
        sleep(Duration::from_millis(25));
    }
    panic!("build did not finish in time");
}

#[test]
fn a_scheduler_without_build_script_never_builds() {
    let mut builder = Builder::new("no_script".to_string(), "   ".to_string(), None);

    assert!(builder.poll().is_none());
    assert!(builder.running_since().is_none());
    assert!(builder.last_build_at().is_none());
}

/// The point of the module: `poll` hands back control immediately, long before
/// the script is done.
#[test]
fn poll_returns_while_the_script_is_still_running() {
    let dir = assert_fs::TempDir::new().unwrap();
    let script = executable_script(&dir, "slow.sh", "#!/bin/sh\nsleep 2\n");

    let mut builder = Builder::new("slow".to_string(), script, None);

    let before = std::time::Instant::now();
    assert!(builder.poll().is_none(), "no outcome while building");
    assert!(
        before.elapsed() < Duration::from_secs(1),
        "poll blocked for {:?}",
        before.elapsed()
    );
    assert!(builder.running_since().is_some(), "build should be running");
}

#[test]
fn a_successful_build_is_reported_once() {
    let dir = assert_fs::TempDir::new().unwrap();
    let script = executable_script(&dir, "ok.sh", "#!/bin/sh\nexit 0\n");

    let mut builder = Builder::new("ok".to_string(), script, Some(24));

    assert_eq!(wait_for_outcome(&mut builder), BuildOutcome::Success);
    assert!(builder.last_build_at().is_some());
    assert!(builder.running_since().is_none());
    assert!(
        builder.poll().is_none(),
        "the 24h interval forbids an immediate rebuild"
    );
}

#[test]
fn a_failing_build_reports_its_exit_status() {
    let dir = assert_fs::TempDir::new().unwrap();
    let script = executable_script(&dir, "ko.sh", "#!/bin/sh\nexit 3\n");

    let mut builder = Builder::new("ko".to_string(), script, Some(24));

    match wait_for_outcome(&mut builder) {
        BuildOutcome::Failure(msg) => assert!(msg.contains("ko"), "unexpected message: {msg}"),
        BuildOutcome::Success => panic!("a script exiting with 3 is not a success"),
    }
}

/// A rejected script must be reported without being retried on every poll,
/// otherwise the backend is flooded with the same admin event.
#[test]
fn a_rejected_script_is_reported_once_per_interval() {
    let mut builder = Builder::new("relative".to_string(), "./build.sh".to_string(), Some(24));

    match builder.poll() {
        Some(BuildOutcome::Failure(msg)) => {
            assert!(msg.contains("relative"), "unexpected message: {msg}")
        }
        other => panic!("expected a rejection, got {other:?}"),
    }
    assert!(builder.poll().is_none(), "the rejection must not repeat");
}

#[test]
fn a_missing_script_is_reported_as_a_failure() {
    let missing = Path::new("/nonexistent/fuzzwizard/build.sh");
    let mut builder = Builder::new(
        "missing".to_string(),
        missing.to_string_lossy().into_owned(),
        Some(24),
    );

    match builder.poll() {
        Some(BuildOutcome::Failure(msg)) => {
            assert!(msg.contains("missing"), "unexpected message: {msg}")
        }
        other => panic!("expected a failure, got {other:?}"),
    }
}

/// Without a configured interval the script is rebuilt as soon as the previous
/// run ends, which is what the schedulers relied on before builds went async.
#[test]
fn without_an_interval_the_build_restarts_immediately() {
    let dir = assert_fs::TempDir::new().unwrap();
    let script = executable_script(&dir, "quick.sh", "#!/bin/sh\nexit 0\n");

    let mut builder = Builder::new("quick".to_string(), script, None);

    assert_eq!(wait_for_outcome(&mut builder), BuildOutcome::Success);
    assert!(builder.poll().is_none());
    assert!(
        builder.running_since().is_some(),
        "a new build should have started"
    );
}
