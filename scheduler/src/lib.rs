// SPDX-FileCopyrightText: © 2026 LEDGER SAS
// SPDX-License-Identifier: MIT

use chrono::{prelude::*, TimeDelta};
use humantime::format_duration;
use log::{debug, info, warn};
use rand::{rng, Rng};
use serde::{Deserialize, Serialize};
use sha3::{Digest, Sha3_256};
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant};
use std::{
    collections::{HashMap, HashSet},
    fs, io,
    io::Read,
    path::{Path, PathBuf},
    process::{Child, ChildStderr, Command, Stdio},
    sync::mpsc,
    thread,
};
use thiserror::Error;

/// Return type for get_task_options_from_state_file (var_env, cmd_options).
type TaskOptionsFromState = (Option<HashMap<String, String>>, Option<Vec<String>>);

/// Errors that can occur within the scheduler crate
#[derive(Debug, Error)]
pub enum SchedulerError {
    /// I/O related failures (filesystem, process spawn, pipes, etc.)
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    /// Collector returned an inconsistent state
    #[error("Inconsistent state: {0}")]
    InconsistentState(&'static str),
    /// Child process had no stderr pipe attached
    #[error("Child process missing stderr pipe")]
    MissingChildStderr,
    /// Failed HTTP request to backend
    #[error("Backend error: {0}")]
    Backend(#[from] reqwest::Error),
    #[error("Container binary path not specified")]
    MissingContainerBinary,
    /// Invalid input provided by user
    #[error("Invalid input: {0}")]
    InvalidInput(String),
    /// Path traversal attempt detected
    #[error("Path traversal detected: {0}")]
    PathTraversal(String),
}

#[derive(Serialize, Deserialize, Clone)]
pub struct SchedulerConfig {
    pub scheduler_name: String,
    pub targets_directory: Vec<String>,
    pub build_script: String,
    pub build_script_interval_hours: Option<u64>,
    pub backend_url: String,
    pub corpus_directory: String,
    pub crashes_directory: String,
    pub coverage_directory: String,
    pub container_image: Option<String>,
    pub container_options: Option<Vec<String>>,
    pub has_coverage: bool,
    /// Directory containing `llvm-cov` and `llvm-profdata` (optional).
    /// If unset, `FUZZWIZARD_LLVM_BIN` is used when set, otherwise tools are resolved from `PATH`.
    pub llvm_bin_directory: Option<String>,
    pub state_file: Option<String>,
}

/// Represents the state of a single fuzzer in the state file
/// The state file is now only used for logging, not for commands.
/// Commands (START, STOP, RESTART) are handled directly via the TUI.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum FuzzerStateCommand {
    /// Automatic state: fuzzer is running (used for logging)
    RUNNING,
    /// Automatic state: fuzzer has crashed (used for logging)
    CRASHED,
    /// Automatic state: fuzzer has been stopped (used for logging)
    STOPPED,
}

/// Represents a fuzzer entry in the state file
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct FuzzerStateEntry {
    pub path: String,
    pub sha: String,
    pub state: FuzzerStateCommand,
    /// Container name if the fuzzer is running in a container (for easy attachment)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub container_name: Option<String>,
    /// Environment variables to set when starting the fuzzer
    #[serde(skip_serializing_if = "Option::is_none")]
    pub var_env: Option<HashMap<String, String>>,
    /// Command line options to pass to the fuzzer
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cmd_options: Option<Vec<String>>,
}

/// Represents the complete state file
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct StateFile {
    pub fuzzers: Vec<FuzzerStateEntry>,
}

/// Convenient result alias for the scheduler crate
pub type Result<T> = std::result::Result<T, SchedulerError>;

/// Coverage analysis module
mod coverage;

pub use coverage::COVERAGE_SOURCE_UNAVAILABLE_MARKER;
pub use coverage::{CoverageMetricEntry, CoverageSummary};

/// Security validation module for input sanitization
pub mod validation {
    use super::{Result, SchedulerError};
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    /// Maximum length for environment variable keys and values
    const MAX_ENV_VAR_LENGTH: usize = 4096;
    /// Maximum length for command options
    const MAX_CMD_OPTION_LENGTH: usize = 8192;
    /// Maximum size of state file (10 MB)
    const MAX_STATE_FILE_SIZE: usize = 10 * 1024 * 1024;

    /// Dangerous characters that could be used for shell injection
    /// Used for validating environment variables and command options
    const DANGEROUS_CHARS: &[char] = &[
        '\0', // null byte
        '\n', '\r', // newlines
        ';',  // command separator
        '&',  // background/AND
        '|',  // pipe
        '`',  // command substitution
        '$',  // variable expansion
        '(', ')', // subshell
        '<', '>', // redirection
    ];

    /// Validates an environment variable key and value
    ///
    /// # Security
    ///
    /// - Checks for null bytes
    /// - Checks for newlines (could break shell escaping)
    /// - Validates length limits
    /// - Checks for dangerous shell metacharacters that could enable command injection
    ///
    /// # Errors
    ///
    /// Returns `SchedulerError::InvalidInput` if the key or value is invalid
    pub fn validate_env_var(key: &str, value: &str) -> Result<()> {
        // Check key is not empty
        if key.is_empty() {
            return Err(SchedulerError::InvalidInput(
                "Environment variable key cannot be empty".to_string(),
            ));
        }

        // Check length limits
        if key.len() > MAX_ENV_VAR_LENGTH {
            return Err(SchedulerError::InvalidInput(format!(
                "Environment variable key too long: {} bytes (max: {})",
                key.len(),
                MAX_ENV_VAR_LENGTH
            )));
        }
        if value.len() > MAX_ENV_VAR_LENGTH {
            return Err(SchedulerError::InvalidInput(format!(
                "Environment variable value too long: {} bytes (max: {})",
                value.len(),
                MAX_ENV_VAR_LENGTH
            )));
        }

        // Check key for dangerous characters
        if key.chars().any(|c| DANGEROUS_CHARS.contains(&c)) {
            return Err(SchedulerError::InvalidInput(format!(
                "Environment variable key contains dangerous characters: {key}"
            )));
        }

        // Key should be alphanumeric + underscore + dash
        if !key
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
        {
            return Err(SchedulerError::InvalidInput(format!(
                "Invalid characters in environment variable key: {key}"
            )));
        }

        // Check value for dangerous characters
        if value.chars().any(|c| DANGEROUS_CHARS.contains(&c)) {
            return Err(SchedulerError::InvalidInput(
                "Environment variable value contains dangerous characters".to_string(),
            ));
        }

        Ok(())
    }

    /// Validates a command option
    ///
    /// # Security
    ///
    /// - Checks for null bytes
    /// - Validates length limits
    /// - Checks for shell metacharacters that could enable command injection
    ///
    /// # Errors
    ///
    /// Returns `SchedulerError::InvalidInput` if the option is invalid
    pub fn validate_cmd_option(option: &str) -> Result<()> {
        // Check option is not empty
        if option.is_empty() {
            return Err(SchedulerError::InvalidInput(
                "Command option cannot be empty".to_string(),
            ));
        }

        if option.len() > MAX_CMD_OPTION_LENGTH {
            return Err(SchedulerError::InvalidInput(format!(
                "Command option too long: {} bytes (max: {})",
                option.len(),
                MAX_CMD_OPTION_LENGTH
            )));
        }

        // Note: For command options, we use a subset of dangerous chars
        // (excluding '<', '>', '(', ')' which might be needed for some options)
        const CMD_OPTION_DANGEROUS_CHARS: &[char] = &[
            '\0', // null byte
            '\n', '\r', // newlines
            ';',  // command separator
            '&',  // background/AND
            '|',  // pipe
            '`',  // command substitution
            '$',  // variable expansion (can be used in paths but risky)
        ];

        if option
            .chars()
            .any(|c| CMD_OPTION_DANGEROUS_CHARS.contains(&c))
        {
            return Err(SchedulerError::InvalidInput(
                "Command option contains dangerous characters".to_string(),
            ));
        }

        Ok(())
    }

    /// Validates a SHA-256 hex string
    ///
    /// # Security
    ///
    /// - Must be exactly 64 hexadecimal characters
    /// - No other characters allowed
    pub fn validate_sha256_hex(sha: &str) -> Result<()> {
        if sha.len() != 64 {
            return Err(SchedulerError::InvalidInput(format!(
                "Invalid SHA-256 length: {} (expected 64)",
                sha.len()
            )));
        }

        if !sha.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(SchedulerError::InvalidInput(
                "SHA-256 must contain only hexadecimal characters".to_string(),
            ));
        }

        Ok(())
    }

    /// Validates the size of state file content before parsing
    pub fn validate_state_file_size(content: &str) -> Result<()> {
        if content.len() > MAX_STATE_FILE_SIZE {
            return Err(SchedulerError::InvalidInput(format!(
                "State file too large: {} bytes (max: {})",
                content.len(),
                MAX_STATE_FILE_SIZE
            )));
        }
        Ok(())
    }

    /// Validates a container name to ensure it doesn't contain dangerous characters
    pub fn validate_container_name(name: &str) -> Result<()> {
        if name.is_empty() {
            return Err(SchedulerError::InvalidInput(
                "Container name cannot be empty".to_string(),
            ));
        }

        if name.len() > 256 {
            return Err(SchedulerError::InvalidInput(format!(
                "Container name too long: {} bytes (max: 256)",
                name.len()
            )));
        }

        // Container names should only contain alphanumeric, dash, underscore
        if !name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '_' || c == '.')
        {
            return Err(SchedulerError::InvalidInput(format!(
                "Invalid characters in container name: {name}"
            )));
        }

        Ok(())
    }

    /// Validates a container option against a strict allowlist.
    /// Rejects dangerous flags like --privileged, --pid=host, --cap-add, etc.
    pub fn validate_container_option(option: &str) -> Result<()> {
        if option.is_empty() {
            return Ok(());
        }

        const FORBIDDEN_OPTIONS: &[&str] = &[
            "--privileged",
            "--pid=host",
            "--pid=",
            "--net=host",
            "--network=host",
            "--cap-add",
            "--device",
            "--ipc=host",
            "--userns=host",
            "--security-opt",
            "-v",
            "--volume",
            "--mount",
        ];

        let lower = option.to_lowercase();
        for forbidden in FORBIDDEN_OPTIONS {
            if lower.starts_with(forbidden) {
                return Err(SchedulerError::InvalidInput(format!(
                    "Forbidden container option: {option} (matches blocked pattern '{forbidden}')"
                )));
            }
        }

        Ok(())
    }

    /// Validates that a backend URL points to localhost only (SSRF hardening).
    ///
    /// Parses the authority/host explicitly so prefixes like `http://127.0.0.1.evil.test`
    /// are not mistaken for loopback.
    pub fn validate_backend_url(url: &str) -> Result<()> {
        let url = url.trim();
        let scheme_len = if url.len() >= 7 && url.as_bytes()[..7].eq_ignore_ascii_case(b"http://") {
            7
        } else if url.len() >= 8 && url.as_bytes()[..8].eq_ignore_ascii_case(b"https://") {
            8
        } else {
            return Err(SchedulerError::InvalidInput(
                "backend_url must start with http:// or https://".to_string(),
            ));
        };

        let rest = &url[scheme_len..];
        let authority_end = rest
            .find(|c| ['/', '?', '#'].contains(&c))
            .unwrap_or(rest.len());
        let authority = &rest[..authority_end];

        let host_port = authority
            .rsplit_once('@')
            .map(|(_, host)| host)
            .unwrap_or(authority);

        if host_port.starts_with('[') {
            return Err(SchedulerError::InvalidInput(
                "backend_url must use 127.0.0.1 or localhost (IPv6 bracket host is not supported)"
                    .to_string(),
            ));
        }

        let host_end = host_port.find(':').unwrap_or(host_port.len());
        let host = &host_port[..host_end];
        if host != "127.0.0.1" && !host.eq_ignore_ascii_case("localhost") {
            return Err(SchedulerError::InvalidInput(format!(
                "backend_url must use 127.0.0.1 or localhost as host, got: {host}"
            )));
        }

        if host_end < host_port.len() {
            let port_part = &host_port[host_end + 1..];
            if port_part.is_empty() || !port_part.chars().all(|c| c.is_ascii_digit()) {
                return Err(SchedulerError::InvalidInput(
                    "backend_url: invalid port".to_string(),
                ));
            }
        }

        Ok(())
    }

    /// Validates a build script path: must be an absolute path to an existing executable file.
    /// Rejects arbitrary shell commands to prevent code injection.
    pub fn validate_build_script(script: &str) -> Result<()> {
        if script.trim().is_empty() {
            return Ok(());
        }

        let s = script.trim();
        if s.chars().any(|c| c.is_control()) {
            return Err(SchedulerError::InvalidInput(
                "Build script must not contain control characters".to_string(),
            ));
        }

        if script.contains('\0') {
            return Err(SchedulerError::InvalidInput(
                "Build script contains null bytes".to_string(),
            ));
        }

        if script.len() > 65536 {
            return Err(SchedulerError::InvalidInput(format!(
                "Build script too long: {} bytes (max: 65536)",
                script.len()
            )));
        }

        const DANGEROUS_PATTERNS: &[&str] = &["&&", "||", ";", "`", "$(", ">>", "<<"];
        for pattern in DANGEROUS_PATTERNS {
            if script.contains(pattern) {
                return Err(SchedulerError::InvalidInput(format!(
                    "Build script contains dangerous shell pattern '{pattern}'. \
                     Use an executable script file instead of inline shell commands."
                )));
            }
        }

        let path = Path::new(s);
        if !path.is_absolute() {
            return Err(SchedulerError::InvalidInput(
                "Build script must be an absolute path to an executable file (e.g. /opt/fuzzwizard/scripts/build.sh)".to_string(),
            ));
        }

        let meta = std::fs::metadata(path).map_err(|_| {
            SchedulerError::InvalidInput(format!(
                "Build script path does not exist or is not accessible: {s}"
            ))
        })?;
        if !meta.is_file() {
            return Err(SchedulerError::InvalidInput(format!(
                "Build script must be a regular file: {s}"
            )));
        }
        let mode = meta.permissions().mode();
        if mode & 0o111 == 0 {
            return Err(SchedulerError::InvalidInput(format!(
                "Build script is not executable: {s}"
            )));
        }

        Ok(())
    }
}

/// A trait for collecting targets to be fuzzed by the scheduler.
pub trait TargetsCollector {
    /// Collect the latest fuzzer binaries to be run.
    fn collect_targets(&self) -> HashMap<Sha, PathBuf>;
}

pub type Sha = [u8; 32];

/// A collector that scans directories for executable fuzzer binaries
pub struct DirectoryTargetsCollector {
    targets_directories: Vec<PathBuf>,
}

impl DirectoryTargetsCollector {
    /// Create a new DirectoryTargetsCollector
    pub fn new(targets_directories: Vec<PathBuf>) -> Self {
        Self {
            targets_directories,
        }
    }
}

impl TargetsCollector for DirectoryTargetsCollector {
    fn collect_targets(&self) -> HashMap<Sha, PathBuf> {
        let mut targets = HashMap::new();

        for directory in self.targets_directories.iter() {
            // Handle error instead of panicking
            let read_dir = match fs::read_dir(directory) {
                Ok(rd) => rd,
                Err(e) => {
                    log::error!("Failed to read directory {directory:?}: {e}");
                    continue; // Move to next directory
                }
            };

            for direntry_result in read_dir {
                // Handle directory entry read error
                let direntry = match direntry_result {
                    Ok(de) => de,
                    Err(e) => {
                        log::error!("Failed to read directory entry in {directory:?}: {e}");
                        continue;
                    }
                };

                let target_path = direntry.path();

                // Check that it's a file
                if !target_path.is_file() {
                    continue;
                }

                // Handle metadata read error
                let metadata = match target_path.metadata() {
                    Ok(m) => m,
                    Err(e) => {
                        log::error!("Failed to get metadata for {target_path:?}: {e}");
                        continue;
                    }
                };

                // Check execution permissions
                if metadata.permissions().mode() & 0o111 == 0 {
                    continue;
                }

                // Handle file read error
                let file_contents = match fs::read(&target_path) {
                    Ok(contents) => contents,
                    Err(e) => {
                        log::error!("Failed to read file {target_path:?}: {e}");
                        continue;
                    }
                };

                let target_hash = Sha3_256::digest(&file_contents);
                targets.insert(target_hash.into(), target_path);
            }
        }

        targets
    }
}

/// A scheduler that manages the lifecycle of fuzzing tasks.
///
/// For now the scheduler does not implement any scheduling algorithm, it just
/// manages the fuzzing tasks.
pub struct Scheduler<C: TargetsCollector> {
    collector: C,
    /// Map of current targets keyed by the hash of the binary
    targets: HashMap<Sha, PathBuf>,
    /// Map of current tasks keyed by the hash of the binary
    tasks: HashMap<Sha, Task>,
    /// Directory where fuzzer corpuses are stored
    corpus_directory: PathBuf,
    /// Directory where fuzzer crashes are stored
    crashes_directory: PathBuf,
    /// Directory where fuzzer coverage are stored
    coverage_directory: PathBuf,
    /// URL of the backend to report crashes
    backend_url: String,
    /// Name of the scheduler
    scheduler_name: String,
    container_image: Option<String>,
    container_binary: Option<PathBuf>,
    container_options: Option<Vec<String>>,
    has_coverage: bool,
    /// Optional directory for LLVM coverage tools (`llvm-cov`, `llvm-profdata`).
    llvm_bin_directory: Option<PathBuf>,
    /// Path to the state file (if configured)
    state_file_path: Option<PathBuf>,
    /// Lock to protect concurrent access to the state file
    state_file_lock: Arc<RwLock<()>>,
}

#[derive(Serialize, Clone)]
struct CrashReport {
    report: Vec<u8>,
    input: Vec<u8>,
    filename: String,
    program: Vec<u8>,
    scheduler_name: String,
}

#[derive(Clone, Debug)]
struct ProcessRecord {
    pid: u32,
    container_name: Option<String>,
    fuzzer_name: String,
    scheduler_name: String,
}

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
    /// SHA hash of the fuzzer binary
    pub sha: Sha,
    /// Path of the fuzzer binary
    pub path: PathBuf,
    /// Status of the task
    pub status: TaskStatus,
    /// Timestamp when the task started
    pub started_at: Instant,
    /// Timestamp when the task crashed (if applicable)
    pub crashed_at: Option<Instant>,
    /// Timestamp when the task was stopped (if applicable)
    pub stopped_at: Option<Instant>,
}

/// A fuzzing task to be scheduled by the [`Scheduler`]
#[derive(Debug)]
pub struct Task {
    /// Path of the fuzzer binary
    path: PathBuf,
    /// The fuzzer process
    child: Child,
    /// The stderr of the fuzzer process
    stderr: ChildStderr,
    started_at: Instant,
    container_name: Option<String>,
    /// LLVM `.profraw` filename pattern. `%m` is resolved when coverage is read.
    profraw_path: Option<PathBuf>,
    /// Status of the task (running, crashed, or stopped)
    pub status: TaskStatus,
    /// Timestamp when the task crashed (if applicable)
    crashed_at: Option<Instant>,
    /// Timestamp when the task was stopped (if applicable)
    stopped_at: Option<Instant>,
}

static PROCESS_REGISTRY: OnceLock<Arc<Mutex<Vec<ProcessRecord>>>> = OnceLock::new();

/// Helper function to kill a process by PID
fn kill_process_by_pid(pid: u32) {
    let _ = std::thread::spawn(move || {
        let _ = Command::new("/bin/kill")
            .args(["-TERM", &pid.to_string()])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .and_then(|mut child| child.wait());
    });
}

/// Helper function to remove a container by name
fn remove_container(binary: &Path, name: &str) {
    let binary_clone = binary.to_path_buf();
    let name_clone = name.to_string();
    let _ = std::thread::spawn(move || {
        let _ = Command::new(&binary_clone)
            .args(["rm", "-f", &name_clone])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .and_then(|mut child| child.wait());
    });
}

/// Resolve the files produced by LLVM's `%m` online-merge filename token.
///
/// The runtime expands `%m` to a load module signature that cannot be computed
/// ahead of time, so the directory is listed instead. A fuzzer normally yields
/// a single file; the `<sha>-` prefix keeps profiles of other builds out.
fn resolve_profraw_paths(pattern: &Path) -> Vec<PathBuf> {
    let Some(parent) = pattern.parent() else {
        return Vec::new();
    };
    let Some(file_pattern) = pattern.file_name().and_then(|name| name.to_str()) else {
        return Vec::new();
    };
    let Some((prefix, suffix)) = file_pattern.split_once("%m") else {
        return Vec::new();
    };

    let mut paths: Vec<PathBuf> = fs::read_dir(parent)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let name = entry.file_name();
            let name = name.to_str()?;
            (name.starts_with(prefix) && name.ends_with(suffix)).then(|| entry.path())
        })
        .collect();
    paths.sort();
    paths
}

/// Helper function to collect coverage from a containerized fuzzer
fn collect_coverage_container(binary: &Path, container_name: &str, timeout_secs: u64) {
    let binary_clone = binary.to_path_buf();
    let name_for_thread = container_name.to_string();
    let name_for_log = container_name.to_string();
    let (tx, rx) = mpsc::channel();

    // Spawn command in a thread
    thread::spawn(move || {
        match Command::new(&binary_clone)
            .args([
                "exec",
                &name_for_thread,
                "pkill",
                "-SIGUSR1",
                "-f",
                "binary",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(mut child) => {
                let status = child.wait();
                let _ = tx.send(status);
            }
            Err(e) => {
                let _ = tx.send(Err(e));
            }
        }
    });

    // Wait for completion with timeout
    match rx.recv_timeout(Duration::from_secs(timeout_secs)) {
        Ok(Ok(status)) => {
            if status.success() {
                info!("Coverage collection successful for container: {name_for_log}");
            } else {
                warn!("Coverage collection failed for container {name_for_log}: {status:?}");
            }
        }
        Ok(Err(e)) => {
            warn!("Failed to execute coverage collection for container {name_for_log}: {e}");
        }
        Err(mpsc::RecvTimeoutError::Timeout) => {
            warn!("Coverage collection timeout ({timeout_secs}s) for container: {name_for_log}");
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            warn!("Coverage collection thread disconnected for container: {name_for_log}");
        }
    }
}

/// Helper function to collect coverage from a non-containerized fuzzer
fn collect_coverage_native(fuzzer_name: &str, timeout_secs: u64) {
    let fuzzer_name_clone = fuzzer_name.to_string();
    let fuzzer_name_for_log = fuzzer_name.to_string();
    let (tx, rx) = mpsc::channel();

    // Spawn command in a thread
    thread::spawn(move || {
        match Command::new("/usr/bin/pkill")
            .args(["-SIGUSR1", "-f", &fuzzer_name_clone])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(mut child) => {
                let status = child.wait();
                let _ = tx.send(status);
            }
            Err(e) => {
                let _ = tx.send(Err(e));
            }
        }
    });

    // Wait for completion with timeout
    match rx.recv_timeout(Duration::from_secs(timeout_secs)) {
        Ok(Ok(status)) => {
            if status.success() {
                info!("Coverage collection successful for fuzzer: {fuzzer_name_for_log}");
            } else {
                warn!("Coverage collection failed for fuzzer {fuzzer_name_for_log}: {status:?}");
            }
        }
        Ok(Err(e)) => {
            warn!("Failed to execute coverage collection for fuzzer {fuzzer_name_for_log}: {e}");
        }
        Err(mpsc::RecvTimeoutError::Timeout) => {
            warn!(
                "Coverage collection timeout ({timeout_secs}s) for fuzzer: {fuzzer_name_for_log}"
            );
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            warn!("Coverage collection thread disconnected for fuzzer: {fuzzer_name_for_log}");
        }
    }
}

fn install_ctrlc_handler() {
    let registry = PROCESS_REGISTRY
        .get_or_init(|| Arc::new(Mutex::new(Vec::new())))
        .clone();
    let _ = ctrlc::set_handler(move || {
        // Extract data quickly and release the lock
        // This prevents holding the lock during potentially slow operations
        let records: Vec<ProcessRecord> = {
            match registry.lock() {
                Ok(guard) => {
                    // Clone the records to release the lock quickly
                    guard.clone()
                }
                Err(poisoned) => {
                    warn!("Process registry was poisoned, recovering data");
                    // Recover data even if mutex is poisoned
                    poisoned.into_inner().clone()
                }
            }
        };
        // Lock is released here

        // Process records without holding the lock
        for rec in records.iter() {
            if let Some(name) = &rec.container_name {
                // Try docker first, then podman (non-blocking spawn)
                let name_clone = name.clone();
                let _ = Command::new("/usr/bin/docker")
                    .args(["rm", "-f", &name_clone])
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status()
                    .or_else(|_| {
                        Command::new("/usr/bin/podman")
                            .args(["rm", "-f", &name_clone])
                            .stdout(std::process::Stdio::null())
                            .stderr(std::process::Stdio::null())
                            .status()
                    });
            };
            // Kill process using helper function
            kill_process_by_pid(rec.pid);
        }
        // Give a small delay for commands to start, then exit
        std::thread::sleep(std::time::Duration::from_millis(100));
        std::process::exit(130);
    });
}

impl<C: TargetsCollector> Scheduler<C> {
    /// Get the path to the state file
    pub fn state_file_path(&self) -> Option<&PathBuf> {
        self.state_file_path.as_ref()
    }

    /// Create a new [`Scheduler`].
    ///
    /// # Panics
    ///
    /// Panics if `backend_url` does not point to localhost.
    pub fn new(collector: C, config: SchedulerConfig, container_binary: Option<PathBuf>) -> Self {
        // Validate backend_url is localhost-only to prevent data exfiltration
        if let Err(e) = validation::validate_backend_url(&config.backend_url) {
            panic!("Security: {e}");
        }

        // Ensure Ctrl-C handler is installed once
        install_ctrlc_handler();

        // Determine state file path: use config value or default to a protected directory
        let state_file_path = config.state_file.map(PathBuf::from).unwrap_or_else(|| {
            let state_dir = PathBuf::from(format!(
                "{}/.fuzzwizard",
                std::env::var("HOME").unwrap_or_else(|_| "/var/lib/fuzzwizard".to_string())
            ));
            state_dir.join(format!("{}_state.json", config.scheduler_name))
        });

        // Ensure the state file directory exists with restricted permissions (owner-only)
        if let Some(parent) = state_file_path.parent() {
            if let Err(e) = fs::create_dir_all(parent) {
                info!(
                    "Failed to create state file directory {}: {}",
                    parent.display(),
                    e
                );
            } else {
                let mut perms = fs::metadata(parent)
                    .map(|m| m.permissions())
                    .unwrap_or_else(|_| fs::Permissions::from_mode(0o700));
                perms.set_mode(0o700);
                let _ = fs::set_permissions(parent, perms);
            }
        }

        // Create empty state file at startup (overwrite any existing file)
        let empty_state = StateFile {
            fuzzers: Vec::new(),
        };
        if let Ok(json) = serde_json::to_string_pretty(&empty_state) {
            if let Err(e) = fs::write(&state_file_path, json) {
                info!(
                    "Failed to create state file {}: {}",
                    state_file_path.display(),
                    e
                );
            } else {
                info!("Created empty state file: {}", state_file_path.display());
            }
        }

        Self {
            collector,
            targets: HashMap::new(),
            tasks: HashMap::new(),
            corpus_directory: config.corpus_directory.into(),
            crashes_directory: config.crashes_directory.into(),
            backend_url: config.backend_url,
            scheduler_name: config.scheduler_name,
            container_image: config.container_image,
            container_options: config.container_options,
            container_binary,
            coverage_directory: config.coverage_directory.into(),
            has_coverage: config.has_coverage,
            llvm_bin_directory: config.llvm_bin_directory.map(PathBuf::from),
            state_file_path: Some(state_file_path),
            state_file_lock: Arc::new(RwLock::new(())),
        }
    }

    /// Read the state file and return the state entries
    /// Get var_env and cmd_options for a task from the state file
    fn get_task_options_from_state_file(&self, sha: Sha) -> Result<TaskOptionsFromState> {
        let sha_str = hex::encode(sha);
        if let Some(state_file) = self.read_state_file()? {
            for entry in &state_file.fuzzers {
                if entry.sha == sha_str {
                    return Ok((entry.var_env.clone(), entry.cmd_options.clone()));
                }
            }
        }
        // No entry found, return None for both
        Ok((None, None))
    }

    /// Internal helper to start a task by SHA with options from state file
    /// Returns the task path for logging purposes
    fn start_task_with_state_file_options(&mut self, sha: Sha) -> Result<PathBuf> {
        // Find the path for this SHA
        let path = self
            .targets
            .get(&sha)
            .ok_or_else(|| SchedulerError::InvalidInput("Task not found in targets".to_string()))?;

        let fuzzer_path = path.canonicalize()?;
        let task_path = fuzzer_path.clone();

        // Read var_env and cmd_options from state file if available
        let (var_env, cmd_options) = self.get_task_options_from_state_file(sha)?;

        let task = self.start_task(fuzzer_path, sha, var_env.as_ref(), cmd_options.as_ref())?;
        self.tasks.insert(sha, task);

        // Update state file
        let _ = self.write_state_file();

        Ok(task_path)
    }

    fn read_state_file(&self) -> Result<Option<StateFile>> {
        let Some(ref state_path) = self.state_file_path else {
            return Ok(None);
        };

        // Acquire read lock
        let _lock = self.state_file_lock.read().map_err(|e| {
            io::Error::other(format!("Failed to acquire read lock on state file: {e}"))
        })?;
        // Try to read the file
        let contents = match fs::read_to_string(state_path) {
            Ok(c) => c,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                // File doesn't exist yet, create empty one
                let empty_state = StateFile {
                    fuzzers: Vec::new(),
                };
                if let Ok(json) = serde_json::to_string_pretty(&empty_state) {
                    let _ = fs::write(state_path, json);
                }
                return Ok(Some(empty_state));
            }
            Err(e) => {
                // Other error, log and return empty state
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

        // Validate state file size before parsing
        validation::validate_state_file_size(&contents)?;

        // Try to parse JSON
        match serde_json::from_str::<StateFile>(&contents) {
            Ok(state) => {
                // Validate the parsed state
                for entry in &state.fuzzers {
                    // Validate SHA format
                    validation::validate_sha256_hex(&entry.sha)?;

                    // Validate path
                    let path = Path::new(&entry.path);
                    if !path.is_absolute() {
                        return Err(SchedulerError::InvalidInput(format!(
                            "State file contains non-absolute path: {}",
                            entry.path
                        )));
                    }

                    // Validate container name if present
                    if let Some(ref name) = entry.container_name {
                        validation::validate_container_name(name)?;
                    }

                    // Validate environment variables if present
                    if let Some(ref env_vars) = entry.var_env {
                        for (key, value) in env_vars {
                            validation::validate_env_var(key, value)?;
                        }
                    }

                    // Validate command options if present
                    if let Some(ref cmd_options) = entry.cmd_options {
                        for option in cmd_options {
                            validation::validate_cmd_option(option)?;
                        }
                    }
                }
                Ok(Some(state))
            }
            Err(e) => {
                // File is corrupted or invalid, log and return empty state
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

    /// Write the state file with current fuzzer states
    /// Only updates the file if there are actual changes
    pub fn write_state_file(&self) -> Result<()> {
        let Some(ref state_path) = self.state_file_path else {
            return Ok(());
        };

        // Ensure parent directory exists
        if let Some(parent) = state_path.parent() {
            fs::create_dir_all(parent)?;
        }

        // Read existing state file to preserve var_env and cmd_options
        let existing_state = self.read_state_file()?;
        let mut existing_entries: HashMap<String, &FuzzerStateEntry> = HashMap::new();
        if let Some(ref state) = existing_state {
            for entry in &state.fuzzers {
                existing_entries.insert(entry.sha.clone(), entry);
            }
        }

        // Build state from current tasks
        let mut fuzzers = Vec::new();
        for (sha, task) in &self.tasks {
            let state = match task.status {
                TaskStatus::Running => FuzzerStateCommand::RUNNING,
                TaskStatus::Crashed => FuzzerStateCommand::CRASHED,
                TaskStatus::Stopped => FuzzerStateCommand::STOPPED,
            };
            let sha_str = hex::encode(sha);
            // Preserve var_env and cmd_options from existing entry if present
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

        // Also include targets that are not yet started
        for (sha, path) in &self.targets {
            if !self.tasks.contains_key(sha) {
                let sha_str = hex::encode(sha);
                // Preserve var_env and cmd_options from existing entry if present
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

        // Write atomically: write to temp file then rename
        let temp_path = state_path.with_extension("tmp");
        let json = serde_json::to_string_pretty(&state_file).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Failed to serialize state: {e}"),
            )
        })?;
        fs::write(&temp_path, json)?;

        // Acquire write lock
        let _lock = self.state_file_lock.write().map_err(|e| {
            io::Error::other(format!("Failed to acquire write lock on state file: {e}"))
        })?;

        fs::rename(&temp_path, state_path)?;
        Ok(())
    }

    fn kill_task(&mut self, task: &mut Task) {
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
        // State file is now only used for logging, not for commands
        // Commands are handled directly via the TUI

        let latest_targets = self.collector.collect_targets();

        debug!("latest_targets: {latest_targets:?}");

        let old_targets_sha: HashSet<Sha> = self.targets.keys().copied().collect();
        let latest_targets_sha: HashSet<Sha> = latest_targets.keys().copied().collect();

        // NOTE: Can be optimized by iterating only once
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

        // Helper function to log target paths
        let log_targets = |label: &str, shas: &[Sha], targets_map: &HashMap<Sha, PathBuf>| {
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

        // Capture coverage snapshot before transitioning binaries (fingerprint change)
        if !targets_to_stop.is_empty() || !targets_to_start.is_empty() {
            self.snapshot_coverage_before_transition(&old_targets_sha);
        }

        for target in targets_to_stop {
            info!("stopping task: {target:x?}");
            if let Some(mut task) = self.tasks.remove(&target) {
                self.kill_task(&mut task);
                info!("removing task: {target:?}");
                // Task removed, update state file
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
            // Read var_env and cmd_options from state file if available
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
            // Task added, update state file
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
            // For restart, we don't need to log to running_tasks.log again
            // Read var_env and cmd_options from state file if available
            let (var_env, cmd_options) = self.get_task_options_from_state_file(target)?;
            let fuzzer_path = path.canonicalize()?;
            let task =
                self.start_task(fuzzer_path, target, var_env.as_ref(), cmd_options.as_ref())?;
            // Replace the stopped task with the new running one
            self.tasks.insert(target, task);
            // Task restarted, update state file
            let _ = self.write_state_file();
        }

        self.targets = latest_targets;

        // State file is updated only when tasks are added/removed/status changes
        // (handled in the loops above and in other methods)
        Ok(())
    }

    fn start_task(
        &self,
        fuzzer_path: PathBuf,
        fuzzer_hash: Sha,
        var_env: Option<&HashMap<String, String>>,
        cmd_options: Option<&Vec<String>>,
    ) -> Result<Task> {
        // Validate environment variables if provided
        if let Some(env_vars) = var_env {
            for (key, value) in env_vars {
                validation::validate_env_var(key, value)?;
            }
        }

        // Validate command options if provided
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
            //TODO : init with a random file here
            let random_file = fuzzer_corpus.join("crash_init");
            let mut file = fs::File::create(random_file)?;
            let mut data: [u8; 8192] = [0u8; 8192];
            rng.fill(&mut data);
            file.write_all(&data)?;
        }
        // Set permissions to 770 (owner + group only, no world access)
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
        let mut child: Child;
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
                    // Security hardening: isolate network, drop capabilities, prevent privilege escalation
                    "--network=none".into(),
                    "--cap-drop".into(),
                    "ALL".into(),
                    "--security-opt".into(),
                    "no-new-privileges".into(),
                    "--read-only".into(),
                    "--pids-limit".into(),
                    "512".into(),
                    // Writable /tmp inside the container for fuzzer scratch space
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

                // Docker requires flags like --platform before IMAGE; keep container_options here.
                if let Some(ref opts) = self.container_options {
                    for opt in opts {
                        if !opt.is_empty() {
                            validation::validate_container_option(opt)?;
                            args.push(opt.clone());
                        }
                    }
                }

                args.push(container_image.clone());

                args.push("/fuzzer/binary".into());
                args.push("/fuzzer/corpus_dir/".into());
                args.push("-artifact_prefix=/fuzzer/crash_dir/".into());

                // Add custom command options from state file if provided
                if let Some(custom_opts) = cmd_options {
                    args.extend(custom_opts.iter().cloned());
                }

                // Add environment variables from state file if provided (using -e for docker)
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
            // Build command arguments
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

            // Add custom command options from state file if provided
            if let Some(custom_opts) = cmd_options {
                args.extend(custom_opts.iter().cloned());
            }

            let mut cmd = Command::new(&fuzzer_path);
            cmd.args(&args);

            // Set environment variables from state file if provided
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
        // This allows later coverage processing by retaining the tested binary.

        // Compose path for the binary destination inside the coverage directory
        let coverage_bin_name = format!("fuzzer_{sha_hex}.bin");
        let coverage_bin_path = fuzzer_coveragedir.join(&coverage_bin_name);
        // Copy the binary to the coverage directory
        // If an error occurs, log it but don't fail the task startup
        if let Err(e) = fs::copy(fuzzer_path.clone(), &coverage_bin_path) {
            log::warn!(
                "Failed to copy binary to coverage directory ({}): {}",
                coverage_bin_path.display(),
                e
            );
        }
        let profraw_path = if self.has_coverage {
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
            .get_or_init(|| Arc::new(Mutex::new(Vec::new())))
            .clone();

        // Extract data quickly and release the lock
        let all_records: Vec<ProcessRecord> = {
            match registry.lock() {
                Ok(guard) => guard.clone(),
                Err(e) => {
                    warn!("Failed to lock process registry: {e}");
                    return;
                }
            }
        };
        // Lock is released here

        // Filter records to only include those from this scheduler
        let records: Vec<ProcessRecord> = all_records
            .into_iter()
            .filter(|rec| rec.scheduler_name == self.scheduler_name)
            .collect();

        // Process records without holding the lock
        for rec in records.iter() {
            info!(
                "Killing task: {:?} (scheduler: {})",
                rec.fuzzer_name, rec.scheduler_name
            );
            if let Some(name) = &rec.container_name {
                if let Some(ref binary) = self.container_binary {
                    // Use helper function to remove container
                    remove_container(binary, name);
                }
            }
            // Use helper function to kill process
            kill_process_by_pid(rec.pid);
        }

        // Remove killed records from registry
        if let Some(reg) = PROCESS_REGISTRY.get() {
            if let Ok(mut guard) = reg.lock() {
                guard.retain(|rec| rec.scheduler_name != self.scheduler_name);
            }
        }

        // Update task statuses: set Running tasks to Stopped (don't change Crashed or Stopped tasks)
        let mut status_changed = false;
        for (_sha, task) in self.tasks.iter_mut() {
            if task.status == TaskStatus::Running {
                task.status = TaskStatus::Stopped;
                task.stopped_at = Some(Instant::now());
                info!("Task {:?} marked as Stopped", task.path);
                status_changed = true;
            }
        }
        // Update state file if any task status changed
        if status_changed {
            let _ = self.write_state_file();
        }
    }

    pub fn report_crashes(&mut self) -> Result<()> {
        let mut crashed_tasks = Vec::new();
        for (_hash, task) in self.tasks.iter_mut() {
            // Only check tasks that are still running
            if task.status != TaskStatus::Running {
                continue;
            }

            match task.child.try_wait() {
                Ok(None) => {
                    // Task is still running, keep it as Running
                }
                Ok(Some(_)) => {
                    info!("CRASH - task has exited: {:?}", task.path.display());
                    // Task has exited - mark as crashed and report it
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
                    // Get crashing corpus instance
                    let crash = stderr
                        .lines()
                        .find(|l| l.starts_with("artifact_prefix="))
                        .unwrap_or("Crash detected but no element in stderr");
                    let pattern = "Test unit written to ";
                    let crash_corpus = &crash[crash.find(pattern).unwrap_or(0) + pattern.len()..];

                    // If the task was run in a docker container, crash_corpus will be like /fuzzer/crash_dir/...
                    // We want to rewrite that path to use self.crashes_directory instead.
                    let mut crash_corpus = crash_corpus.to_string();
                    if crash_corpus.starts_with("/fuzzer/crash_dir/") {
                        let fuzzer_name = task.path.file_name().and_then(|s| s.to_str()).ok_or(
                            SchedulerError::InconsistentState("invalid fuzzer path name"),
                        )?;
                        let filename = &crash_corpus["/fuzzer/crash_dir/".len()..];
                        // Reject path traversal attempts (e.g. "../../etc/shadow")
                        if filename.contains("..") {
                            return Err(SchedulerError::PathTraversal(format!(
                                "Path traversal detected in crash path: {crash_corpus}"
                            )));
                        }
                        let candidate = self.crashes_directory.join(fuzzer_name).join(filename);
                        // Canonicalize and verify the resolved path stays within crashes_directory
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

                    // Send crashing input to the backend
                    let client = reqwest::blocking::Client::new();
                    let binary = fs::read(task.path.clone())?;
                    let request =
                        client
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
                    // Task is kept in self.tasks with status Crashed, not removed
                }
                Err(e) => return Err(SchedulerError::Io(e)),
            }
        }

        // Update state file if any task crashed
        if !crashed_tasks.is_empty() {
            let _ = self.write_state_file();
        }

        Ok(())
    }

    /// Get tasks with their status and timestamps
    pub fn get_tasks_with_status(&mut self) -> Vec<TaskStatusInfo> {
        let mut result: Vec<TaskStatusInfo> = Vec::new();

        for (hash, task) in self.tasks.iter_mut() {
            // Only check tasks that are still running
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
                    // Task is still running, keep it as Running
                }
                Ok(Some(_)) => {
                    // Task has exited - mark as crashed and report it
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
                    // Task is still running, keep it as Running
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

    /// Start a task by SHA. Returns Ok(()) if successful, Err if task not found or already running.
    /// Reads var_env and cmd_options from state file if available.
    pub fn start_task_by_sha(&mut self, sha: Sha) -> Result<()> {
        // Check if task exists and is not running
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

    /// Stop a task by SHA. Returns Ok(()) if successful, Err if task not found or not running.
    pub fn stop_task_by_sha(&mut self, sha: Sha) -> Result<()> {
        if let Some(mut task) = self.tasks.remove(&sha) {
            if task.status == TaskStatus::Running {
                let task_path = task.path.clone();
                self.kill_task(&mut task);
                task.status = TaskStatus::Stopped;
                task.stopped_at = Some(Instant::now());
                self.tasks.insert(sha, task);
                info!("Task stopped: {task_path:?}");
                // Task stopped, update state file
                let _ = self.write_state_file();
                Ok(())
            } else {
                // Task is not running, just reinsert it
                self.tasks.insert(sha, task);
                Err(SchedulerError::InvalidInput(
                    "Task is not running".to_string(),
                ))
            }
        } else {
            Err(SchedulerError::InvalidInput("Task not found".to_string()))
        }
    }

    /// Restart a task by SHA. Returns Ok(()) if successful, Err if task not found.
    pub fn restart_task_by_sha(&mut self, sha: Sha) -> Result<()> {
        // Kill the task if it exists and is running
        if let Some(mut task) = self.tasks.remove(&sha) {
            if task.status == TaskStatus::Running {
                self.kill_task(&mut task);
            }
        }

        // Start the task (or restart if it was removed above)
        let task_path = self.start_task_with_state_file_options(sha)?;
        info!("Task restarted: {:?}", task_path.display());
        Ok(())
    }

    pub fn collect_coverage(&self) {
        if !self.has_coverage {
            return;
        }
        const TIMEOUT_SECS: u64 = 10; // 10 seconds timeout

        for (_, task) in self.tasks.iter() {
            // Only collect coverage for running tasks
            if task.status != TaskStatus::Running {
                continue;
            }

            info!("collecting coverage for: {:?}", task.path);

            if let Some(name) = &task.container_name {
                if let Some(ref binary) = self.container_binary {
                    collect_coverage_container(binary, name, TIMEOUT_SECS);
                }
            } else {
                let fuzzer_name = task
                    .path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("unknown");
                if fuzzer_name != "unknown" {
                    collect_coverage_native(fuzzer_name, TIMEOUT_SECS);
                } else {
                    info!("No coverage collection for: {:?}", task.path);
                }
            }
        }
    }

    /// Build the appropriate [`coverage::CoverageRunner`] for this scheduler.
    ///
    /// When a `container_image` is configured the LLVM tools are executed
    /// inside an ephemeral container so that the LLVM version matches the one
    /// that produced the `.profraw` files.
    fn coverage_runner(&self) -> std::result::Result<coverage::CoverageRunner, String> {
        match (&self.container_image, &self.container_binary) {
            (Some(image), Some(bin)) => Ok(coverage::CoverageRunner::Container {
                binary: bin.clone(),
                image: image.clone(),
                options: self.container_options.clone().unwrap_or_default(),
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

    /// Get coverage analysis for a specific task by SHA
    ///
    /// # Arguments
    ///
    /// * `sha` - SHA hash of the fuzzer binary
    ///
    /// # Returns
    ///
    /// A string containing the coverage analysis report, or an error message
    pub fn get_task_coverage(&self, sha: &Sha) -> String {
        if !self.has_coverage {
            return "Coverage is not enabled for this scheduler".to_string();
        }

        let runner = match self.coverage_runner() {
            Ok(r) => r,
            Err(e) => return e,
        };

        let task = match self.tasks.get(sha) {
            Some(t) => t,
            None => {
                return format!("Task not found for SHA: 0x{}", hex::encode(sha));
            }
        };

        let fuzzer_name = task
            .path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown");

        let coverage_dir = self.coverage_directory.join(fuzzer_name);
        let profraw_paths = task
            .profraw_path
            .as_deref()
            .map(resolve_profraw_paths)
            .unwrap_or_default();
        let profraw_refs: Vec<&Path> = profraw_paths.iter().map(PathBuf::as_path).collect();

        coverage::analyze_coverage(&coverage_dir, &task.path, sha, &profraw_refs, &runner)
    }

    /// Collect profraw paths, coverage binaries, and original binary paths
    /// from all tasks for project-wide coverage.
    ///
    /// Returns `(profraw_paths, coverage_binaries, original_paths)`:
    /// - `coverage_binaries` are the instrumented copies under `coverage_directory/`
    ///   (used as `-object` arguments for `llvm-cov`).
    /// - `original_paths` are the original task binary paths (used by
    ///   `locate_sources` to find the source tree via path-equivalence).
    fn collect_project_coverage_inputs(&self) -> (Vec<PathBuf>, Vec<PathBuf>, Vec<PathBuf>) {
        let mut profraw_paths = Vec::new();
        let mut binaries = Vec::new();
        let mut original_paths = Vec::new();

        for (sha, task) in self.tasks.iter() {
            if let Some(ref profraw) = task.profraw_path {
                profraw_paths.extend(resolve_profraw_paths(profraw));
            }
            let fuzzer_name = task
                .path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown");
            let cov_dir = self.coverage_directory.join(fuzzer_name);
            let coverage_binary = cov_dir.join(format!("fuzzer_{}.bin", hex::encode(sha)));
            if coverage_binary.exists() {
                binaries.push(coverage_binary);
            } else if task.path.exists() {
                binaries.push(task.path.clone());
            }
            original_paths.push(task.path.clone());
        }

        (profraw_paths, binaries, original_paths)
    }

    /// Get project-wide coverage by merging all task profraw files.
    ///
    /// Returns a summary report covering every fuzzer managed by this
    /// scheduler.  The merge runs `llvm-profdata merge` over all available
    /// `.profraw` files, then `llvm-cov report` with every instrumented
    /// binary as object.
    pub fn get_project_coverage(&self) -> String {
        if !self.has_coverage {
            return "Coverage is not enabled for this scheduler".to_string();
        }

        let runner = match self.coverage_runner() {
            Ok(r) => r,
            Err(e) => return e,
        };

        let (profraw_paths, binaries, original_paths) = self.collect_project_coverage_inputs();
        let profraw_refs: Vec<&Path> = profraw_paths.iter().map(|p| p.as_path()).collect();

        coverage::analyze_project_coverage(
            &self.coverage_directory,
            &profraw_refs,
            &binaries,
            &original_paths,
            &runner,
        )
    }

    /// Get project-wide file coverage (or all files).
    ///
    /// Same merge + multi-object logic as [`get_project_coverage`] but with
    /// `llvm-cov show` for line-by-line annotated source.  Falls back to
    /// region-level data when sources are unavailable.
    pub fn get_project_file_coverage(&self, file_path: Option<&Path>) -> String {
        if !self.has_coverage {
            return "Coverage is not enabled for this scheduler".to_string();
        }

        let runner = match self.coverage_runner() {
            Ok(r) => r,
            Err(e) => return e,
        };

        let (profraw_paths, binaries, original_paths) = self.collect_project_coverage_inputs();
        let profraw_refs: Vec<&Path> = profraw_paths.iter().map(|p| p.as_path()).collect();

        coverage::get_project_file_coverage(
            &self.coverage_directory,
            &profraw_refs,
            &binaries,
            &original_paths,
            file_path,
            &runner,
        )
    }

    /// Compute a canonical fingerprint for the current set of target binaries.
    ///
    /// The fingerprint is the sorted hex-encoded SHA3-256 hashes joined by `:`.
    fn compute_build_fingerprint(shas: &HashSet<Sha>) -> String {
        let mut sorted: Vec<String> = shas.iter().map(hex::encode).collect();
        sorted.sort();
        sorted.join(":")
    }

    /// Capture and send a coverage snapshot to the backend.
    ///
    /// Called before binary transitions (stop/start) when the set of targets is
    /// about to change. Silently does nothing when coverage is disabled, tools
    /// are unavailable, or the profile data cannot be computed.
    fn snapshot_coverage_before_transition(&self, outgoing_shas: &HashSet<Sha>) {
        if !self.has_coverage || self.tasks.is_empty() {
            return;
        }

        let runner = match self.coverage_runner() {
            Ok(r) => r,
            Err(_) => return,
        };

        let (profraw_paths, binaries, _) = self.collect_project_coverage_inputs();
        let profraw_refs: Vec<&Path> = profraw_paths.iter().map(|p| p.as_path()).collect();

        let summary = match coverage::compute_project_summary(
            &self.coverage_directory,
            &profraw_refs,
            &binaries,
            &runner,
        ) {
            Some(s) => s,
            None => return,
        };

        let fingerprint = Self::compute_build_fingerprint(outgoing_shas);

        #[derive(serde::Serialize)]
        struct SnapshotPayload {
            scheduler_name: String,
            fuzzer_engine: String,
            build_fingerprint: String,
            num_fuzzers: i32,
            metrics: Vec<coverage::CoverageMetricEntry>,
        }

        let payload = SnapshotPayload {
            scheduler_name: self.scheduler_name.clone(),
            fuzzer_engine: "libfuzzer".to_string(),
            build_fingerprint: fingerprint,
            num_fuzzers: self.tasks.len() as i32,
            metrics: summary.metrics,
        };

        let url = format!("{}/coverage_snapshots", self.backend_url);
        match reqwest::blocking::Client::new()
            .post(&url)
            .json(&payload)
            .timeout(Duration::from_secs(10))
            .send()
        {
            Ok(resp) if resp.status().is_success() => {
                info!("Coverage snapshot sent to backend for scheduler={}", self.scheduler_name);
            }
            Ok(resp) => {
                debug!(
                    "Coverage snapshot rejected by backend: {}",
                    resp.status()
                );
            }
            Err(e) => {
                debug!("Failed to send coverage snapshot: {e}");
            }
        }
    }

    /// Get coverage for a specific file (or all files) in a task.
    ///
    /// When `file_path` is `Some`, returns annotated source for that file.
    /// When `file_path` is `None`, returns annotated source for all files (TOTAL).
    pub fn get_task_file_coverage(&self, sha: &Sha, file_path: Option<&Path>) -> String {
        info!("Start file coverage for: {file_path:?}");
        if !self.has_coverage {
            return "Coverage is not enabled for this scheduler".to_string();
        }

        let runner = match self.coverage_runner() {
            Ok(r) => r,
            Err(e) => return e,
        };

        let task = match self.tasks.get(sha) {
            Some(t) => t,
            None => return format!("Task not found for SHA: 0x{}", hex::encode(sha)),
        };

        let fuzzer_name = task
            .path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown");
        let coverage_dir = self.coverage_directory.join(fuzzer_name);
        let profraw_paths = task
            .profraw_path
            .as_deref()
            .map(resolve_profraw_paths)
            .unwrap_or_default();
        let profraw_refs: Vec<&Path> = profraw_paths.iter().map(PathBuf::as_path).collect();

        coverage::get_file_coverage(
            &coverage_dir,
            &task.path,
            sha,
            &profraw_refs,
            file_path,
            &runner,
        )
    }
}

#[cfg(test)]
mod profraw_path_tests {
    use super::resolve_profraw_paths;
    use assert_fs::prelude::*;

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
}
