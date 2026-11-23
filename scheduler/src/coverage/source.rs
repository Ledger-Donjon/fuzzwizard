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

/// Directory names that hold credentials or user configuration. A coverage
/// mapping naming a file in one of them is no reason to expose it.
const SENSITIVE_DIR_NAMES: &[&str] = &[
    ".aws",
    ".azure",
    ".config",
    ".docker",
    ".fuzzwizard",
    ".gnupg",
    ".kube",
    ".local",
    ".password-store",
    ".ssh",
];

/// Prefixes of toolchain and system headers (`/usr/include/...`). Nearly every
/// coverage mapping references some, so finding them on the host says nothing
/// about where the project sources are.
const SYSTEM_SOURCE_PREFIXES: &[&str] = &["/usr", "/nix/store"];

/// Exceptions to [`SYSTEM_SOURCE_PREFIXES`]: projects do live there.
const PROJECT_SOURCE_PREFIXES: &[&str] = &["/usr/src", "/usr/local/src"];

fn is_system_source(path: &Path) -> bool {
    SYSTEM_SOURCE_PREFIXES
        .iter()
        .any(|prefix| path.starts_with(prefix))
        && !PROJECT_SOURCE_PREFIXES
            .iter()
            .any(|prefix| path.starts_with(prefix))
}

/// Whether `dir` is specific enough to be exposed to a coverage container.
///
/// This is a path-string check only (no filesystem access). Call
/// [`resolve_safe_source_mount`] before actually bind-mounting, so symlinks
/// cannot sneak a forbidden tree past the prefix list.
pub(super) fn is_safe_source_mount(dir: &Path) -> bool {
    if !dir.is_absolute() {
        return false;
    }
    if dir.components().count() <= MIN_SOURCE_MOUNT_DEPTH {
        return false;
    }
    if is_home_root(dir) || has_sensitive_component(dir) {
        return false;
    }
    // `/var` is otherwise forbidden, but `/var/tmp` is a normal staging area
    // (and `canonicalize` of `/tmp` lands here on some distros).
    if dir.starts_with("/var/tmp") {
        return true;
    }
    !FORBIDDEN_SOURCE_MOUNTS
        .iter()
        .any(|forbidden| dir.starts_with(forbidden))
}

/// A home directory holds the user's credentials next to their projects:
/// `/home/<user>/proj` may be mounted, `/home/<user>` may not.
fn is_home_root(dir: &Path) -> bool {
    let under_home = dir.starts_with("/home") && dir.components().count() == 3;
    under_home || std::env::var_os("HOME").is_some_and(|home| dir == Path::new(&home))
}

fn has_sensitive_component(dir: &Path) -> bool {
    dir.components().any(|c| {
        c.as_os_str()
            .to_str()
            .is_some_and(|name| SENSITIVE_DIR_NAMES.contains(&name))
    })
}

/// Canonicalize `dir` and re-apply [`is_safe_source_mount`] on the resolved
/// target. Returns `None` if the path does not exist, is not a directory, or
/// resolves to a forbidden / too-shallow location.
pub(super) fn resolve_safe_source_mount(dir: &Path) -> Option<PathBuf> {
    if !is_safe_source_mount(dir) {
        return None;
    }
    let canonical = dir.canonicalize().ok()?;
    if !canonical.is_dir() {
        return None;
    }
    is_safe_source_mount(&canonical).then_some(canonical)
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
/// Two situations are handled. Either the project sources sit at exactly the
/// path recorded in the binary — no `-path-equivalence` is needed, but a
/// bind-mount still is for container runs — or the tree was moved, in which
/// case we walk up from each entry of `binary_paths` to derive a
/// recorded→host mapping. System headers are only ever mounted alongside, and
/// never decide between the two.
///
/// Only directories accepted by [`resolve_safe_source_mount`] are ever returned.
pub(super) fn locate_sources(
    embedded_paths: &[String],
    binary_paths: &[PathBuf],
) -> Option<SourceLocation> {
    if embedded_paths.is_empty() {
        return None;
    }

    let (system, project): (Vec<String>, Vec<String>) = embedded_paths
        .iter()
        .cloned()
        .partition(|p| is_system_source(Path::new(p)));

    let present_project = present_source_dirs(&project);
    if !present_project.is_empty() || project.is_empty() {
        // Project directories first, so that a long list of headers cannot
        // push them out of the mount limit.
        let mut mounts = dedup_ancestors(present_project);
        mounts.extend(dedup_ancestors(present_source_dirs(&system)));
        mounts.truncate(MAX_SOURCE_MOUNTS);
        if mounts.is_empty() {
            return None;
        }
        info!("Sources found at their recorded paths, mounting {mounts:?}");
        return Some(SourceLocation { flag: None, mounts });
    }

    for binary_path in binary_paths {
        if let Some(loc) = locate_moved_sources(&project, binary_path) {
            return Some(loc);
        }
    }

    warn!(
        "No source tree found on host for {} embedded paths",
        embedded_paths.len()
    );
    None
}

/// Mountable host directories of the recorded sources that exist on the host.
fn present_source_dirs(paths: &[String]) -> Vec<PathBuf> {
    paths
        .iter()
        .map(Path::new)
        .filter(|p| p.is_file())
        .filter_map(|p| p.parent())
        .filter_map(resolve_safe_source_mount)
        .collect()
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
                if let Some(mount) = resolve_safe_source_mount(dir) {
                    let host_str = mount.to_string_lossy();
                    let emb_str = emb_dir.to_string_lossy();
                    let flag = format!("-path-equivalence={emb_str},{host_str}");
                    info!("Path equivalence detected: {flag}");
                    return Some(SourceLocation {
                        flag: Some(flag),
                        mounts: vec![mount],
                    });
                }
            }
            candidate = dir.parent();
        }
    }

    None
}
