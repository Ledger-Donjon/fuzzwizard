// SPDX-FileCopyrightText: © 2026 LEDGER SAS
// SPDX-License-Identifier: MIT

//! Task lifecycle methods for [`Scheduler`]: start, stop, restart, update,
//! crash reporting, and status queries.

use std::collections::HashSet;
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Instant;
use std::fs;

use chrono::prelude::*;
use chrono::TimeDelta;
use humantime::format_duration;
use log::{info, warn};
use rand::{rng, Rng};
use serde::Serialize;

use crate::backend::http_client;
use crate::collector::Sha;
use crate::process::{kill_process_by_pid, remove_container, PROCESS_REGISTRY};
use crate::types::{CrashReport, ProcessRecord, Task, TaskStatus, TaskStatusInfo};
use crate::{validation, Result, Scheduler, SchedulerError, TargetsCollector};
use crate::HEARTBEAT_TIMEOUT;
use crate::backend::post_to_backend;

impl<C: TargetsCollector> Scheduler<C> {
    pub(crate) fn kill_task(&mut self, task: &mut Task) {
        if let Some(ref container_name) = task.container_name {
            if let Some(ref binary) = self.container_binary {
                info!("killing container: {container_name:?}");
                remove_container(binary, container_name);
            } else {
                warn!("Container name present but container binary not configured");
            }
        } else if task.child.try_wait().is_ok() {
            kill_process_by_pid(task.child.id());
        }
    }

    /// Update the scheduler with the latest targets collected by the collector.
    pub fn update(&mut self, logdir: &str) -> Result<()> {
        let latest_targets = self.collector.collect_targets();

        log::debug!("latest_targets: {latest_targets:?}");

        let old_targets_sha: HashSet<Sha> = self.targets.keys().copied().collect();
        let latest_targets_sha: HashSet<Sha> = latest_targets.keys().copied().collect();

        let targets_to_stop: Vec<Sha> = old_targets_sha
            .difference(&latest_targets_sha)
            .copied()
            .collect();
        let targets_to_start: Vec<Sha> = latest_targets_sha
            .difference(&old_targets_sha)
            .copied()
            .collect();
        let targets_to_check: Vec<Sha> = latest_targets_sha
            .intersection(&old_targets_sha)
            .copied()
            .collect();

        let log_targets =
            |label: &str, shas: &[Sha], targets_map: &std::collections::HashMap<Sha, PathBuf>| {
                info!("{label} (paths):");
                for sha in shas {
                    if let Some(path) = targets_map.get(sha) {
                        info!("- {}: {} : {:?}", label, hex::encode(sha), path.display());
                    } else {
                        info!("- {}: {} : [missing path]", label, hex::encode(sha));
                    }
                }
            };

        log_targets("STOP", &targets_to_stop, &self.targets);
        log_targets("CHECK", &targets_to_check, &self.targets);
        log_targets("START", &targets_to_start, &latest_targets);

        if (!targets_to_stop.is_empty() || !targets_to_start.is_empty())
            && self.spawn_coverage_snapshot(&old_targets_sha)
        {
            self.last_snapshot_at = Some(Instant::now());
        }

        for target in targets_to_stop {
            info!("stopping task: {target:x?}");
            if let Some(mut task) = self.tasks.remove(&target) {
                self.kill_task(&mut task);
                info!("removing task: {target:?}");
                let _ = self.write_state_file();
            } else {
                return Err(SchedulerError::InconsistentState(
                    "task missing during stop",
                ));
            }
        }

        let mut logfile = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(format!("{logdir}/running_tasks.log"))?;

        for target in targets_to_start {
            let Some(path) = latest_targets.get(&target) else {
                return Err(SchedulerError::InconsistentState(
                    "missing start target path",
                ));
            };
            let (var_env, cmd_options) = self.get_task_options_from_state_file(target)?;
            let fuzzer_path = path.canonicalize()?;
            let task =
                self.start_task(fuzzer_path, target, var_env.as_ref(), cmd_options.as_ref())?;
            self.tasks.insert(target, task);
            let current_date = Local::now().format("%Y-%m-%d-%H-%M-%S").to_string();
            writeln!(
                logfile,
                "[{}]{} : 0x{}",
                current_date,
                path.display(),
                hex::encode(target)
            )?;
            let _ = self.write_state_file();
        }

        for target in targets_to_check {
            if self.targets.get(&target) == latest_targets.get(&target) {
                continue;
            }

            if let Some(mut task) = self.tasks.remove(&target) {
                task.child.kill()?;
            } else {
                return Err(SchedulerError::InconsistentState(
                    "task missing during restart",
                ));
            }

            let Some(path) = latest_targets.get(&target) else {
                return Err(SchedulerError::InconsistentState(
                    "missing restart target path",
                ));
            };
            let (var_env, cmd_options) = self.get_task_options_from_state_file(target)?;
            let fuzzer_path = path.canonicalize()?;
            let task =
                self.start_task(fuzzer_path, target, var_env.as_ref(), cmd_options.as_ref())?;
            self.tasks.insert(target, task);
            let _ = self.write_state_file();
        }

        self.targets = latest_targets;
        Ok(())
    }

    pub(crate) fn start_task(
        &self,
        fuzzer_path: PathBuf,
        fuzzer_hash: Sha,
        var_env: Option<&std::collections::HashMap<String, String>>,
        cmd_options: Option<&Vec<String>>,
    ) -> Result<Task> {
        if let Some(env_vars) = var_env {
            for (key, value) in env_vars {
                validation::validate_env_var(key, value)?;
            }
        }

        if let Some(options) = cmd_options {
            for option in options {
                validation::validate_cmd_option(option)?;
            }
        }

        let fuzzer_name = fuzzer_path.file_name().and_then(|s| s.to_str()).ok_or(
            SchedulerError::InconsistentState("invalid fuzzer path name"),
        )?;
        let mut rng = rng();

        let fuzzer_corpus = self.corpus_directory.join(fuzzer_name);
        if !fuzzer_corpus.exists() {
            fs::create_dir_all(&fuzzer_corpus)?;
            let random_file = fuzzer_corpus.join("crash_init");
            let mut file = fs::File::create(random_file)?;
            let mut data: [u8; 8192] = [0u8; 8192];
            rng.fill(&mut data);
            file.write_all(&data)?;
        }
        let mut perms = fs::metadata(&fuzzer_corpus)?.permissions();
        perms.set_mode(0o770);
        fs::set_permissions(&fuzzer_corpus, perms)?;
        let fuzzer_crashdir = self.crashes_directory.join(fuzzer_name);
        if !fuzzer_crashdir.exists() {
            fs::create_dir_all(&fuzzer_crashdir)?;
        }
        let mut perms = fs::metadata(&fuzzer_crashdir)?.permissions();
        perms.set_mode(0o770);
        fs::set_permissions(&fuzzer_crashdir, perms)?;
        let fuzzer_coveragedir = self.coverage_directory.join(fuzzer_name);
        if !fuzzer_coveragedir.exists() {
            fs::create_dir_all(&fuzzer_coveragedir)?;
        }
        let mut perms = fs::metadata(&fuzzer_coveragedir)?.permissions();
        perms.set_mode(0o770);
        fs::set_permissions(&fuzzer_coveragedir, perms)?;
        let mut child: std::process::Child;
        let mut task_container_name: Option<String> = None;
        let sha_hex = hex::encode(fuzzer_hash);

        if let Some(ref container_image) = self.container_image {
            if let Some(ref binary) = self.container_binary {
                let mut random_name: [u8; 8] = [0u8; 8];

                rng.fill(&mut random_name);
                let name = format!("docker-fw-{}-{}", hex::encode(random_name), fuzzer_name);
                let mut args: Vec<String> = vec![
                    "run".into(),
                    "--rm".into(),
                    "--name".into(),
                    name.clone(),
                    "--network=none".into(),
                    "--cap-drop".into(),
                    "ALL".into(),
                    "--security-opt".into(),
                    "no-new-privileges".into(),
                    "--read-only".into(),
                    "--pids-limit".into(),
                    "512".into(),
                    "--tmpfs".into(),
                    "/tmp:rw,noexec,nosuid,size=256m".into(),
                    "-e".into(),
                    format!("LLVM_PROFILE_FILE=/fuzzer/coverage/{sha_hex}-%m.profraw"),
                    "-v".into(),
                    format!("{}:/fuzzer/binary:ro", fuzzer_path.display()),
                    "-v".into(),
                    format!("{}:/fuzzer/crash_dir/", fuzzer_crashdir.display()),
                    "-v".into(),
                    format!("{}:/fuzzer/corpus_dir/", fuzzer_corpus.display()),
                    "-v".into(),
                    format!("{}:/fuzzer/coverage/", fuzzer_coveragedir.display()),
                ];

                args.extend(
                    self.checked_container_options()
                        .map_err(SchedulerError::InvalidInput)?,
                );

                args.push(container_image.clone());

                args.push("/fuzzer/binary".into());
                args.push("/fuzzer/corpus_dir/".into());
                args.push("-artifact_prefix=/fuzzer/crash_dir/".into());

                if let Some(custom_opts) = cmd_options {
                    args.extend(custom_opts.iter().cloned());
                }

                if let Some(env_vars) = var_env {
                    for (key, value) in env_vars {
                        args.push("-e".into());
                        args.push(format!("{key}={value}"));
                    }
                }
                let mut cmd = Command::new(binary);
                cmd.args(args.iter().map(|s| s.as_str()));

                child = cmd.stderr(Stdio::piped()).spawn()?;
                if let Some(reg) = PROCESS_REGISTRY.get() {
                    if let Ok(mut v) = reg.lock() {
                        v.push(ProcessRecord {
                            pid: child.id(),
                            container_name: Some(name.clone()),
                            fuzzer_name: fuzzer_name.to_string(),
                            scheduler_name: self.scheduler_name.clone(),
                        });
                    }
                }
                task_container_name = Some(name);
            } else {
                return Err(SchedulerError::MissingContainerBinary);
            }
        } else {
            let mut args = vec![
                fuzzer_corpus.display().to_string(),
                format!("-artifact_prefix={}{}", fuzzer_crashdir.display(), "/"),
            ];

            if self.has_coverage {
                args.push(format!(
                    "-LLVM_PROFILE_FILE={}/{}-%m.profraw",
                    fuzzer_coveragedir.display(),
                    sha_hex,
                ));
            }

            if let Some(custom_opts) = cmd_options {
                args.extend(custom_opts.iter().cloned());
            }

            let mut cmd = Command::new(&fuzzer_path);
            cmd.args(&args);

            if let Some(env_vars) = var_env {
                for (key, value) in env_vars {
                    cmd.env(key, value);
                }
            }

            if self.has_coverage {
                cmd.env(
                    "LLVM_PROFILE_FILE",
                    format!("{}/{}-%m.profraw", fuzzer_coveragedir.display(), sha_hex),
                );
            }

            child = cmd.stderr(Stdio::piped()).spawn()?;
            info!("Child process started: {:?}", child.id());
            if let Some(reg) = PROCESS_REGISTRY.get() {
                if let Ok(mut v) = reg.lock() {
                    v.push(ProcessRecord {
                        pid: child.id(),
                        container_name: None,
                        fuzzer_name: fuzzer_name.to_string(),
                        scheduler_name: self.scheduler_name.clone(),
                    });
                }
            }
        }
        let Some(child_stderr) = child.stderr.take() else {
            return Err(SchedulerError::MissingChildStderr);
        };

        let profraw_path = if self.has_coverage {
            let coverage_bin_path = fuzzer_coveragedir.join(format!("fuzzer_{sha_hex}.bin"));
            if let Err(e) = fs::copy(&fuzzer_path, &coverage_bin_path) {
                warn!(
                    "Failed to copy binary to coverage directory ({}): {}",
                    coverage_bin_path.display(),
                    e
                );
            }
            Some(fuzzer_coveragedir.join(format!("{sha_hex}-%m.profraw")))
        } else {
            None
        };

        Ok(Task {
            child,
            stderr: child_stderr,
            path: fuzzer_path,
            started_at: Instant::now(),
            container_name: task_container_name,
            profraw_path,
            status: TaskStatus::Running,
            crashed_at: None,
            stopped_at: None,
        })
    }

    pub fn kill_tasks(&mut self) {
        info!("Killing tasks for scheduler: {}", self.scheduler_name);
        let registry = PROCESS_REGISTRY
            .get_or_init(|| std::sync::Arc::new(std::sync::Mutex::new(Vec::new())))
            .clone();

        let all_records: Vec<ProcessRecord> = {
            match registry.lock() {
                Ok(guard) => guard.clone(),
                Err(e) => {
                    warn!("Failed to lock process registry: {e}");
                    return;
                }
            }
        };

        let records: Vec<ProcessRecord> = all_records
            .into_iter()
            .filter(|rec| rec.scheduler_name == self.scheduler_name)
            .collect();

        for rec in records.iter() {
            info!(
                "Killing task: {:?} (scheduler: {})",
                rec.fuzzer_name, rec.scheduler_name
            );
            if let Some(name) = &rec.container_name {
                if let Some(ref binary) = self.container_binary {
                    remove_container(binary, name);
                }
            }
            kill_process_by_pid(rec.pid);
        }

        if let Some(reg) = PROCESS_REGISTRY.get() {
            if let Ok(mut guard) = reg.lock() {
                guard.retain(|rec| rec.scheduler_name != self.scheduler_name);
            }
        }

        let mut status_changed = false;
        for (_sha, task) in self.tasks.iter_mut() {
            if task.status == TaskStatus::Running {
                task.status = TaskStatus::Stopped;
                task.stopped_at = Some(Instant::now());
                info!("Task {:?} marked as Stopped", task.path);
                status_changed = true;
            }
        }
        if status_changed {
            let _ = self.write_state_file();
        }
    }

    pub fn report_crashes(&mut self) -> Result<()> {
        let mut crashed_tasks = Vec::new();
        for (_hash, task) in self.tasks.iter_mut() {
            if task.status != TaskStatus::Running {
                continue;
            }

            match task.child.try_wait() {
                Ok(None) => {}
                Ok(Some(_)) => {
                    info!("CRASH - task has exited: {:?}", task.path.display());
                    task.status = TaskStatus::Crashed;
                    task.crashed_at = Some(Instant::now());
                    crashed_tasks.push(true);

                    let mut stderr = String::new();
                    task.stderr.read_to_string(&mut stderr)?;
                    info!(
                        "crash task: {:?}, {:?}, {:?}, {:?}",
                        task,
                        task.child.id(),
                        task.child.stdout,
                        task.child.stderr
                    );
                    // The stderr of a fuzz target is untrusted input: it may
                    // hold an `artifact_prefix=` line without the artifact
                    // path, so the path is only read when the marker is there.
                    const PATTERN: &str = "Test unit written to ";
                    let mut crash_corpus = stderr
                        .lines()
                        .filter(|l| l.starts_with("artifact_prefix="))
                        .find_map(|l| l.find(PATTERN).map(|idx| &l[idx + PATTERN.len()..]))
                        .unwrap_or_default()
                        .to_string();

                    if crash_corpus.is_empty() {
                        warn!(
                            "No crash artifact path in stderr of {:?}",
                            task.path.display()
                        );
                    }

                    if crash_corpus.starts_with("/fuzzer/crash_dir/") {
                        let fuzzer_name = task.path.file_name().and_then(|s| s.to_str()).ok_or(
                            SchedulerError::InconsistentState("invalid fuzzer path name"),
                        )?;
                        let filename = &crash_corpus["/fuzzer/crash_dir/".len()..];
                        if filename.contains("..") {
                            return Err(SchedulerError::PathTraversal(format!(
                                "Path traversal detected in crash path: {crash_corpus}"
                            )));
                        }
                        let candidate = self.crashes_directory.join(fuzzer_name).join(filename);
                        let canonical_crash_dir = self
                            .crashes_directory
                            .canonicalize()
                            .map_err(SchedulerError::Io)?;
                        let canonical_candidate =
                            candidate.canonicalize().map_err(SchedulerError::Io)?;
                        if !canonical_candidate.starts_with(&canonical_crash_dir) {
                            return Err(SchedulerError::PathTraversal(format!(
                                "Crash path escapes crash directory: {}",
                                canonical_candidate.display()
                            )));
                        }
                        crash_corpus = canonical_candidate.to_string_lossy().to_string();
                    }
                    info!("crash_corpus: {crash_corpus:?}");
                    let crash_corpus_path = crash_corpus.clone();
                    let crashing_instance = match fs::read(&crash_corpus) {
                        Ok(data) => data,
                        Err(e) => {
                            warn!("Failed to read crash corpus from {crash_corpus_path:?}: {e}");
                            b"NO CRASH INFO - Failed to read crash corpus".to_vec()
                        }
                    };

                    let binary = fs::read(task.path.clone())?;
                    let request = http_client()
                        .post(format!("{}/crashes", self.backend_url))
                        .json(&CrashReport {
                            report: stderr.as_bytes().to_vec(),
                            input: crashing_instance,
                            filename: task.path.display().to_string(),
                            program: binary,
                            scheduler_name: self.scheduler_name.clone(),
                        });
                    let res = request.send()?;

                    let status = res.status();
                    if status.is_success() {
                        info!("Successfully uploaded crash to backend: {status}");
                    } else {
                        warn!(
                            "Backend returned error status: {} - {}",
                            status,
                            res.text().unwrap_or_default()
                        );
                    }
                }
                Err(e) => return Err(SchedulerError::Io(e)),
            }
        }

        if !crashed_tasks.is_empty() {
            let _ = self.write_state_file();
        }

        Ok(())
    }

    pub fn get_tasks_with_status(&mut self) -> Vec<TaskStatusInfo> {
        let mut result: Vec<TaskStatusInfo> = Vec::new();

        for (hash, task) in self.tasks.iter_mut() {
            if task.status != TaskStatus::Running {
                result.push(TaskStatusInfo {
                    sha: *hash,
                    path: task.path.clone(),
                    status: task.status,
                    started_at: task.started_at,
                    crashed_at: task.crashed_at,
                    stopped_at: task.stopped_at,
                });
                continue;
            }
            match task.child.try_wait() {
                Ok(None) => {
                    result.push(TaskStatusInfo {
                        sha: *hash,
                        path: task.path.clone(),
                        status: task.status,
                        started_at: task.started_at,
                        crashed_at: task.crashed_at,
                        stopped_at: task.stopped_at,
                    });
                }
                Ok(Some(_)) => {
                    let crashed_at = Some(Instant::now());
                    result.push(TaskStatusInfo {
                        sha: *hash,
                        path: task.path.clone(),
                        status: TaskStatus::Crashed,
                        started_at: task.started_at,
                        crashed_at,
                        stopped_at: task.stopped_at,
                    });
                }
                Err(_) => {
                    result.push(TaskStatusInfo {
                        sha: *hash,
                        path: task.path.clone(),
                        status: task.status,
                        started_at: task.started_at,
                        crashed_at: task.crashed_at,
                        stopped_at: task.stopped_at,
                    });
                }
            }
        }
        result
    }

    pub fn log_running_tasks(&self) {
        if self.tasks.is_empty() {
            info!("No tasks running");
            return;
        }
        info!("Running tasks:");
        for (sha, task) in self.tasks.iter() {
            let secs: u64 = task.started_at.elapsed().as_secs();
            match TimeDelta::new(secs as i64, 0) {
                Some(d) => match d.to_std() {
                    Ok(std_duration) => {
                        info!(
                            "- '{}:0x{}' has been running for {:?}",
                            task.path.display(),
                            hex::encode(sha),
                            format_duration(std_duration).to_string()
                        );
                    }
                    Err(e) => {
                        info!(
                            "- '{}:0x{}' has been running for {} seconds (duration conversion error: {})",
                            task.path.display(),
                            hex::encode(sha),
                            secs,
                            e
                        );
                    }
                },
                None => {
                    info!(
                        "- '{}:0x{}' has been running for {} seconds (invalid time delta)",
                        task.path.display(),
                        hex::encode(sha),
                        secs
                    );
                }
            }
        }
    }

    /// Report the status of every known fuzzer to the backend.
    pub fn report_status(&self) {
        #[derive(Serialize)]
        struct FuzzerStatusInput {
            sha: String,
            path: String,
            status: &'static str,
            started_at: Option<NaiveDateTime>,
        }

        #[derive(Serialize)]
        struct HeartbeatPayload {
            scheduler_name: String,
            fuzzers: Vec<FuzzerStatusInput>,
        }

        let now = Local::now().naive_local();
        let mut fuzzers = Vec::with_capacity(self.targets.len().max(self.tasks.len()));

        for (sha, task) in &self.tasks {
            let status = match task.status {
                TaskStatus::Running => "running",
                TaskStatus::Crashed => "crashed",
                TaskStatus::Stopped => "stopped",
            };
            let started_at = TimeDelta::from_std(task.started_at.elapsed())
                .ok()
                .map(|elapsed| now - elapsed);
            fuzzers.push(FuzzerStatusInput {
                sha: hex::encode(sha),
                path: task.path.display().to_string(),
                status,
                started_at,
            });
        }

        for (sha, path) in &self.targets {
            if !self.tasks.contains_key(sha) {
                fuzzers.push(FuzzerStatusInput {
                    sha: hex::encode(sha),
                    path: path.display().to_string(),
                    status: "stopped",
                    started_at: None,
                });
            }
        }

        let payload = HeartbeatPayload {
            scheduler_name: self.scheduler_name.clone(),
            fuzzers,
        };

        post_to_backend(
            &self.backend_url,
            "fuzzer_status",
            &payload,
            HEARTBEAT_TIMEOUT,
            &self.scheduler_name,
        );
    }

    /// Start a task by SHA.
    pub fn start_task_by_sha(&mut self, sha: Sha) -> Result<()> {
        if let Some(task) = self.tasks.get(&sha) {
            if task.status == TaskStatus::Running {
                return Err(SchedulerError::InvalidInput(
                    "Task is already running".to_string(),
                ));
            }
        }

        let task_path = self.start_task_with_state_file_options(sha)?;
        info!("Task started: {task_path:?}");
        Ok(())
    }

    /// Stop a task by SHA.
    pub fn stop_task_by_sha(&mut self, sha: Sha) -> Result<()> {
        if let Some(mut task) = self.tasks.remove(&sha) {
            if task.status == TaskStatus::Running {
                let task_path = task.path.clone();
                self.kill_task(&mut task);
                task.status = TaskStatus::Stopped;
                task.stopped_at = Some(Instant::now());
                self.tasks.insert(sha, task);
                info!("Task stopped: {task_path:?}");
                let _ = self.write_state_file();
                Ok(())
            } else {
                self.tasks.insert(sha, task);
                Err(SchedulerError::InvalidInput(
                    "Task is not running".to_string(),
                ))
            }
        } else {
            Err(SchedulerError::InvalidInput("Task not found".to_string()))
        }
    }

    /// Restart a task by SHA.
    pub fn restart_task_by_sha(&mut self, sha: Sha) -> Result<()> {
        if let Some(mut task) = self.tasks.remove(&sha) {
            if task.status == TaskStatus::Running {
                self.kill_task(&mut task);
            }
        }

        let task_path = self.start_task_with_state_file_options(sha)?;
        info!("Task restarted: {:?}", task_path.display());
        Ok(())
    }
}
