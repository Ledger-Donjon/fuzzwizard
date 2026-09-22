// SPDX-FileCopyrightText: © 2026 LEDGER SAS
// SPDX-License-Identifier: MIT

//! Background execution of the build script that produces the fuzzer binaries.
//!
//! Building a fuzzing corpus of targets routinely takes minutes. Running the
//! script inline stalls everything the caller does on the same thread: crash
//! reporting, coverage collection, and in the TUI the key presses too, because
//! the launcher lock is held for the whole build. [`Builder`] therefore runs the
//! script on its own thread and exposes a `poll` that never blocks.

use std::process::{Command, Stdio};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use log::info;

use crate::validation;

/// How a build ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildOutcome {
    Success,
    /// The script reported a failure, or could not be run at all. Callers
    /// decide how loudly to report it.
    Failure(String),
}

/// Drives the build script of one scheduler without ever blocking its caller.
pub struct Builder {
    scheduler_name: String,
    script: String,
    interval: Option<Duration>,
    last_build_at: Option<Instant>,
    started_at: Option<Instant>,
    /// The build in flight. A join handle rather than a channel, because the
    /// launcher that owns the builder is shared across threads and a
    /// `mpsc::Receiver` is not `Sync`.
    running: Option<JoinHandle<BuildOutcome>>,
}

impl Builder {
    /// An interval of `None` or `0` hours rebuilds on every poll, which is what
    /// a scheduler without `build_script_interval_hours` has always done.
    pub fn new(scheduler_name: String, script: String, interval_hours: Option<u64>) -> Self {
        let interval = match interval_hours {
            Some(hours) if hours > 0 => Some(Duration::from_secs(hours.saturating_mul(3600))),
            _ => None,
        };

        Builder {
            scheduler_name,
            script,
            interval,
            last_build_at: None,
            started_at: None,
            running: None,
        }
    }

    /// When the last build finished, if one ever did.
    pub fn last_build_at(&self) -> Option<Instant> {
        self.last_build_at
    }

    /// When the build in flight started, if one is running.
    pub fn running_since(&self) -> Option<Instant> {
        self.started_at
    }

    /// Advance the build: collect a finished one, and start the next when it is
    /// due. Returns the outcome of a build that ended since the last call.
    ///
    /// Must be called regularly, since this is the only place where a finished
    /// build is noticed.
    pub fn poll(&mut self) -> Option<BuildOutcome> {
        if let Some(outcome) = self.collect_finished() {
            return Some(outcome);
        }
        if self.running.is_some() || !self.is_due() {
            return None;
        }
        self.start()
    }

    /// Outcome of the running build, or `None` while it is still going.
    fn collect_finished(&mut self) -> Option<BuildOutcome> {
        if !self.running.as_ref()?.is_finished() {
            return None;
        }

        // The thread has ended, so this join returns without waiting.
        let outcome = self.running.take()?.join().unwrap_or_else(|_| {
            BuildOutcome::Failure(format!("Build thread for {} panicked", self.scheduler_name))
        });

        self.started_at = None;
        self.last_build_at = Some(Instant::now());

        Some(outcome)
    }

    fn is_due(&self) -> bool {
        if self.script.trim().is_empty() {
            return false;
        }
        match (self.interval, self.last_build_at) {
            // Nothing has been built yet: targets are needed now, not in N hours.
            (_, None) => true,
            (None, Some(_)) => true,
            (Some(interval), Some(last)) => last.elapsed() >= interval,
        }
    }

    /// Spawn the build thread, or report why it could not even start.
    fn start(&mut self) -> Option<BuildOutcome> {
        let script = self.script.trim().to_string();
        let name = self.scheduler_name.clone();

        if let Err(e) = validation::validate_build_script(&script) {
            // A rejected script stays rejected until the config changes, so the
            // attempt is recorded to keep the interval from replaying it on
            // every single poll.
            self.last_build_at = Some(Instant::now());
            return Some(BuildOutcome::Failure(format!(
                "Build script rejected for {name}: {e}"
            )));
        }

        info!("Executing build script for {name}: {script}");
        let spawned = thread::Builder::new()
            .name(format!("build-{name}"))
            .spawn(move || run_build_script(&script, &name));

        match spawned {
            Ok(handle) => {
                self.started_at = Some(Instant::now());
                self.running = Some(handle);
                None
            }
            Err(e) => {
                self.last_build_at = Some(Instant::now());
                Some(BuildOutcome::Failure(format!(
                    "Failed to spawn build thread for {}: {e}",
                    self.scheduler_name
                )))
            }
        }
    }
}

/// Run the script to completion. Its output is dropped: it is a long-running
/// build, and the scheduler logs are not the place for it.
fn run_build_script(script: &str, scheduler_name: &str) -> BuildOutcome {
    let status = Command::new(script)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();

    match status {
        Ok(status) if status.success() => {
            info!("build_script ran successfully for {scheduler_name}");
            BuildOutcome::Success
        }
        Ok(status) => BuildOutcome::Failure(format!(
            "build_script exited with status {status} for {scheduler_name}"
        )),
        Err(e) => BuildOutcome::Failure(format!(
            "Failed to run build_script for {scheduler_name}: {e}"
        )),
    }
}
