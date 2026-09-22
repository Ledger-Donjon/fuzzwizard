// SPDX-FileCopyrightText: © 2026 LEDGER SAS
// SPDX-License-Identifier: MIT

use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::fs;

use sha3::{Digest, Sha3_256};

pub type Sha = [u8; 32];

/// A trait for collecting targets to be fuzzed by the scheduler.
pub trait TargetsCollector {
    fn collect_targets(&self) -> HashMap<Sha, PathBuf>;
}

/// A collector that scans directories for executable fuzzer binaries
pub struct DirectoryTargetsCollector {
    targets_directories: Vec<PathBuf>,
}

impl DirectoryTargetsCollector {
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
            let read_dir = match fs::read_dir(directory) {
                Ok(rd) => rd,
                Err(e) => {
                    log::error!("Failed to read directory {directory:?}: {e}");
                    continue;
                }
            };

            for direntry_result in read_dir {
                let direntry = match direntry_result {
                    Ok(de) => de,
                    Err(e) => {
                        log::error!("Failed to read directory entry in {directory:?}: {e}");
                        continue;
                    }
                };

                let target_path = direntry.path();

                if !target_path.is_file() {
                    continue;
                }

                let metadata = match target_path.metadata() {
                    Ok(m) => m,
                    Err(e) => {
                        log::error!("Failed to get metadata for {target_path:?}: {e}");
                        continue;
                    }
                };

                if metadata.permissions().mode() & 0o111 == 0 {
                    continue;
                }

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
