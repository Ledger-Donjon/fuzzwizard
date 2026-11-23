// SPDX-FileCopyrightText: © 2026 LEDGER SAS
// SPDX-License-Identifier: MIT

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::{Child, ChildStderr};
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::collector::Sha;

/// Return type for get_task_options_from_state_file (var_env, cmd_options).
pub(crate) type TaskOptionsFromState = (Option<HashMap<String, String>>, Option<Vec<String>>);

/// Status of a task
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskStatus {
    Running,
    Crashed,
    Stopped,
}

/// Information about a task's status and timestamps (without the process handle)
#[derive(Debug, Clone)]
pub struct TaskStatusInfo {
    pub sha: Sha,
    pub path: PathBuf,
    pub status: TaskStatus,
    pub started_at: Instant,
    pub crashed_at: Option<Instant>,
    pub stopped_at: Option<Instant>,
}

/// A fuzzing task to be scheduled by the [`crate::Scheduler`]
#[derive(Debug)]
pub struct Task {
    pub(crate) path: PathBuf,
    pub(crate) child: Child,
    pub(crate) stderr: ChildStderr,
    pub(crate) started_at: Instant,
    pub(crate) container_name: Option<String>,
    /// LLVM `.profraw` filename pattern. `%m` is resolved when coverage is read.
    pub(crate) profraw_path: Option<PathBuf>,
    pub status: TaskStatus,
    pub(crate) crashed_at: Option<Instant>,
    pub(crate) stopped_at: Option<Instant>,
}

#[derive(Serialize, Clone)]
pub(crate) struct CrashReport {
    pub report: Vec<u8>,
    pub input: Vec<u8>,
    pub filename: String,
    pub program: Vec<u8>,
    pub scheduler_name: String,
}

#[derive(Clone, Debug)]
pub(crate) struct ProcessRecord {
    pub pid: u32,
    pub container_name: Option<String>,
    pub fuzzer_name: String,
    pub scheduler_name: String,
}

/// Represents the state of a single fuzzer in the state file
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum FuzzerStateCommand {
    RUNNING,
    CRASHED,
    STOPPED,
}

/// Represents a fuzzer entry in the state file
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct FuzzerStateEntry {
    pub path: String,
    pub sha: String,
    pub state: FuzzerStateCommand,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub container_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub var_env: Option<HashMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cmd_options: Option<Vec<String>>,
}

/// Represents the complete state file
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct StateFile {
    pub fuzzers: Vec<FuzzerStateEntry>,
}
