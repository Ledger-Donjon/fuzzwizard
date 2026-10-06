// SPDX-FileCopyrightText: © 2026 LEDGER SAS
// SPDX-License-Identifier: MIT

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::time::Duration;
use std::{fs, thread};

use log::{info, warn};

use crate::types::ProcessRecord;

pub(crate) static PROCESS_REGISTRY: OnceLock<Arc<Mutex<Vec<ProcessRecord>>>> = OnceLock::new();

pub(crate) fn kill_process_by_pid(pid: u32) {
    let _ = thread::spawn(move || {
        let _ = Command::new("/bin/kill")
            .args(["-TERM", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .and_then(|mut child| child.wait());
    });
}

pub(crate) fn remove_container(binary: &Path, name: &str) {
    let binary_clone = binary.to_path_buf();
    let name_clone = name.to_string();
    let _ = thread::spawn(move || {
        let _ = Command::new(&binary_clone)
            .args(["rm", "-f", &name_clone])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .and_then(|mut child| child.wait());
    });
}

/// Resolve the files produced by LLVM's `%m` online-merge filename token.
///
/// The runtime expands `%m` to a load module signature that cannot be computed
/// ahead of time, so the directory is listed instead. A fuzzer normally yields
/// a single file; the `<sha>-` prefix keeps profiles of other builds out.
pub(crate) fn resolve_profraw_paths(pattern: &Path) -> Vec<PathBuf> {
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

/// File name of a fuzzer binary, used to derive its per-fuzzer directories.
pub(crate) fn fuzzer_name_of(path: &Path) -> &str {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("unknown")
}

/// Ask a fuzzer to flush its `.profraw` by sending it `SIGUSR1`.
///
/// libFuzzer dumps the profile on that signal, which is the only way to read
/// coverage without stopping the campaign. The call is bounded because `pkill`
/// goes through the container runtime, which can hang on a wedged daemon.
pub(crate) fn flush_coverage(mut command: Command, subject: &str, timeout: Duration) {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let status = command
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .and_then(|mut child| child.wait());
        let _ = tx.send(status);
    });

    match rx.recv_timeout(timeout) {
        Ok(Ok(status)) if status.success() => {
            info!("Coverage collection successful for {subject}");
        }
        Ok(Ok(status)) => {
            warn!("Coverage collection failed for {subject}: {status:?}");
        }
        Ok(Err(e)) => {
            warn!("Failed to execute coverage collection for {subject}: {e}");
        }
        Err(mpsc::RecvTimeoutError::Timeout) => {
            warn!("Coverage collection timed out ({timeout:?}) for {subject}");
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            warn!("Coverage collection thread disconnected for {subject}");
        }
    }
}

pub(crate) fn install_ctrlc_handler() {
    let registry = PROCESS_REGISTRY
        .get_or_init(|| Arc::new(Mutex::new(Vec::new())))
        .clone();
    let _ = ctrlc::set_handler(move || {
        let records: Vec<ProcessRecord> = {
            match registry.lock() {
                Ok(guard) => guard.clone(),
                Err(poisoned) => {
                    warn!("Process registry was poisoned, recovering data");
                    poisoned.into_inner().clone()
                }
            }
        };

        for rec in records.iter() {
            if let Some(name) = &rec.container_name {
                let name_clone = name.clone();
                let _ = Command::new("/usr/bin/docker")
                    .args(["rm", "-f", &name_clone])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .or_else(|_| {
                        Command::new("/usr/bin/podman")
                            .args(["rm", "-f", &name_clone])
                            .stdout(Stdio::null())
                            .stderr(Stdio::null())
                            .status()
                    });
            };
            kill_process_by_pid(rec.pid);
        }
        thread::sleep(Duration::from_millis(100));
        std::process::exit(130);
    });
}
