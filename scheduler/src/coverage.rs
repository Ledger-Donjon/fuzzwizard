// SPDX-FileCopyrightText: © 2026 LEDGER SAS
// SPDX-License-Identifier: MIT

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use log::{debug, info, warn};

use crate::Sha;

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
struct Mount {
    host_path: PathBuf,
    read_only: bool,
}

/// Build the full argument list for a containerised coverage tool invocation.
///
/// The resulting vector is suitable for `Command::new(binary).args(...)`.
fn build_container_args(
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
fn tool_not_found_error(tool: &str, image: &str) -> String {
    format!(
        "{tool} not found in container image '{image}'.\n\
         The image used to run fuzzers must also provide llvm-profdata and llvm-cov \
         (see fuzzwizard_scheduler_test/docker/ubuntu-llvm.Dockerfile), or set \
         llvm_bin_directory to their path inside the image."
    )
}

/// Returns `true` when the command output indicates the executable was not
/// found (exit code 127, or well-known stderr patterns).
fn is_tool_not_found(status: &std::process::ExitStatus, stderr: &str) -> bool {
    if status.code() == Some(127) {
        return true;
    }
    let lower = stderr.to_lowercase();
    lower.contains("executable file not found") || lower.contains("no such file or directory")
}

impl CoverageRunner {
    /// Build a [`Command`] that runs `tool` with the given arguments.
    ///
    /// In `Host` mode the tool is resolved via `llvm_tool()`.
    /// In `Container` mode an ephemeral `run --rm` container is launched with
    /// identity mounts so that all path arguments remain valid inside the
    /// container.
    fn command(&self, tool: &str, tool_args: &[String], mounts: &[Mount]) -> Command {
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

    fn image_name(&self) -> Option<&str> {
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
fn llvm_tool_container(name: &str, llvm_bin_dir: Option<&Path>) -> String {
    if let Some(dir) = llvm_bin_dir {
        dir.join(name).to_string_lossy().into_owned()
    } else {
        name.to_string()
    }
}

/// Convert `.profraw` → indexed `.profdata` via `llvm-profdata merge`.
fn merge_profdata(
    profraw: &Path,
    coverage_dir: &Path,
    runner: &CoverageRunner,
) -> Result<PathBuf, String> {
    let profdata = coverage_dir.join("coverage.profdata");
    let tool_args = vec![
        "merge".into(),
        "-sparse".into(),
        profraw.to_string_lossy().into_owned(),
        "-o".into(),
        profdata.to_string_lossy().into_owned(),
    ];
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
        if let Some(img) = runner.image_name() {
            if is_tool_not_found(&output.status, &stderr) {
                return Err(tool_not_found_error("llvm-profdata", img));
            }
        }
        return Err(format!("llvm-profdata merge failed:\n{stderr}"));
    }
    Ok(profdata)
}

/// Resolve `.profdata` and the instrumented binary for a fuzzer.
fn resolve_profdata_and_binary(
    coverage_dir: &Path,
    binary_path: &Path,
    sha: &Sha,
    profraw_path: Option<&Path>,
    runner: &CoverageRunner,
) -> Result<(PathBuf, PathBuf), String> {
    let profraw = profraw_path.ok_or_else(|| "No profraw path set for this task".to_string())?;
    if !profraw.exists() {
        return Err(format!("Profraw file not found: {}", profraw.display()));
    }
    let profdata = merge_profdata(profraw, coverage_dir, runner)?;

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

/// Extract `"filename":"…"` values from `llvm-cov export` JSON output.
fn parse_filenames_from_export(json: &serde_json::Value) -> Vec<String> {
    let mut result = HashSet::new();
    collect_filenames(json, &mut result);
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

/// Run `llvm-cov export` and parse its JSON output.
///
/// `summary_only` keeps the payload small when only file names and totals are
/// needed; the full export additionally carries per-function regions, which is
/// what lets us report coverage when the sources themselves are unavailable.
fn export_json(
    profdata: &Path,
    binary: &Path,
    runner: &CoverageRunner,
    coverage_dir: &Path,
    summary_only: bool,
) -> Option<serde_json::Value> {
    let mut tool_args: Vec<String> = vec!["export".into()];
    if summary_only {
        tool_args.push("--summary-only".into());
    }
    tool_args.extend([
        "-instr-profile".into(),
        profdata.to_string_lossy().into_owned(),
        binary.to_string_lossy().into_owned(),
    ]);
    let mounts = coverage_mounts(coverage_dir, binary);
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

/// Extract source file paths embedded in the binary via `llvm-cov export --summary-only`.
fn embedded_source_paths(
    profdata: &Path,
    binary: &Path,
    runner: &CoverageRunner,
    coverage_dir: &Path,
) -> Vec<String> {
    let Some(json) = export_json(profdata, binary, runner, coverage_dir, true) else {
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

/// Where the sources referenced by the coverage mapping live on the host.
#[derive(Debug)]
struct SourceLocation {
    /// `-path-equivalence=FROM,TO` flag, needed only when the on-disk location
    /// differs from the path recorded in the binary.
    flag: Option<String>,
    /// Host directories to bind-mount read-only so containerised `llvm-cov`
    /// can read the sources.
    mounts: Vec<PathBuf>,
}

/// Upper bound on source directories bind-mounted into a coverage container.
const MAX_SOURCE_MOUNTS: usize = 16;

/// Drop directories already covered by another entry so nested source trees
/// collapse into a single mount.
fn dedup_ancestors(mut dirs: Vec<PathBuf>) -> Vec<PathBuf> {
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
/// up from `host_binary_path` to derive a recorded→host mapping.
fn locate_sources(embedded_paths: &[String], host_binary_path: &Path) -> Option<SourceLocation> {
    if embedded_paths.is_empty() {
        return None;
    }

    let present: Vec<PathBuf> = embedded_paths
        .iter()
        .map(Path::new)
        .filter(|p| p.is_file())
        .filter_map(|p| p.parent().map(Path::to_path_buf))
        .collect();
    if !present.is_empty() {
        let mut mounts = dedup_ancestors(present);
        mounts.truncate(MAX_SOURCE_MOUNTS);
        info!("Sources found at their recorded paths, mounting {mounts:?}");
        return Some(SourceLocation { flag: None, mounts });
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

    warn!(
        "No source tree found on host for {} embedded paths",
        embedded_paths.len()
    );
    None
}

/// Collect the base mounts needed for coverage commands: `coverage_dir` (rw)
/// and the binary's parent directory (ro, only if it lives outside `coverage_dir`).
fn coverage_mounts(coverage_dir: &Path, binary: &Path) -> Vec<Mount> {
    let mut mounts = vec![Mount {
        host_path: coverage_dir.to_path_buf(),
        read_only: false,
    }];
    if let Some(bin_dir) = binary.parent() {
        if !bin_dir.starts_with(coverage_dir) {
            mounts.push(Mount {
                host_path: bin_dir.to_path_buf(),
                read_only: true,
            });
        }
    }
    mounts
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
fn run_report(
    profdata: &Path,
    binary: &Path,
    runner: &CoverageRunner,
    source_loc: Option<&SourceLocation>,
    coverage_dir: &Path,
) -> String {
    let mut tool_args: Vec<String> = vec![
        "report".into(),
        binary.to_string_lossy().into_owned(),
        "-instr-profile".into(),
        profdata.to_string_lossy().into_owned(),
        "-use-color=false".into(),
    ];
    push_source_flag(&mut tool_args, source_loc);
    let mut mounts = coverage_mounts(coverage_dir, binary);
    push_source_mounts(&mut mounts, source_loc);
    match runner.command("llvm-cov", &tool_args, &mounts).output() {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).to_string(),
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr);
            if let Some(img) = runner.image_name() {
                if is_tool_not_found(&o.status, &stderr) {
                    return tool_not_found_error("llvm-cov", img);
                }
            }
            format!("llvm-cov report failed:\n{stderr}")
        }
        Err(e) => format!("llvm-cov not available: {e}"),
    }
}

/// Outcome of a successful `llvm-cov show` invocation.
enum ShowOutcome {
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
fn parse_unreadable_sources(stderr: &str) -> Vec<String> {
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
fn run_show(
    profdata: &Path,
    binary: &Path,
    source_file: Option<&Path>,
    runner: &CoverageRunner,
    source_loc: Option<&SourceLocation>,
    coverage_dir: &Path,
) -> Result<ShowOutcome, String> {
    let mut tool_args: Vec<String> = vec![
        "show".into(),
        "-instr-profile".into(),
        profdata.to_string_lossy().into_owned(),
        binary.to_string_lossy().into_owned(),
        "-format=text".into(),
        "-show-line-counts-or-regions".into(),
        "-use-color=false".into(),
    ];
    push_source_flag(&mut tool_args, source_loc);

    let mut mounts = coverage_mounts(coverage_dir, binary);
    push_source_mounts(&mut mounts, source_loc);

    debug!("llvm-cov show tool_args: {:?}", tool_args);

    let output = runner
        .command("llvm-cov", &tool_args, &mounts)
        .output()
        .map_err(|e| format!("llvm-cov not available: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if let Some(img) = runner.image_name() {
            if is_tool_not_found(&output.status, &stderr) {
                return Err(tool_not_found_error("llvm-cov", img));
            }
        }
        return Err(format!("llvm-cov show failed:\n{stderr}"));
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

/// First line of the coverage view rendered when `llvm-cov` cannot read the
/// sources and only region-level data can be shown.
///
/// The TUI keys off this marker to style the panel as a degraded view.
pub const COVERAGE_SOURCE_UNAVAILABLE_MARKER: &str = "SOURCE NOT AVAILABLE";

/// `kind` value identifying an ordinary code region in `llvm-cov export` output.
const REGION_KIND_CODE: u64 = 0;

/// A code region from `llvm-cov export`, whose JSON encoding is the array
/// `[lineStart, colStart, lineEnd, colEnd, count, fileID, expandedFileID, kind]`.
struct Region {
    line_start: u64,
    line_end: u64,
    count: u64,
}

/// Per-function coverage extracted from `llvm-cov export`.
struct FunctionCoverage {
    name: String,
    count: u64,
    regions: Vec<Region>,
}

impl FunctionCoverage {
    fn line_span(&self) -> Option<(u64, u64)> {
        let start = self.regions.iter().map(|r| r.line_start).min()?;
        let end = self.regions.iter().map(|r| r.line_end).max()?;
        Some((start, end))
    }

    fn covered_regions(&self) -> usize {
        self.regions.iter().filter(|r| r.count > 0).count()
    }
}

/// Compare two source paths, falling back to a basename match so that
/// container/host path differences still line up.
fn same_source_file(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    fn base(p: &str) -> Option<&str> {
        Path::new(p).file_name().and_then(|n| n.to_str())
    }
    match (base(a), base(b)) {
        (Some(x), Some(y)) => x == y,
        _ => false,
    }
}

fn parse_region(v: &serde_json::Value) -> Option<Region> {
    let a = v.as_array()?;
    let num = |i: usize| a.get(i).and_then(serde_json::Value::as_u64);
    if num(7).unwrap_or(REGION_KIND_CODE) != REGION_KIND_CODE {
        return None;
    }
    Some(Region {
        line_start: num(0)?,
        line_end: num(2)?,
        count: num(4)?,
    })
}

/// Collect per-function coverage from `llvm-cov export` JSON, restricted to
/// `file` when given, ordered by position in the source.
fn parse_functions(json: &serde_json::Value, file: Option<&str>) -> Vec<FunctionCoverage> {
    let mut out: Vec<FunctionCoverage> = Vec::new();
    let Some(data) = json.get("data").and_then(|d| d.as_array()) else {
        return out;
    };

    for entry in data {
        let Some(functions) = entry.get("functions").and_then(|f| f.as_array()) else {
            continue;
        };
        for f in functions {
            if let Some(target) = file {
                let belongs = f
                    .get("filenames")
                    .and_then(|n| n.as_array())
                    .is_some_and(|names| {
                        names
                            .iter()
                            .filter_map(serde_json::Value::as_str)
                            .any(|n| same_source_file(n, target))
                    });
                if !belongs {
                    continue;
                }
            }
            let regions = f
                .get("regions")
                .and_then(|r| r.as_array())
                .map(|r| r.iter().filter_map(parse_region).collect::<Vec<_>>())
                .unwrap_or_default();
            if regions.is_empty() {
                continue;
            }
            out.push(FunctionCoverage {
                name: f
                    .get("name")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("<unknown>")
                    .to_string(),
                count: f
                    .get("count")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0),
                regions,
            });
        }
    }

    out.sort_by_key(|f| f.line_span().map(|(start, _)| start).unwrap_or(0));
    out
}

/// Merge overlapping or adjacent `(start, end)` line ranges.
fn merge_ranges(mut ranges: Vec<(u64, u64)>) -> Vec<(u64, u64)> {
    ranges.sort_unstable();
    let mut merged: Vec<(u64, u64)> = Vec::new();
    for (start, end) in ranges {
        match merged.last_mut() {
            Some(last) if start <= last.1.saturating_add(1) => last.1 = last.1.max(end),
            _ => merged.push((start, end)),
        }
    }
    merged
}

fn format_range((start, end): (u64, u64)) -> String {
    if start == end {
        start.to_string()
    } else {
        format!("{start}-{end}")
    }
}

/// Shorten an over-long symbol name so the function table stays aligned.
fn truncate_name(name: &str, max: usize) -> String {
    if name.chars().count() <= max {
        return name.to_string();
    }
    let head: String = name.chars().take(max.saturating_sub(3)).collect();
    format!("{head}...")
}

/// Render the `Lines`/`Functions`/`Regions` totals of a file, if the export
/// carries a summary block for it.
fn render_file_summary(json: &serde_json::Value, file: &str) -> Option<String> {
    let files = json
        .get("data")
        .and_then(|d| d.as_array())?
        .iter()
        .filter_map(|e| e.get("files").and_then(|f| f.as_array()))
        .flatten();
    let summary = files
        .filter(|f| {
            f.get("filename")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|n| same_source_file(n, file))
        })
        .find_map(|f| f.get("summary"))?;

    let mut out = String::new();
    for key in ["lines", "functions", "regions", "branches"] {
        let Some(stat) = summary.get(key) else {
            continue;
        };
        let covered = stat.get("covered").and_then(serde_json::Value::as_u64);
        let count = stat.get("count").and_then(serde_json::Value::as_u64);
        let percent = stat.get("percent").and_then(serde_json::Value::as_f64);
        if let (Some(covered), Some(count), Some(percent)) = (covered, count, percent) {
            let label = format!("{}{}", key[..1].to_uppercase(), &key[1..]);
            out.push_str(&format!(
                "  {label:<10} {covered:>8} / {count:<8} ({percent:.2}%)\n"
            ));
        }
    }
    (!out.is_empty()).then_some(out)
}

/// Render the per-function table and the uncovered line ranges.
fn render_functions(functions: &[FunctionCoverage]) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "\nFUNCTIONS ({} total, {} executed)\n\n",
        functions.len(),
        functions.iter().filter(|f| f.count > 0).count()
    ));
    out.push_str(&format!(
        "  {:>12}  {:>13}  {:>9}  {}\n",
        "EXECUTIONS", "LINES", "REGIONS", "NAME"
    ));
    for f in functions {
        let lines = f.line_span().map(format_range).unwrap_or_default();
        let regions = format!("{}/{}", f.covered_regions(), f.regions.len());
        out.push_str(&format!(
            "  {:>12}  {:>13}  {:>9}  {}\n",
            f.count,
            lines,
            regions,
            truncate_name(&f.name, 80)
        ));
    }

    let uncovered = merge_ranges(
        functions
            .iter()
            .flat_map(|f| f.regions.iter())
            .filter(|r| r.count == 0)
            .map(|r| (r.line_start, r.line_end))
            .collect(),
    );
    if !uncovered.is_empty() {
        let plural = if uncovered.len() == 1 {
            "range"
        } else {
            "ranges"
        };
        out.push_str(&format!(
            "\nUNCOVERED LINES ({} {plural})\n\n",
            uncovered.len()
        ));
        let rendered: Vec<String> = uncovered.into_iter().map(format_range).collect();
        for chunk in rendered.chunks(8) {
            out.push_str(&format!("  {}\n", chunk.join(", ")));
        }
    }

    out
}

/// Build the view shown when `llvm-cov` cannot read the sources.
///
/// `llvm-cov report` and `llvm-cov export` only need the coverage mapping, so
/// region-level data remains available even with no source tree on disk; we
/// show that rather than an empty panel.
fn render_missing_source_view(
    requested: Option<&Path>,
    export: Option<&serde_json::Value>,
) -> String {
    let mut out = format!("{COVERAGE_SOURCE_UNAVAILABLE_MARKER}\n\n");

    let requested_str = requested.map(|p| p.to_string_lossy().into_owned());
    match &requested_str {
        Some(path) => out.push_str(&format!(
            "llvm-cov could not read the source file recorded in the binary:\n  {path}\n"
        )),
        None => out
            .push_str("llvm-cov could not read any of the source files recorded in the binary.\n"),
    }
    out.push_str("\nRegion-level coverage from the profile data is shown below.\n");

    let Some(json) = export else {
        out.push_str("\nNo region data available: llvm-cov export produced no usable output.\n");
        return out;
    };

    match &requested_str {
        Some(path) => out.push_str(&render_file_section(json, path, false)),
        None => {
            let files = export_file_names(json);
            if files.is_empty() {
                out.push_str("\nNo region data available in the coverage mapping.\n");
            }
            for file in files {
                out.push_str(&render_file_section(json, &file, true));
            }
        }
    }

    out
}

/// File names carried by an `llvm-cov export` payload.
fn export_file_names(json: &serde_json::Value) -> Vec<String> {
    let Some(data) = json.get("data").and_then(|d| d.as_array()) else {
        return Vec::new();
    };
    let mut names: Vec<String> = data
        .iter()
        .filter_map(|e| e.get("files").and_then(|f| f.as_array()))
        .flatten()
        .filter_map(|f| f.get("filename").and_then(serde_json::Value::as_str))
        .map(str::to_string)
        .collect();
    names.sort();
    names.dedup();
    names
}

/// Render the summary and function table of a single file.
///
/// `with_header` prefixes the file name, which the all-files view needs to keep
/// the per-file sections apart.
fn render_file_section(json: &serde_json::Value, file: &str, with_header: bool) -> String {
    let mut out = String::new();
    if with_header {
        out.push_str(&format!("\n{}\n{file}\n", "=".repeat(72)));
    }
    if let Some(summary) = render_file_summary(json, file) {
        out.push_str("\nSUMMARY\n\n");
        out.push_str(&summary);
    }
    let functions = parse_functions(json, Some(file));
    if functions.is_empty() {
        out.push_str("\nNo function-level region data found for this file.\n");
    } else {
        out.push_str(&render_functions(&functions));
    }
    out
}

/// Coverage summary report for a fuzzer (used by the TUI file list view).
pub fn analyze_coverage(
    coverage_dir: &Path,
    binary_path: &Path,
    sha: &Sha,
    profraw_path: Option<&Path>,
    runner: &CoverageRunner,
) -> String {
    let (profdata, binary) =
        match resolve_profdata_and_binary(coverage_dir, binary_path, sha, profraw_path, runner) {
            Ok(x) => x,
            Err(e) => return e,
        };

    let embedded = embedded_source_paths(&profdata, &binary, runner, coverage_dir);
    let source_loc = locate_sources(&embedded, binary_path);
    let report = run_report(
        &profdata,
        &binary,
        runner,
        source_loc.as_ref(),
        coverage_dir,
    );

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
    runner: &CoverageRunner,
) -> String {
    let (profdata, binary) =
        match resolve_profdata_and_binary(coverage_dir, binary_path, sha, profraw_path, runner) {
            Ok(x) => x,
            Err(e) => return e,
        };

    let embedded = embedded_source_paths(&profdata, &binary, runner, coverage_dir);
    let source_loc = locate_sources(&embedded, binary_path);

    let unreadable = match run_show(
        &profdata,
        &binary,
        file_path,
        runner,
        source_loc.as_ref(),
        coverage_dir,
    ) {
        Ok(ShowOutcome::Annotated(s)) if !s.trim().is_empty() => return s,
        Ok(ShowOutcome::Annotated(_)) => Vec::new(),
        Ok(ShowOutcome::NoSource(paths)) => paths,
        Err(e) => return e,
    };

    warn!(
        "No annotated source for {file_path:?} (binary={binary:?}, sources={source_loc:?}); \
         llvm-cov could not read {unreadable:?}; falling back to region-level coverage"
    );
    let export = export_json(&profdata, &binary, runner, coverage_dir, false);
    render_missing_source_view(file_path, export.as_ref())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt;
    use std::process::ExitStatus;

    #[test]
    fn build_container_args_basic() {
        let mounts = vec![
            Mount {
                host_path: PathBuf::from("/data/coverage"),
                read_only: false,
            },
            Mount {
                host_path: PathBuf::from("/data/bin"),
                read_only: true,
            },
        ];
        let tool_args: Vec<String> = vec![
            "merge".into(),
            "-sparse".into(),
            "/data/coverage/abc123.profraw".into(),
        ];
        let args = build_container_args(
            "fuzzing-image:latest",
            &[],
            "llvm-profdata",
            &tool_args,
            &mounts,
        );

        assert!(args.contains(&"run".to_string()));
        assert!(args.contains(&"--rm".to_string()));
        assert!(args.contains(&"--network=none".to_string()));

        let image_pos = args
            .iter()
            .position(|a| a == "fuzzing-image:latest")
            .unwrap();
        let tool_pos = args.iter().position(|a| a == "llvm-profdata").unwrap();
        assert!(image_pos < tool_pos, "image must appear before tool name");

        assert!(args.contains(&"-v".to_string()));
        assert!(args.contains(&"/data/coverage:/data/coverage".to_string()));
        assert!(args.contains(&"/data/bin:/data/bin:ro".to_string()));
    }

    #[test]
    fn build_container_args_with_options() {
        let args = build_container_args(
            "my-image:v1",
            &["--platform=linux/amd64".to_string()],
            "llvm-cov",
            &["report".into()],
            &[],
        );

        let opt_pos = args
            .iter()
            .position(|a| a == "--platform=linux/amd64")
            .unwrap();
        let image_pos = args.iter().position(|a| a == "my-image:v1").unwrap();
        assert!(
            opt_pos < image_pos,
            "container_options must appear before image"
        );
    }

    #[test]
    fn build_container_args_no_mount_when_empty() {
        let args = build_container_args("img:latest", &[], "llvm-cov", &["show".into()], &[]);
        assert!(!args.contains(&"-v".to_string()));
    }

    #[test]
    fn is_tool_not_found_exit_127() {
        let status = ExitStatus::from_raw(127 << 8);
        assert!(is_tool_not_found(&status, ""));
    }

    #[test]
    fn is_tool_not_found_stderr_pattern() {
        let status = ExitStatus::from_raw(1 << 8);
        assert!(is_tool_not_found(
            &status,
            "OCI runtime exec failed: executable file not found in $PATH"
        ));
        assert!(is_tool_not_found(
            &status,
            "llvm-profdata: No such file or directory"
        ));
    }

    #[test]
    fn is_tool_not_found_normal_failure() {
        let status = ExitStatus::from_raw(1 << 8);
        assert!(!is_tool_not_found(&status, "merge failed: corrupted data"));
    }

    #[test]
    fn tool_not_found_error_contains_image() {
        let msg = tool_not_found_error("llvm-profdata", "my-image:v2");
        assert!(msg.contains("my-image:v2"));
        assert!(msg.contains("llvm-profdata"));
        assert!(msg.contains("ubuntu-llvm.Dockerfile"));
    }

    #[test]
    fn llvm_tool_container_with_dir() {
        let result = llvm_tool_container("llvm-cov", Some(Path::new("/usr/lib/llvm-18/bin")));
        assert_eq!(result, "/usr/lib/llvm-18/bin/llvm-cov");
    }

    #[test]
    fn llvm_tool_container_without_dir() {
        let result = llvm_tool_container("llvm-cov", None);
        assert_eq!(result, "llvm-cov");
    }

    #[test]
    fn coverage_mounts_binary_inside_coverage_dir() {
        let coverage_dir = Path::new("/data/coverage/fuzzer1");
        let binary = Path::new("/data/coverage/fuzzer1/fuzzer_abc.bin");
        let mounts = coverage_mounts(coverage_dir, binary);
        assert_eq!(
            mounts.len(),
            1,
            "no extra mount when binary is inside coverage_dir"
        );
        assert_eq!(mounts[0].host_path, PathBuf::from("/data/coverage/fuzzer1"));
        assert!(!mounts[0].read_only);
    }

    #[test]
    fn coverage_mounts_binary_outside_coverage_dir() {
        let coverage_dir = Path::new("/data/coverage/fuzzer1");
        let binary = Path::new("/opt/targets/fuzzer1");
        let mounts = coverage_mounts(coverage_dir, binary);
        assert_eq!(mounts.len(), 2);
        assert_eq!(mounts[1].host_path, PathBuf::from("/opt/targets"));
        assert!(mounts[1].read_only);
    }

    #[test]
    fn build_container_args_dedups_repeated_mount_points() {
        let mounts = vec![
            Mount {
                host_path: PathBuf::from("/src"),
                read_only: false,
            },
            Mount {
                host_path: PathBuf::from("/src"),
                read_only: true,
            },
        ];
        let args = build_container_args("img", &[], "llvm-cov", &["show".into()], &mounts);
        assert_eq!(args.iter().filter(|a| *a == "-v").count(), 1);
        assert!(args.contains(&"/src:/src".to_string()));
        assert!(!args.contains(&"/src:/src:ro".to_string()));
    }

    #[test]
    fn parse_unreadable_sources_from_stderr() {
        let stderr = "error: /build/src/parser.c: No such file or directory\n\
                      warning: The file '/build/src/parser.c' isn't covered.\n\
                      warning: The file '/build/src/lexer.c' isn't covered.\n\
                      error: unrelated failure\n";
        assert_eq!(
            parse_unreadable_sources(stderr),
            vec![
                "/build/src/parser.c".to_string(),
                "/build/src/lexer.c".to_string()
            ]
        );
    }

    #[test]
    fn parse_unreadable_sources_ignores_unrelated_errors() {
        let stderr = "error: /tmp/x.profdata: profile data is corrupted\n";
        assert!(parse_unreadable_sources(stderr).is_empty());
    }

    #[test]
    fn dedup_ancestors_collapses_nested_dirs() {
        let dirs = vec![
            PathBuf::from("/proj/src/net"),
            PathBuf::from("/proj/src"),
            PathBuf::from("/usr/include"),
            PathBuf::from("/proj/src"),
        ];
        assert_eq!(
            dedup_ancestors(dirs),
            vec![PathBuf::from("/proj/src"), PathBuf::from("/usr/include")]
        );
    }

    #[test]
    fn locate_sources_mounts_without_flag_when_paths_match() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let src_dir = tmp.path().join("src");
        std::fs::create_dir_all(&src_dir).unwrap();
        let source = src_dir.join("parser.c");
        std::fs::write(&source, "int main(void) { return 0; }\n").unwrap();

        let binary = tmp.path().join("build").join("fuzzer");
        let embedded = vec![source.to_string_lossy().into_owned()];
        let loc = locate_sources(&embedded, &binary).expect("sources should be located");

        assert!(
            loc.flag.is_none(),
            "no path-equivalence needed when the recorded path exists"
        );
        assert_eq!(loc.mounts, vec![src_dir]);
    }

    #[test]
    fn locate_sources_derives_path_equivalence_when_tree_moved() {
        let tmp = assert_fs::TempDir::new().unwrap();
        std::fs::write(tmp.path().join("parser.c"), "int x;\n").unwrap();
        let binary = tmp.path().join("fuzzer");

        let embedded = vec!["/build/elsewhere/parser.c".to_string()];
        let loc = locate_sources(&embedded, &binary).expect("sources should be located");

        let flag = loc.flag.expect("moved tree needs a path-equivalence flag");
        assert!(flag.starts_with("-path-equivalence=/build/elsewhere,"));
        assert_eq!(loc.mounts, vec![tmp.path().to_path_buf()]);
    }

    #[test]
    fn locate_sources_none_when_nothing_on_disk() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let embedded = vec!["/build/gone/parser.c".to_string()];
        assert!(locate_sources(&embedded, &tmp.path().join("fuzzer")).is_none());
    }

    #[test]
    fn merge_ranges_joins_overlapping_and_adjacent() {
        let merged = merge_ranges(vec![(10, 12), (13, 15), (30, 40), (35, 50), (1, 2)]);
        assert_eq!(merged, vec![(1, 2), (10, 15), (30, 50)]);
    }

    #[test]
    fn same_source_file_matches_on_basename() {
        assert!(same_source_file("/build/src/a.c", "/host/src/a.c"));
        assert!(same_source_file("/build/src/a.c", "/build/src/a.c"));
        assert!(!same_source_file("/build/src/a.c", "/build/src/b.c"));
    }

    /// Minimal `llvm-cov export` payload: one file, two functions, one of
    /// which is never executed.
    fn sample_export() -> serde_json::Value {
        serde_json::json!({
            "data": [{
                "files": [{
                    "filename": "/build/src/parser.c",
                    "summary": {
                        "lines": { "count": 100, "covered": 60, "percent": 60.0 },
                        "functions": { "count": 2, "covered": 1, "percent": 50.0 }
                    }
                }],
                "functions": [
                    {
                        "name": "parse_header",
                        "count": 1204,
                        "filenames": ["/build/src/parser.c"],
                        "regions": [[12, 1, 48, 2, 1204, 0, 0, 0]]
                    },
                    {
                        "name": "parse_trailer",
                        "count": 0,
                        "filenames": ["/build/src/parser.c"],
                        "regions": [
                            [142, 1, 160, 2, 0, 0, 0, 0],
                            [161, 1, 165, 2, 0, 0, 0, 3]
                        ]
                    }
                ]
            }]
        })
    }

    #[test]
    fn parse_functions_filters_by_file_and_skips_non_code_regions() {
        let json = sample_export();
        let functions = parse_functions(&json, Some("/build/src/parser.c"));
        assert_eq!(functions.len(), 2);
        assert_eq!(functions[0].name, "parse_header");
        assert_eq!(functions[0].line_span(), Some((12, 48)));
        // The gap region (kind 3) of parse_trailer must not widen its span.
        assert_eq!(functions[1].line_span(), Some((142, 160)));
        assert_eq!(functions[1].covered_regions(), 0);

        assert!(parse_functions(&json, Some("/build/src/other.c")).is_empty());
    }

    #[test]
    fn missing_source_view_reports_regions_instead_of_nothing() {
        let json = sample_export();
        let view = render_missing_source_view(Some(Path::new("/build/src/parser.c")), Some(&json));

        assert!(view.starts_with(COVERAGE_SOURCE_UNAVAILABLE_MARKER));
        assert!(view.contains("/build/src/parser.c"));
        assert!(view.contains("parse_header"));
        assert!(view.contains("1204"));
        assert!(view.contains("142-160"), "uncovered range must be listed");
        assert!(view.contains("60.00%"), "file summary must be included");
    }

    #[test]
    fn missing_source_view_groups_per_file_when_no_file_requested() {
        let view = render_missing_source_view(None, Some(&sample_export()));
        assert!(view.contains("/build/src/parser.c"), "file header expected");
        assert!(view.contains("parse_header"));
    }

    #[test]
    fn missing_source_view_without_export_data() {
        let view = render_missing_source_view(Some(Path::new("/build/src/parser.c")), None);
        assert!(view.starts_with(COVERAGE_SOURCE_UNAVAILABLE_MARKER));
        assert!(view.contains("No region data available"));
    }
}
