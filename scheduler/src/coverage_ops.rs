// SPDX-FileCopyrightText: © 2026 LEDGER SAS
// SPDX-License-Identifier: MIT

//! Coverage-related methods for [`Scheduler`]: runner construction, queries,
//! and periodic snapshot logic.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::thread;
use std::time::Instant;

use log::{debug, info, warn};
use sha3::{Digest, Sha3_256};

use crate::backend::SnapshotJob;
use crate::collector::Sha;
use crate::coverage;
use crate::process::{flush_coverage, fuzzer_name_of, resolve_profraw_paths};
use crate::types::{Task, TaskStatus};
use crate::{validation, Scheduler, TargetsCollector};
use crate::{COVERAGE_FLUSH_TIMEOUT, FIRST_SNAPSHOT_DELAY, PROJECT_PROFDATA, SNAPSHOT_PROFDATA};

impl<C: TargetsCollector> Scheduler<C> {
    pub fn collect_coverage(&self) {
        if !self.has_coverage {
            return;
        }

        for task in self.tasks.values() {
            if task.status != TaskStatus::Running {
                continue;
            }

            info!("collecting coverage for: {:?}", task.path);

            match (&task.container_name, &self.container_binary) {
                (Some(name), Some(binary)) => {
                    let mut command = Command::new(binary);
                    command.args(["exec", name, "pkill", "-SIGUSR1", "-f", "binary"]);
                    flush_coverage(
                        command,
                        &format!("container {name}"),
                        COVERAGE_FLUSH_TIMEOUT,
                    );
                }
                (Some(name), None) => {
                    warn!("Cannot flush coverage of container {name}: no container binary set");
                }
                (None, _) => {
                    let mut command = Command::new("/usr/bin/pkill");
                    let fuzzer_name = fuzzer_name_of(&task.path);
                    command.args(["-SIGUSR1", "-f", fuzzer_name]);
                    flush_coverage(
                        command,
                        &format!("fuzzer {fuzzer_name}"),
                        COVERAGE_FLUSH_TIMEOUT,
                    );
                }
            }
        }
    }

    /// Build the appropriate [`coverage::CoverageRunner`] for this scheduler.
    fn coverage_runner(&self) -> std::result::Result<coverage::CoverageRunner, String> {
        match (&self.container_image, &self.container_binary) {
            (Some(image), Some(bin)) => Ok(coverage::CoverageRunner::Container {
                binary: bin.clone(),
                image: image.clone(),
                options: self.checked_container_options()?,
                llvm_bin_dir: self.llvm_bin_directory.clone(),
            }),
            (Some(image), None) => Err(format!(
                "Coverage: container image '{image}' configured but no container_binary set in config.json"
            )),
            (None, _) => Ok(coverage::CoverageRunner::Host {
                llvm_bin_dir: self.llvm_bin_directory.clone(),
            }),
        }
    }

    /// Configured container options, rejected as a whole if any of them would
    /// weaken the sandbox.
    pub(crate) fn checked_container_options(&self) -> std::result::Result<Vec<String>, String> {
        let checked: Vec<String> = self
            .container_options
            .iter()
            .flatten()
            .filter(|option| !option.is_empty())
            .cloned()
            .collect();
        validation::validate_container_options(&checked).map_err(|e| e.to_string())?;
        Ok(checked)
    }

    /// Path of the instrumented copy kept for coverage, falling back to the
    /// running binary when the copy is missing.
    fn instrumented_binary(&self, sha: &Sha, task: &Task) -> Option<PathBuf> {
        let copy = self
            .task_coverage_directory(task)
            .join(format!("fuzzer_{}.bin", hex::encode(sha)));
        if copy.exists() {
            Some(copy)
        } else {
            task.path.exists().then(|| task.path.clone())
        }
    }

    fn task_coverage_directory(&self, task: &Task) -> PathBuf {
        self.coverage_directory.join(fuzzer_name_of(&task.path))
    }

    fn coverage_request<'a>(
        &self,
        tasks: impl Iterator<Item = (&'a Sha, &'a Task)>,
        coverage_dir: PathBuf,
        profdata_name: &str,
    ) -> std::result::Result<coverage::CoverageRequest, String> {
        let runner = self.coverage_runner()?;
        let mut profraw_paths = Vec::new();
        let mut binaries = Vec::new();
        let mut source_anchors = Vec::new();

        for (sha, task) in tasks {
            if let Some(ref pattern) = task.profraw_path {
                profraw_paths.extend(resolve_profraw_paths(pattern));
            }
            binaries.extend(self.instrumented_binary(sha, task));
            source_anchors.push(task.path.clone());
        }

        Ok(coverage::CoverageRequest {
            coverage_dir,
            profdata_name: profdata_name.to_string(),
            profraw_paths,
            binaries,
            source_anchors,
            runner,
        })
    }

    fn task_coverage_request(
        &self,
        sha: &Sha,
    ) -> std::result::Result<coverage::CoverageRequest, String> {
        if !self.has_coverage {
            return Err("Coverage is not enabled for this scheduler".to_string());
        }
        let task = self
            .tasks
            .get(sha)
            .ok_or_else(|| format!("Task not found for SHA: 0x{}", hex::encode(sha)))?;

        self.coverage_request(
            std::iter::once((sha, task)),
            self.task_coverage_directory(task),
            "coverage.profdata",
        )
    }

    fn project_coverage_request(
        &self,
        profdata_name: &str,
    ) -> std::result::Result<coverage::CoverageRequest, String> {
        if !self.has_coverage {
            return Err("Coverage is not enabled for this scheduler".to_string());
        }
        self.coverage_request(
            self.tasks.iter(),
            self.coverage_directory.clone(),
            profdata_name,
        )
    }

    /// Coverage summary for a single fuzzer, or the reason there is none.
    pub fn get_task_coverage(&self, sha: &Sha) -> String {
        match self.task_coverage_request(sha) {
            Ok(request) => {
                let title = format!("Coverage Analysis for: 0x{}", hex::encode(sha));
                request.report(&title)
            }
            Err(e) => e,
        }
    }

    /// Coverage summary merging every fuzzer this scheduler manages.
    pub fn get_project_coverage(&self) -> String {
        match self.project_coverage_request(PROJECT_PROFDATA) {
            Ok(request) => {
                let title = format!(
                    "Project Coverage ({} fuzzer{})",
                    request.binaries.len(),
                    if request.binaries.len() == 1 { "" } else { "s" }
                );
                request.report(&title)
            }
            Err(e) => e,
        }
    }

    /// Project-wide annotated source for one file, or for all of them when
    /// `file_path` is `None`.
    pub fn get_project_file_coverage(&self, file_path: Option<&Path>) -> String {
        match self.project_coverage_request(PROJECT_PROFDATA) {
            Ok(request) => request.annotated_source(file_path),
            Err(e) => e,
        }
    }

    /// Compute a canonical fingerprint for the current set of target binaries.
    fn compute_build_fingerprint(shas: &HashSet<Sha>) -> String {
        let mut sorted: Vec<Sha> = shas.iter().copied().collect();
        sorted.sort_unstable();
        let mut hasher = Sha3_256::new();
        for sha in sorted {
            hasher.update(sha);
        }
        hex::encode(hasher.finalize())
    }

    /// Take a coverage snapshot if the configured interval has elapsed.
    pub fn snapshot_coverage_if_due(&mut self) {
        let Some(interval) = self.snapshot_interval else {
            return;
        };

        let due = match self.last_snapshot_at {
            None => self.started_at + FIRST_SNAPSHOT_DELAY.min(interval),
            Some(prev) => prev + interval,
        };
        if Instant::now() < due {
            return;
        }

        let shas: HashSet<Sha> = self.tasks.keys().copied().collect();
        if self.spawn_coverage_snapshot(&shas) {
            self.last_snapshot_at = Some(Instant::now());
        }
    }

    /// Capture and send a coverage snapshot to the backend, off the scheduler thread.
    pub(crate) fn spawn_coverage_snapshot(&self, shas: &HashSet<Sha>) -> bool {
        if !self.has_coverage || self.tasks.is_empty() {
            return false;
        }

        if self.snapshot_running.swap(true, Ordering::SeqCst) {
            debug!("Coverage snapshot skipped: a previous one is still running");
            return false;
        }

        let request = match self.project_coverage_request(SNAPSHOT_PROFDATA) {
            Ok(r) => r,
            Err(e) => {
                debug!("Coverage snapshot skipped: {e}");
                self.snapshot_running.store(false, Ordering::SeqCst);
                return false;
            }
        };

        let job = SnapshotJob {
            request,
            fingerprint: Self::compute_build_fingerprint(shas),
            num_fuzzers: self.tasks.len() as i32,
            scheduler_name: self.scheduler_name.clone(),
            backend_url: self.backend_url.clone(),
        };

        let running = Arc::clone(&self.snapshot_running);
        thread::spawn(move || {
            job.run();
            running.store(false, Ordering::SeqCst);
        });
        true
    }

    /// Annotated source for one file of a fuzzer.
    pub fn get_task_file_coverage(&self, sha: &Sha, file_path: Option<&Path>) -> String {
        info!("Start file coverage for: {file_path:?}");
        match self.task_coverage_request(sha) {
            Ok(request) => request.annotated_source(file_path),
            Err(e) => e,
        }
    }
}
