// SPDX-FileCopyrightText: © 2026 LEDGER SAS
// SPDX-License-Identifier: MIT

use std::path::{Path, PathBuf};

use log::{info, warn};

/// Where the sources referenced by the coverage mapping live on the host.
#[derive(Debug)]
pub(super) struct SourceLocation {
    /// `-path-equivalence=FROM,TO` flag, needed only when the on-disk location
    /// differs from the path recorded in the binary.
    pub flag: Option<String>,
    /// Host directories to bind-mount read-only so containerised `llvm-cov`
    /// can read the sources.
    pub mounts: Vec<PathBuf>,
}

/// Upper bound on source directories bind-mounted into a coverage container.
const MAX_SOURCE_MOUNTS: usize = 16;

/// Shallowest directory that may be bind-mounted into a coverage container.
///
/// `/src/a.c` is fine, `/etc` or `/` is not: the paths come from the coverage
/// mapping of a binary the scheduler did not build, and the upward walk in
/// [`locate_sources`] would otherwise happily settle on a system directory.
const MIN_SOURCE_MOUNT_DEPTH: usize = 2;

/// Directories that are never mounted, whatever their depth.
const FORBIDDEN_SOURCE_MOUNTS: &[&str] = &[
    "/bin", "/boot", "/dev", "/etc", "/lib", "/lib32", "/lib64", "/proc", "/root", "/run", "/sbin",
    "/sys", "/var",
];

/// Whether `dir` is specific enough to be exposed to a coverage container.
pub(super) fn is_safe_source_mount(dir: &Path) -> bool {
    if !dir.is_absolute() {
        return false;
    }
    if dir.components().count() <= MIN_SOURCE_MOUNT_DEPTH {
        return false;
    }
    !FORBIDDEN_SOURCE_MOUNTS
        .iter()
        .any(|forbidden| dir.starts_with(forbidden))
}

/// Drop directories already covered by another entry so nested source trees
/// collapse into a single mount.
pub(super) fn dedup_ancestors(mut dirs: Vec<PathBuf>) -> Vec<PathBuf> {
    dirs.sort();
    dirs.dedup();
    let mut kept: Vec<PathBuf> = Vec::new();
    for dir in dirs {
        if kept.iter().any(|k| dir.starts_with(k)) {
            continue;
        }
        kept.push(dir);
    }
    kept
}

/// Locate on the host the sources referenced by the coverage mapping.
///
/// Two situations are handled. Either the sources sit at exactly the path
/// recorded in the binary — no `-path-equivalence` is needed, but a bind-mount
/// still is for container runs — or the tree was moved, in which case we walk
/// up from each entry of `binary_paths` to derive a recorded→host mapping.
///
/// Only directories accepted by [`is_safe_source_mount`] are ever returned.
pub(super) fn locate_sources(
    embedded_paths: &[String],
    binary_paths: &[PathBuf],
) -> Option<SourceLocation> {
    if embedded_paths.is_empty() {
        return None;
    }

    let present: Vec<PathBuf> = embedded_paths
        .iter()
        .map(Path::new)
        .filter(|p| p.is_file())
        .filter_map(|p| p.parent().map(Path::to_path_buf))
        .filter(|dir| is_safe_source_mount(dir))
        .collect();
    if !present.is_empty() {
        let mut mounts = dedup_ancestors(present);
        mounts.truncate(MAX_SOURCE_MOUNTS);
        info!("Sources found at their recorded paths, mounting {mounts:?}");
        return Some(SourceLocation { flag: None, mounts });
    }

    for binary_path in binary_paths {
        if let Some(loc) = locate_moved_sources(embedded_paths, binary_path) {
            return Some(loc);
        }
    }

    warn!(
        "No source tree found on host for {} embedded paths",
        embedded_paths.len()
    );
    None
}

/// Walk up from `binary_path` looking for a directory holding a file named
/// like one of the recorded sources, and derive the `-path-equivalence` mapping.
fn locate_moved_sources(embedded_paths: &[String], binary_path: &Path) -> Option<SourceLocation> {
    let start_dir = if binary_path.is_dir() {
        binary_path.to_path_buf()
    } else {
        binary_path.parent()?.to_path_buf()
    };

    for emb in embedded_paths {
        let emb_path = Path::new(emb);
        let Some(emb_name) = emb_path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        let emb_dir = emb_path.parent().unwrap_or(Path::new("/"));

        let mut candidate = Some(start_dir.as_path());
        while let Some(dir) = candidate {
            if !is_safe_source_mount(dir) {
                break;
            }
            if dir.join(emb_name).is_file() {
                let host_str = dir.to_string_lossy();
                let emb_str = emb_dir.to_string_lossy();
                let flag = format!("-path-equivalence={emb_str},{host_str}");
                info!("Path equivalence detected: {flag}");
                return Some(SourceLocation {
                    flag: Some(flag),
                    mounts: vec![dir.to_path_buf()],
                });
            }
            candidate = dir.parent();
        }
    }

    None
}
