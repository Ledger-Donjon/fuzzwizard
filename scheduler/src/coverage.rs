// SPDX-FileCopyrightText: © 2026 LEDGER SAS
// SPDX-License-Identifier: MIT

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use log::{debug, info, warn};

use crate::Sha;

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

/// Convert `.profraw` → indexed `.profdata` via `llvm-profdata merge`.
fn merge_profdata(
    profraw: &Path,
    coverage_dir: &Path,
    llvm_bin_dir: Option<&Path>,
) -> Result<PathBuf, String> {
    let profdata = coverage_dir.join("coverage.profdata");
    let exe = llvm_tool("llvm-profdata", llvm_bin_dir);
    let args = vec![
        "merge".into(),
        "-sparse".into(),
        profraw.to_string_lossy().into_owned(),
        "-o".into(),
        profdata.to_string_lossy().into_owned(),
    ];
    let output = Command::new(&exe)
        .args(&args)
        .output()
        .map_err(|e| format!("llvm-profdata not available: {e}"))?;

    if !output.status.success() {
        return Err(format!(
            "llvm-profdata merge failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(profdata)
}

/// Resolve `.profdata` and the instrumented binary for a fuzzer.
fn resolve_profdata_and_binary(
    coverage_dir: &Path,
    binary_path: &Path,
    sha: &Sha,
    profraw_path: Option<&Path>,
    llvm_bin_dir: Option<&Path>,
) -> Result<(PathBuf, PathBuf), String> {
    let profraw = profraw_path.ok_or_else(|| "No profraw path set for this task".to_string())?;
    if !profraw.exists() {
        return Err(format!("Profraw file not found: {}", profraw.display()));
    }
    let profdata = merge_profdata(profraw, coverage_dir, llvm_bin_dir)?;

    let coverage_binary = coverage_dir.join(format!("fuzzer_{}.bin", hex::encode(sha)));
    let binary = if coverage_binary.exists() {
        coverage_binary
    } else if binary_path.exists() {
        binary_path.to_path_buf()
    } else {
        return Err(format!("Binary not found: {}", coverage_binary.display()));
    };

    Ok((profdata, binary))
}

/// Extract `"filename":"…"` values from `llvm-cov export --summary-only` JSON output.
fn parse_filenames_from_export(json: &str) -> Vec<String> {
    let mut result = HashSet::new();
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(json) {
        collect_filenames(&v, &mut result);
    }
    result.into_iter().collect()
}

fn collect_filenames(v: &serde_json::Value, out: &mut HashSet<String>) {
    match v {
        serde_json::Value::Object(map) => {
            if let Some(serde_json::Value::String(s)) = map.get("filename") {
                if !s.is_empty() {
                    out.insert(s.clone());
                }
            }
            for val in map.values() {
                collect_filenames(val, out);
            }
        }
        serde_json::Value::Array(arr) => {
            for val in arr {
                collect_filenames(val, out);
            }
        }
        _ => {}
    }
}

/// Extract source file paths embedded in the binary via `llvm-cov export --summary-only`.
fn embedded_source_paths(
    profdata: &Path,
    binary: &Path,
    llvm_bin_dir: Option<&Path>,
) -> Vec<String> {
    let exe = llvm_tool("llvm-cov", llvm_bin_dir);
    let args = vec![
        "export".into(),
        "--summary-only".into(),
        "-instr-profile".into(),
        profdata.to_string_lossy().into_owned(),
        binary.to_string_lossy().into_owned(),
    ];
    let output = match Command::new(&exe).args(&args).output() {
        Ok(o) if o.status.success() => o,
        Ok(o) => {
            debug!(
                "llvm-cov export failed: {}",
                String::from_utf8_lossy(&o.stderr)
            );
            return Vec::new();
        }
        Err(e) => {
            debug!("llvm-cov export not available: {e}");
            return Vec::new();
        }
    };

    let raw = String::from_utf8_lossy(&output.stdout);
    let result = parse_filenames_from_export(&raw);
    debug!(
        "llvm-cov export found {} filenames: {:?}",
        result.len(),
        result
    );
    result
}

/// Detect container/host path mismatch and return a `-path-equivalence=FROM,TO` flag if needed.
///
/// Walks up from `host_binary_path` looking for an embedded source file on disk to derive
/// the mapping between container and host paths.
fn detect_path_equivalence(embedded_paths: &[String], host_binary_path: &Path) -> Option<String> {
    if embedded_paths.is_empty() {
        return None;
    }

    let start_dir = if host_binary_path.is_dir() {
        host_binary_path.to_path_buf()
    } else {
        host_binary_path.parent()?.to_path_buf()
    };

    for emb in embedded_paths {
        let emb_path = Path::new(emb);
        let emb_name = match emb_path.file_name().and_then(|s| s.to_str()) {
            Some(n) => n,
            None => continue,
        };
        let emb_dir = emb_path.parent().unwrap_or(Path::new("/"));

        let mut candidate = Some(start_dir.as_path());
        while let Some(dir) = candidate {
            if dir == emb_dir {
                return None;
            }
            if dir.join(emb_name).is_file() {
                let host_str = dir.to_string_lossy();
                let emb_str = emb_dir.to_string_lossy();
                let result = format!("-path-equivalence={emb_str},{host_str}");
                info!("Path equivalence detected: {result}");
                return Some(result);
            }
            candidate = dir.parent();
        }
    }

    None
}

/// Run `llvm-cov report` and return the text output.
fn run_report(
    profdata: &Path,
    binary: &Path,
    llvm_bin_dir: Option<&Path>,
    path_equiv: Option<&str>,
) -> String {
    let exe = llvm_tool("llvm-cov", llvm_bin_dir);
    let mut args: Vec<String> = vec![
        "report".into(),
        binary.to_string_lossy().into_owned(),
        "-instr-profile".into(),
        profdata.to_string_lossy().into_owned(),
        "-use-color=false".into(),
    ];
    if let Some(pe) = path_equiv {
        args.push(pe.to_string());
    }
    match Command::new(&exe).args(&args).output() {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).to_string(),
        Ok(o) => format!(
            "llvm-cov report failed:\n{}",
            String::from_utf8_lossy(&o.stderr)
        ),
        Err(e) => format!("llvm-cov not available: {e}"),
    }
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
fn run_show(
    profdata: &Path,
    binary: &Path,
    source_file: Option<&Path>,
    llvm_bin_dir: Option<&Path>,
    path_equiv: Option<&str>,
) -> Result<String, String> {
    let exe = llvm_tool("llvm-cov", llvm_bin_dir);

    let mut args: Vec<String> = vec![
        "show".into(),
        "-instr-profile".into(),
        profdata.to_string_lossy().into_owned(),
        binary.to_string_lossy().into_owned(),
        "-format=text".into(),
        "-show-line-counts-or-regions".into(),
        "-use-color=false".into(),
    ];
    if let Some(pe) = path_equiv {
        args.push(pe.to_string());
    }

    debug!("llvm-cov show command: {} {}", exe, args.join(" "));

    let output = Command::new(&exe)
        .args(&args)
        .output()
        .map_err(|e| format!("llvm-cov not available: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("llvm-cov show failed:\n{stderr}"));
    }

    let full_output = String::from_utf8_lossy(&output.stdout).to_string();
    if full_output.trim().is_empty() {
        return Ok(String::new());
    }

    let source_file = match source_file {
        Some(sf) => sf,
        None => return Ok(full_output),
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

    extract_file_section(&full_output, &source_str, file_name)
        .map(Ok)
        .unwrap_or(Ok(String::new()))
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
        // No file headers found — output is probably for a single file already.
        return Some(full_output.to_string());
    }

    debug!("extract_file_section: {} sections found", sections.len());

    // Try exact path match first, then basename match.
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

    // Find the end of this section (next file header or end of output).
    let end_idx = sections
        .iter()
        .find(|(i, _)| *i > start_idx)
        .map(|(i, _)| *i)
        .unwrap_or(lines.len());

    let section: String = lines[start_idx..end_idx].to_vec().join("\n");

    Some(section)
}

/// Coverage summary report for a fuzzer (used by the TUI file list view).
pub fn analyze_coverage(
    coverage_dir: &Path,
    binary_path: &Path,
    sha: &Sha,
    profraw_path: Option<&Path>,
    llvm_bin_dir: Option<&Path>,
) -> String {
    let (profdata, binary) = match resolve_profdata_and_binary(
        coverage_dir,
        binary_path,
        sha,
        profraw_path,
        llvm_bin_dir,
    ) {
        Ok(x) => x,
        Err(e) => return e,
    };

    let embedded = embedded_source_paths(&profdata, &binary, llvm_bin_dir);
    let path_equiv = detect_path_equivalence(&embedded, binary_path);
    let report = run_report(&profdata, &binary, llvm_bin_dir, path_equiv.as_deref());

    let mut result = format!("Coverage Analysis for: {}\n", binary_path.display());
    result.push_str(&format!("Coverage Directory: {}\n", coverage_dir.display()));
    result.push_str(&format!("Profile: {}\n", profdata.display()));
    result.push_str(&format!("Binary: {}\n\n", binary.display()));
    result.push_str(&report);
    result
}

/// Line-by-line coverage for source file(s).
///
/// When `file_path` is `Some`, returns annotated source for that single file.
/// When `file_path` is `None`, returns annotated source for **all** files (TOTAL view).
pub fn get_file_coverage(
    coverage_dir: &Path,
    binary_path: &Path,
    sha: &Sha,
    profraw_path: Option<&Path>,
    file_path: Option<&Path>,
    llvm_bin_dir: Option<&Path>,
) -> String {
    let (profdata, binary) = match resolve_profdata_and_binary(
        coverage_dir,
        binary_path,
        sha,
        profraw_path,
        llvm_bin_dir,
    ) {
        Ok(x) => x,
        Err(e) => return e,
    };

    let embedded = embedded_source_paths(&profdata, &binary, llvm_bin_dir);
    let path_equiv = detect_path_equivalence(&embedded, binary_path);

    match run_show(
        &profdata,
        &binary,
        file_path,
        llvm_bin_dir,
        path_equiv.as_deref(),
    ) {
        Ok(s) if !s.trim().is_empty() => s,
        Ok(_) => {
            warn!(
                "llvm-cov show produced no output for {file_path:?} (binary={binary:?}, path_equiv={path_equiv:?})"
            );
            "llvm-cov show produced no output for this file.".to_string()
        }
        Err(e) => e,
    }
}
