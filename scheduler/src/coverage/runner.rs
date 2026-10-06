// SPDX-FileCopyrightText: © 2026 LEDGER SAS
// SPDX-License-Identifier: MIT

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};

/// Where LLVM coverage tools (`llvm-profdata`, `llvm-cov`) are executed.
///
/// In container mode the same image that ran the fuzzer is used to run the
/// tools inside an ephemeral `run --rm` container, ensuring the LLVM version
/// matches the one that produced the `.profraw` / coverage-mapped binary.
pub enum CoverageRunner {
    /// Run LLVM tools directly on the host.
    Host { llvm_bin_dir: Option<PathBuf> },
    /// Run LLVM tools inside an ephemeral container (`run --rm`).
    Container {
        binary: PathBuf,
        image: String,
        options: Vec<String>,
        llvm_bin_dir: Option<PathBuf>,
    },
}

/// A bind-mount for a container coverage command.
pub(super) struct Mount {
    pub host_path: PathBuf,
    pub read_only: bool,
}

/// Build the full argument list for a containerised coverage tool invocation.
///
/// The resulting vector is suitable for `Command::new(binary).args(...)`.
pub(super) fn build_container_args(
    image: &str,
    options: &[String],
    tool: &str,
    tool_args: &[String],
    mounts: &[Mount],
) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "run".into(),
        "--rm".into(),
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
    ];

    // Container runtimes reject the same mount point twice, and several
    // mount sources (coverage dir, binary dir, source trees) can coincide.
    let mut seen = HashSet::new();
    for m in mounts {
        if !seen.insert(m.host_path.clone()) {
            continue;
        }
        args.push("-v".into());
        let host = m.host_path.to_string_lossy();
        if m.read_only {
            args.push(format!("{host}:{host}:ro"));
        } else {
            args.push(format!("{host}:{host}"));
        }
    }

    for opt in options {
        if !opt.is_empty() {
            args.push(opt.clone());
        }
    }

    args.push(image.into());
    args.push(tool.into());
    args.extend(tool_args.iter().cloned());
    args
}

/// Format a "tool not found in image" error message.
pub(super) fn tool_not_found_error(tool: &str, image: &str) -> String {
    format!(
        "{tool} not found in container image '{image}'.\n\
         The image used to run fuzzers must also provide llvm-profdata and llvm-cov \
         (see fuzzwizard_scheduler_test/docker/ubuntu-llvm.Dockerfile), or set \
         llvm_bin_directory to their path inside the image."
    )
}

/// Render a failed tool invocation, telling "the image lacks the tool" apart
/// from "the tool ran and refused its inputs".
pub(super) fn tool_failure(
    runner: &CoverageRunner,
    tool: &str,
    action: &str,
    status: &ExitStatus,
    stderr: &str,
) -> String {
    if let Some(image) = runner.image_name() {
        if is_tool_not_found(status, stderr, tool) {
            return tool_not_found_error(tool, image);
        }
    }
    format!("{tool} {action} failed:\n{stderr}")
}

/// Returns `true` when the command output indicates that `tool` itself could
/// not be executed, as opposed to the tool running and rejecting its inputs.
///
/// A missing `.profraw` also makes `llvm-profdata` print "no such file or
/// directory", so that wording only counts when the message is about the tool.
pub(super) fn is_tool_not_found(status: &ExitStatus, stderr: &str, tool: &str) -> bool {
    if status.code() == Some(127) {
        return true;
    }
    let lower = stderr.to_lowercase();
    if lower.contains("executable file not found") {
        return true;
    }
    let tool_name = Path::new(tool)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(tool)
        .to_lowercase();
    lower.contains("no such file or directory") && lower.contains(&tool_name)
}

impl CoverageRunner {
    /// Build a [`Command`] that runs `tool` with the given arguments.
    ///
    /// In `Host` mode the tool is resolved via `llvm_tool()`.
    /// In `Container` mode an ephemeral `run --rm` container is launched with
    /// identity mounts so that all path arguments remain valid inside the
    /// container.
    pub(super) fn command(&self, tool: &str, tool_args: &[String], mounts: &[Mount]) -> Command {
        match self {
            CoverageRunner::Host { llvm_bin_dir } => {
                let exe = llvm_tool(tool, llvm_bin_dir.as_deref());
                let mut cmd = Command::new(exe);
                cmd.args(tool_args);
                cmd
            }
            CoverageRunner::Container {
                binary,
                image,
                options,
                llvm_bin_dir,
            } => {
                let resolved = llvm_tool_container(tool, llvm_bin_dir.as_deref());
                let args = build_container_args(image, options, &resolved, tool_args, mounts);
                let mut cmd = Command::new(binary);
                cmd.args(&args);
                cmd
            }
        }
    }

    pub(super) fn image_name(&self) -> Option<&str> {
        match self {
            CoverageRunner::Host { .. } => None,
            CoverageRunner::Container { image, .. } => Some(image),
        }
    }
}

/// Resolve LLVM tool binary: config directory first, then `FUZZWIZARD_LLVM_BIN`, then `PATH`.
fn llvm_tool(name: &str, config_bin_dir: Option<&Path>) -> String {
    const ENV: &str = "FUZZWIZARD_LLVM_BIN";
    if let Some(dir) = config_bin_dir {
        let p = dir.join(name);
        if p.is_file() {
            return p.to_string_lossy().into_owned();
        }
    }
    if let Ok(dir) = std::env::var(ENV) {
        let p = Path::new(dir.trim()).join(name);
        if p.is_file() {
            return p.to_string_lossy().into_owned();
        }
    }
    name.to_string()
}

/// Resolve LLVM tool inside a container: prepend `llvm_bin_dir` if set,
/// otherwise just return the bare tool name (resolved via container PATH).
pub(super) fn llvm_tool_container(name: &str, llvm_bin_dir: Option<&Path>) -> String {
    if let Some(dir) = llvm_bin_dir {
        dir.join(name).to_string_lossy().into_owned()
    } else {
        name.to_string()
    }
}
