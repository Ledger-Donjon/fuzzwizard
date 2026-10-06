// SPDX-FileCopyrightText: © 2026 LEDGER SAS
// SPDX-License-Identifier: MIT

//! Unit tests for the private helpers of the crate root.

use std::time::Duration;

use assert_fs::prelude::*;

use super::process::resolve_profraw_paths;
use super::{snapshot_interval_from, DEFAULT_SNAPSHOT_INTERVAL_MINUTES};

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
    assert!(snapshot_interval_from(Some(u64::MAX)).is_some());
}
