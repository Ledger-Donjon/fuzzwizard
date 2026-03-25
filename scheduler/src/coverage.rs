// SPDX-FileCopyrightText: © 2026 LEDGER SAS
// SPDX-License-Identifier: MIT

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::Sha;

/// Guard that removes the temp directory on drop.
struct TempDirGuard(PathBuf);

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Creates a unique temp dir, merges profraw files into `merged.profdata`, and returns the path
/// plus a guard for cleanup.
fn merge_profraw_to_temp(profraw_files: &[PathBuf]) -> Result<(PathBuf, TempDirGuard), String> {
    let temp_dir = std::env::temp_dir().join(format!(
        "fuzzwizard_coverage_{}_{:016x}",
        std::process::id(),
        rand::random::<u64>()
    ));
    std::fs::create_dir_all(&temp_dir).map_err(|e| format!("Failed to create temp dir: {e}"))?;

    let merged_profdata = temp_dir.join("merged.profdata");
    let profraw_paths: Vec<String> = profraw_files
        .iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect();

    let output = Command::new("/usr/bin/llvm-profdata")
        .args(["merge", "-sparse"])
        .args(&profraw_paths)
        .arg("-o")
        .arg(&merged_profdata)
        .output()
        .map_err(|e| {
            format!(
                "llvm-profdata not available or error: {e}\n\nNote: Install LLVM tools (llvm-profdata, llvm-cov) to analyze coverage."
            )
        })?;

    if !output.status.success() {
        return Err(format!(
            "Failed to merge profraw files:\n{}\n\nNote: Install LLVM tools (llvm-profdata) to analyze coverage.",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    Ok((merged_profdata, TempDirGuard(temp_dir)))
}

/// Resolve coverage dir: collect profraw paths and binary for the given sha.
fn collect_profraw_and_binary(
    coverage_dir: &Path,
    binary_path: &Path,
    sha: &Sha,
) -> Result<(Vec<PathBuf>, PathBuf), String> {
    if !coverage_dir.exists() {
        return Err(format!(
            "Coverage directory not found: {}",
            coverage_dir.display()
        ));
    }

    let mut profraw_files = Vec::new();
    if let Ok(entries) = std::fs::read_dir(coverage_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("profraw") {
                profraw_files.push(path);
            }
        }
    }

    if profraw_files.is_empty() {
        return Err(format!(
            "No profraw files found in: {}",
            coverage_dir.display()
        ));
    }

    let binary_name = format!("fuzzer_{}.bin", hex::encode(sha));
    let coverage_binary = coverage_dir.join(&binary_name);
    let binary_to_use = if coverage_binary.exists() {
        coverage_binary
    } else if binary_path.exists() {
        binary_path.to_path_buf()
    } else {
        return Err(format!(
            "Binary not found for coverage analysis. Expected: {}",
            coverage_binary.display()
        ));
    };

    Ok((profraw_files, binary_to_use))
}

/// Run llvm-cov report and return the report text.
fn run_coverage_report(profraw_files: &[PathBuf], binary_path: &Path) -> String {
    let (_guard, merged_profdata) = match merge_profraw_to_temp(profraw_files) {
        Ok((path, guard)) => (guard, path),
        Err(e) => return e,
    };

    let output = match Command::new("/usr/bin/llvm-cov")
        .args(["report"])
        .arg(binary_path)
        .arg("-instr-profile")
        .arg(&merged_profdata)
        .output()
    {
        Ok(o) => o,
        Err(e) => {
            return format!(
                "llvm-cov not available or error: {e}\n\nNote: Install LLVM tools to see coverage."
            );
        }
    };

    if output.status.success() {
        String::from_utf8_lossy(&output.stdout).to_string()
    } else {
        format!(
            "Failed to generate coverage report:\n{}",
            String::from_utf8_lossy(&output.stderr)
        )
    }
}

/// Analyze coverage for a fuzzer. Returns the full summary report (header + llvm-cov report).
/// The report text includes the list of files; the UI can parse it to show the file list.
pub fn analyze_coverage(coverage_dir: &Path, binary_path: &Path, sha: &Sha) -> String {
    let (profraw_files, binary_to_use) =
        match collect_profraw_and_binary(coverage_dir, binary_path, sha) {
            Ok(x) => x,
            Err(e) => return e,
        };

    let report = run_coverage_report(&profraw_files, &binary_to_use);
    let mut result = format!("Coverage Analysis for: {}\n", binary_path.display());
    result.push_str(&format!("Coverage Directory: {}\n", coverage_dir.display()));
    result.push_str(&format!("Profraw Files Found: {}\n", profraw_files.len()));
    result.push_str(&format!("Binary: {}\n\n", binary_to_use.display()));
    result.push_str(&report);
    log::info!("Coverage summary: {result}");
    result
}

/// Coverage for a single file with line-by-line information.
/// Use this when you already have profraw list and binary path.
fn get_file_coverage_inner(
    profraw_files: &[PathBuf],
    binary_path: &Path,
    file_path: &Path,
) -> String {
    let (_guard, merged_profdata) = match merge_profraw_to_temp(profraw_files) {
        Ok((path, guard)) => (guard, path),
        Err(e) => return e,
    };

    // Run llvm-cov show without file filter (no -include-filename-regex: not supported on all LLVM versions),
    // then extract only the section for the requested file from the text output.
    let output = match Command::new("/usr/bin/llvm-cov")
        .args(["show", "-instr-profile"])
        .arg(&merged_profdata)
        .arg(binary_path)
        .args(["-format=text", "-show-line-counts-or-regions"])
        .output()
    {
        Ok(o) => o,
        Err(e) => {
            return format!(
                "llvm-cov not available or error: {e}\n\nNote: Install LLVM tools to see detailed coverage information."
            );
        }
    };

    if !output.status.success() {
        return format!(
            "Failed to generate file coverage:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let full_output = String::from_utf8_lossy(&output.stdout).to_string();
    extract_file_section(&full_output, file_path).unwrap_or(full_output)
}

/// Extract from `llvm-cov show` text output the section for the given file.
///
/// In text mode each file section starts with a header line `"path:"` at column 0
/// (no leading whitespace, no `|`).  All coverage data lines contain `|`
/// (format: `COUNT|LINENUM|code`), so that is the discriminant.
fn extract_file_section(full_output: &str, file_path: &Path) -> Option<String> {
    let wanted = file_path.to_string_lossy();
    let wanted_name = file_path.file_name().and_then(|s| s.to_str()).unwrap_or("");

    let mut current_path: Option<&str> = None;
    let mut current_lines: Vec<&str> = Vec::new();

    for line in full_output.lines() {
        let is_header = line.ends_with(':') && !line.starts_with(' ') && !line.contains('|');

        if is_header {
            if let Some(prev) = current_path {
                if paths_match(prev, &wanted, wanted_name) {
                    return Some(current_lines.join("\n"));
                }
            }
            current_path = Some(line.trim_end_matches(':'));
            current_lines.clear();
            current_lines.push(line);
        } else if current_path.is_some() {
            current_lines.push(line);
        }
    }

    if let Some(prev) = current_path {
        if paths_match(prev, &wanted, wanted_name) {
            return Some(current_lines.join("\n"));
        }
    }
    None
}

/// Flexible path comparison: exact match, suffix/prefix (absolute vs relative),
/// or same file name.
fn paths_match(section_path: &str, wanted: &str, wanted_name: &str) -> bool {
    let section = section_path.trim();
    section == wanted
        || section.ends_with(wanted)
        || wanted.ends_with(section)
        || (!wanted_name.is_empty()
            && (section.ends_with(&format!("/{wanted_name}"))
                || section.ends_with(&format!("\\{wanted_name}"))
                || section == wanted_name))
}

/// Coverage for a single file (line-by-line). Resolves coverage dir and binary from sha.
/// Call this from the scheduler when you have coverage_dir and task path.
pub fn get_file_coverage(
    coverage_dir: &Path,
    binary_path: &Path,
    sha: &Sha,
    file_path: &Path,
) -> String {
    let (profraw_files, binary_to_use) =
        match collect_profraw_and_binary(coverage_dir, binary_path, sha) {
            Ok(x) => x,
            Err(e) => return e,
        };
    get_file_coverage_inner(&profraw_files, &binary_to_use, file_path)
}
