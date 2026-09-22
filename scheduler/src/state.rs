// SPDX-FileCopyrightText: © 2026 LEDGER SAS
// SPDX-License-Identifier: MIT

//! State file read/write methods for [`Scheduler`].

use std::collections::HashMap;
use std::path::Path;
use std::{fs, io};

use log::info;

use crate::collector::Sha;
use crate::types::{
    FuzzerStateCommand, FuzzerStateEntry, StateFile, TaskOptionsFromState, TaskStatus,
};
use crate::{validation, Result, Scheduler, SchedulerError, TargetsCollector};

impl<C: TargetsCollector> Scheduler<C> {
    /// Get var_env and cmd_options for a task from the state file
    pub(crate) fn get_task_options_from_state_file(
        &self,
        sha: Sha,
    ) -> Result<TaskOptionsFromState> {
        let sha_str = hex::encode(sha);
        if let Some(state_file) = self.read_state_file()? {
            for entry in &state_file.fuzzers {
                if entry.sha == sha_str {
                    return Ok((entry.var_env.clone(), entry.cmd_options.clone()));
                }
            }
        }
        Ok((None, None))
    }

    /// Internal helper to start a task by SHA with options from state file.
    /// Returns the task path for logging purposes.
    pub(crate) fn start_task_with_state_file_options(
        &mut self,
        sha: Sha,
    ) -> Result<std::path::PathBuf> {
        let path = self
            .targets
            .get(&sha)
            .ok_or_else(|| SchedulerError::InvalidInput("Task not found in targets".to_string()))?;

        let fuzzer_path = path.canonicalize()?;
        let task_path = fuzzer_path.clone();

        let (var_env, cmd_options) = self.get_task_options_from_state_file(sha)?;

        let task = self.start_task(fuzzer_path, sha, var_env.as_ref(), cmd_options.as_ref())?;
        self.tasks.insert(sha, task);

        let _ = self.write_state_file();

        Ok(task_path)
    }

    pub(crate) fn read_state_file(&self) -> Result<Option<StateFile>> {
        let Some(ref state_path) = self.state_file_path else {
            return Ok(None);
        };

        let _lock = self.state_file_lock.read().map_err(|e| {
            io::Error::other(format!("Failed to acquire read lock on state file: {e}"))
        })?;
        let contents = match fs::read_to_string(state_path) {
            Ok(c) => c,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                let empty_state = StateFile {
                    fuzzers: Vec::new(),
                };
                if let Ok(json) = serde_json::to_string_pretty(&empty_state) {
                    let _ = fs::write(state_path, json);
                }
                return Ok(Some(empty_state));
            }
            Err(e) => {
                info!(
                    "Failed to read state file {}: {}, using empty state",
                    state_path.display(),
                    e
                );
                return Ok(Some(StateFile {
                    fuzzers: Vec::new(),
                }));
            }
        };

        validation::validate_state_file_size(&contents)?;

        match serde_json::from_str::<StateFile>(&contents) {
            Ok(state) => {
                for entry in &state.fuzzers {
                    validation::validate_sha256_hex(&entry.sha)?;

                    let path = Path::new(&entry.path);
                    if !path.is_absolute() {
                        return Err(SchedulerError::InvalidInput(format!(
                            "State file contains non-absolute path: {}",
                            entry.path
                        )));
                    }

                    if let Some(ref name) = entry.container_name {
                        validation::validate_container_name(name)?;
                    }

                    if let Some(ref env_vars) = entry.var_env {
                        for (key, value) in env_vars {
                            validation::validate_env_var(key, value)?;
                        }
                    }

                    if let Some(ref cmd_options) = entry.cmd_options {
                        for option in cmd_options {
                            validation::validate_cmd_option(option)?;
                        }
                    }
                }
                Ok(Some(state))
            }
            Err(e) => {
                info!(
                    "Failed to parse state file {}: {}, using empty state",
                    state_path.display(),
                    e
                );
                Ok(Some(StateFile {
                    fuzzers: Vec::new(),
                }))
            }
        }
    }

    /// Write the state file with current fuzzer states.
    pub fn write_state_file(&self) -> Result<()> {
        let Some(ref state_path) = self.state_file_path else {
            return Ok(());
        };

        if let Some(parent) = state_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let existing_state = self.read_state_file()?;
        let mut existing_entries: HashMap<String, &FuzzerStateEntry> = HashMap::new();
        if let Some(ref state) = existing_state {
            for entry in &state.fuzzers {
                existing_entries.insert(entry.sha.clone(), entry);
            }
        }

        let mut fuzzers = Vec::new();
        for (sha, task) in &self.tasks {
            let state = match task.status {
                TaskStatus::Running => FuzzerStateCommand::RUNNING,
                TaskStatus::Crashed => FuzzerStateCommand::CRASHED,
                TaskStatus::Stopped => FuzzerStateCommand::STOPPED,
            };
            let sha_str = hex::encode(sha);
            let existing_entry = existing_entries.get(&sha_str);
            fuzzers.push(FuzzerStateEntry {
                path: task.path.display().to_string(),
                sha: sha_str,
                state,
                container_name: task.container_name.clone(),
                var_env: existing_entry.and_then(|e| e.var_env.clone()),
                cmd_options: existing_entry.and_then(|e| e.cmd_options.clone()),
            });
        }

        for (sha, path) in &self.targets {
            if !self.tasks.contains_key(sha) {
                let sha_str = hex::encode(sha);
                let existing_entry = existing_entries.get(&sha_str);
                fuzzers.push(FuzzerStateEntry {
                    path: path.display().to_string(),
                    sha: sha_str,
                    state: FuzzerStateCommand::STOPPED,
                    container_name: None,
                    var_env: existing_entry.and_then(|e| e.var_env.clone()),
                    cmd_options: existing_entry.and_then(|e| e.cmd_options.clone()),
                });
            }
        }

        let state_file = StateFile { fuzzers };

        let temp_path = state_path.with_extension("tmp");
        let json = serde_json::to_string_pretty(&state_file).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Failed to serialize state: {e}"),
            )
        })?;
        fs::write(&temp_path, json)?;

        let _lock = self.state_file_lock.write().map_err(|e| {
            io::Error::other(format!("Failed to acquire write lock on state file: {e}"))
        })?;

        fs::rename(&temp_path, state_path)?;
        Ok(())
    }
}
