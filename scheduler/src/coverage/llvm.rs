// SPDX-FileCopyrightText: © 2026 LEDGER SAS
// SPDX-License-Identifier: MIT

use std::path::{Path, PathBuf};

use log::debug;
use serde_json::Value;

use super::render::parse_filenames_from_export;
use super::runner::{tool_failure, CoverageRunner, Mount};
use super::source::SourceLocation;

/// Merge one or more `.profraw` files into an indexed `.profdata` via
/// `llvm-profdata merge`.
pub(super) fn merge_profdata(
    profraw_paths: &[&Path],
    profdata_out: &Path,
    coverage_dir: &Path,
    runner: &CoverageRunner,
) -> Result<PathBuf, String> {
    if profraw_paths.is_empty() {
        return Err("No profraw files available for coverage merge".to_string());
    }
    let mut tool_args: Vec<String> = vec!["merge".into(), "-sparse".into()];
    for p in profraw_paths {
        tool_args.push(p.to_string_lossy().into_owned());
    }
    tool_args.push("-o".into());
    tool_args.push(profdata_out.to_string_lossy().into_owned());

    let mounts = vec![Mount {
        host_path: coverage_dir.to_path_buf(),
        read_only: false,
    }];
    let output = runner
        .command("llvm-profdata", &tool_args, &mounts)
        .output()
        .map_err(|e| format!("llvm-profdata not available: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(tool_failure(
            runner,
            "llvm-profdata",
            "merge",
            &output.status,
            &stderr,
        ));
    }
    Ok(profdata_out.to_path_buf())
}

/// Run `llvm-cov export` and parse its JSON output.
///
/// `summary_only` keeps the payload small when only file names and totals are
/// needed; the full export additionally carries per-function regions, which is
/// what lets us report coverage when the sources themselves are unavailable.
pub(super) fn export_json(
    profdata: &Path,
    binaries: &[PathBuf],
    runner: &CoverageRunner,
    coverage_dir: &Path,
    summary_only: bool,
) -> Option<Value> {
    if binaries.is_empty() {
        return None;
    }
    let mut tool_args: Vec<String> = vec!["export".into()];
    if summary_only {
        tool_args.push("--summary-only".into());
    }
    tool_args.extend([
        "-instr-profile".into(),
        profdata.to_string_lossy().into_owned(),
    ]);
    push_binary_args(&mut tool_args, binaries);
    let mounts = coverage_mounts(coverage_dir, binaries);
    let output = match runner.command("llvm-cov", &tool_args, &mounts).output() {
        Ok(o) if o.status.success() => o,
        Ok(o) => {
            debug!(
                "llvm-cov export failed: {}",
                String::from_utf8_lossy(&o.stderr)
            );
            return None;
        }
        Err(e) => {
            debug!("llvm-cov export not available: {e}");
            return None;
        }
    };

    match serde_json::from_slice(&output.stdout) {
        Ok(v) => Some(v),
        Err(e) => {
            debug!("llvm-cov export produced invalid JSON: {e}");
            None
        }
    }
}

/// Extract source file paths embedded in the binary(ies) via
/// `llvm-cov export --summary-only`.
pub(super) fn embedded_source_paths(
    profdata: &Path,
    binaries: &[PathBuf],
    runner: &CoverageRunner,
    coverage_dir: &Path,
) -> Vec<String> {
    let Some(json) = export_json(profdata, binaries, runner, coverage_dir, true) else {
        return Vec::new();
    };
    let result = parse_filenames_from_export(&json);
    debug!(
        "llvm-cov export found {} filenames: {:?}",
        result.len(),
        result
    );
    result
}

/// Collect the base mounts needed for coverage commands: `coverage_dir` (rw)
/// and each binary's parent directory (ro, only if it lives outside `coverage_dir`).
pub(super) fn coverage_mounts(coverage_dir: &Path, binaries: &[PathBuf]) -> Vec<Mount> {
    let mut mounts = vec![Mount {
        host_path: coverage_dir.to_path_buf(),
        read_only: false,
    }];
    for binary in binaries {
        if let Some(bin_dir) = binary.parent() {
            if !bin_dir.starts_with(coverage_dir) {
                mounts.push(Mount {
                    host_path: bin_dir.to_path_buf(),
                    read_only: true,
                });
            }
        }
    }
    mounts
}

/// Push multi-object binary arguments for `llvm-cov` subcommands.
///
/// The first binary is a positional argument; additional binaries use
/// `-object` flags.
fn push_binary_args(tool_args: &mut Vec<String>, binaries: &[PathBuf]) {
    if let Some(first) = binaries.first() {
        tool_args.push(first.to_string_lossy().into_owned());
    }
    for extra in binaries.iter().skip(1) {
        tool_args.push("-object".into());
        tool_args.push(extra.to_string_lossy().into_owned());
    }
}

/// Append the `-path-equivalence` flag required by `source_loc`, if any.
fn push_source_flag(tool_args: &mut Vec<String>, source_loc: Option<&SourceLocation>) {
    if let Some(flag) = source_loc.and_then(|loc| loc.flag.as_ref()) {
        tool_args.push(flag.clone());
    }
}

/// Append the read-only source-tree mounts required by `source_loc`, if any.
fn push_source_mounts(mounts: &mut Vec<Mount>, source_loc: Option<&SourceLocation>) {
    let Some(loc) = source_loc else { return };
    for dir in &loc.mounts {
        mounts.push(Mount {
            host_path: dir.clone(),
            read_only: true,
        });
    }
}

/// Run `llvm-cov report` and return the text output.
pub(super) fn run_report(
    profdata: &Path,
    binaries: &[PathBuf],
    runner: &CoverageRunner,
    source_loc: Option<&SourceLocation>,
    coverage_dir: &Path,
) -> String {
    if binaries.is_empty() {
        return "No binaries available for coverage report".to_string();
    }
    let mut tool_args: Vec<String> = vec!["report".into()];
    push_binary_args(&mut tool_args, binaries);
    tool_args.extend([
        "-instr-profile".into(),
        profdata.to_string_lossy().into_owned(),
        "-use-color=false".into(),
    ]);
    push_source_flag(&mut tool_args, source_loc);
    let mut mounts = coverage_mounts(coverage_dir, binaries);
    push_source_mounts(&mut mounts, source_loc);
    match runner.command("llvm-cov", &tool_args, &mounts).output() {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).to_string(),
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr);
            tool_failure(runner, "llvm-cov", "report", &o.status, &stderr)
        }
        Err(e) => format!("llvm-cov not available: {e}"),
    }
}

/// Outcome of a successful `llvm-cov show` invocation.
pub(super) enum ShowOutcome {
    /// Annotated source text for the requested file (or all files).
    Annotated(String),
    /// No annotated source could be produced. Carries the source paths that
    /// `llvm-cov` explicitly reported as unreadable, which may be empty when
    /// the file is simply absent from the output.
    NoSource(Vec<String>),
}

/// Extract from `llvm-cov` stderr the source paths it could not read.
///
/// `llvm-cov show` exits with status 0 when a source file is missing: it only
/// writes `error: <path>: No such file or directory` followed by
/// `warning: The file '<path>' isn't covered.` and skips the file. Without
/// parsing stderr the caller cannot tell "no coverage" from "no sources".
pub(super) fn parse_unreadable_sources(stderr: &str) -> Vec<String> {
    let mut paths: Vec<String> = Vec::new();
    for line in stderr.lines() {
        let line = line.trim();
        let found = if let Some(rest) = line.strip_prefix("warning: The file '") {
            rest.strip_suffix("' isn't covered.").map(str::to_string)
        } else if let Some(rest) = line.strip_prefix("error: ") {
            rest.rsplit_once(": ").and_then(|(path, message)| {
                let m = message.to_lowercase();
                (m.contains("no such file") || m.contains("permission denied"))
                    .then(|| path.to_string())
            })
        } else {
            None
        };
        if let Some(path) = found {
            if !paths.contains(&path) {
                paths.push(path);
            }
        }
    }
    paths
}

/// Run `llvm-cov show` and return annotated source.
///
/// When `source_file` is `None`, the full output (all files) is returned.
/// When `source_file` is `Some`, only the section matching that file is extracted.
///
/// Because `-path-equivalence` interacts unpredictably with the source-file
/// filter argument across LLVM versions (see llvm/llvm-project#62689), we run
/// `llvm-cov show` **without** a source filter and extract the section that
/// matches the requested file from the full output.
pub(super) fn run_show(
    profdata: &Path,
    binaries: &[PathBuf],
    source_file: Option<&Path>,
    runner: &CoverageRunner,
    source_loc: Option<&SourceLocation>,
    coverage_dir: &Path,
) -> Result<ShowOutcome, String> {
    if binaries.is_empty() {
        return Err("No binaries available for coverage show".to_string());
    }
    let mut tool_args: Vec<String> = vec![
        "show".into(),
        "-instr-profile".into(),
        profdata.to_string_lossy().into_owned(),
    ];
    push_binary_args(&mut tool_args, binaries);
    tool_args.extend([
        "-format=text".into(),
        "-show-line-counts-or-regions".into(),
        "-use-color=false".into(),
    ]);
    push_source_flag(&mut tool_args, source_loc);

    let mut mounts = coverage_mounts(coverage_dir, binaries);
    push_source_mounts(&mut mounts, source_loc);

    debug!("llvm-cov show tool_args: {:?}", tool_args);

    let output = runner
        .command("llvm-cov", &tool_args, &mounts)
        .output()
        .map_err(|e| format!("llvm-cov not available: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(tool_failure(
            runner,
            "llvm-cov",
            "show",
            &output.status,
            &stderr,
        ));
    }

    let unreadable = parse_unreadable_sources(&String::from_utf8_lossy(&output.stderr));
    if !unreadable.is_empty() {
        debug!(
            "llvm-cov could not read {} source file(s)",
            unreadable.len()
        );
    }

    let full_output = String::from_utf8_lossy(&output.stdout).to_string();
    if full_output.trim().is_empty() {
        return Ok(ShowOutcome::NoSource(unreadable));
    }

    let source_file = match source_file {
        Some(sf) => sf,
        None => return Ok(ShowOutcome::Annotated(full_output)),
    };

    let file_name = source_file
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    let source_str = source_file.to_string_lossy();

    debug!(
        "run_show: extracting section for {:?} (basename {:?}) from {} bytes",
        source_str,
        file_name,
        full_output.len()
    );

    match extract_file_section(&full_output, &source_str, file_name) {
        Some(section) => Ok(ShowOutcome::Annotated(section)),
        None => Ok(ShowOutcome::NoSource(unreadable)),
    }
}

/// Extract the annotated-source section for a single file from `llvm-cov show` text output.
///
/// `llvm-cov show -format=text` separates files with a header line of the form:
///
/// ```text
/// /path/to/source.c:
///     1|   10|int main() {
/// ```
///
/// We match on the full path first, then fall back to matching the basename.
fn extract_file_section(full_output: &str, source_path: &str, basename: &str) -> Option<String> {
    let lines: Vec<&str> = full_output.lines().collect();

    // A file header is a line that ends with ':' and whose content (minus the colon)
    // looks like a file path (contains '/' or '\').
    let is_file_header = |line: &str| -> bool {
        let t = line.trim();
        t.ends_with(':') && {
            let path_part = &t[..t.len() - 1];
            (path_part.contains('/') || path_part.contains('\\')) && !path_part.contains('|')
        }
    };

    // Find all file sections: (header_line_index, path_in_header)
    let sections: Vec<(usize, &str)> = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| is_file_header(l))
        .map(|(i, l)| {
            let t = l.trim();
            (i, &t[..t.len() - 1])
        })
        .collect();

    if sections.is_empty() {
        return Some(full_output.to_string());
    }

    debug!("extract_file_section: {} sections found", sections.len());

    let target = sections
        .iter()
        .find(|(_, p)| *p == source_path)
        .or_else(|| {
            sections
                .iter()
                .find(|(_, p)| Path::new(p).file_name().and_then(|n| n.to_str()) == Some(basename))
        });

    let &(start_idx, matched_path) = target?;
    debug!("extract_file_section: matched {matched_path:?}");

    let end_idx = sections
        .iter()
        .find(|(i, _)| *i > start_idx)
        .map(|(i, _)| *i)
        .unwrap_or(lines.len());

    let section: String = lines[start_idx..end_idx].to_vec().join("\n");

    Some(section)
}
