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
    Host {
        llvm_bin_dir: Option<PathBuf>,
    },
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

    for m in mounts {
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
    lower.contains("executable file not found")
        || lower.contains("no such file or directory")
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
    runner: &CoverageRunner,
    coverage_dir: &Path,
) -> Vec<String> {
    let tool_args = vec![
        "export".into(),
        "--summary-only".into(),
        "-instr-profile".into(),
        profdata.to_string_lossy().into_owned(),
        binary.to_string_lossy().into_owned(),
    ];
    let mounts = coverage_mounts(coverage_dir, binary);
    let output = match runner.command("llvm-cov", &tool_args, &mounts).output() {
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

/// Result of path-equivalence detection: the flag for llvm-cov plus the host
/// directory that should be mounted read-only for source-level commands.
#[derive(Debug)]
struct PathEquivalence {
    flag: String,
    host_source_dir: PathBuf,
}

/// Detect container/host path mismatch and return a `-path-equivalence=FROM,TO` flag if needed.
///
/// Walks up from `host_binary_path` looking for an embedded source file on disk to derive
/// the mapping between container and host paths.
fn detect_path_equivalence(
    embedded_paths: &[String],
    host_binary_path: &Path,
) -> Option<PathEquivalence> {
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
                let flag = format!("-path-equivalence={emb_str},{host_str}");
                info!("Path equivalence detected: {flag}");
                return Some(PathEquivalence {
                    flag,
                    host_source_dir: dir.to_path_buf(),
                });
            }
            candidate = dir.parent();
        }
    }

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

/// Run `llvm-cov report` and return the text output.
fn run_report(
    profdata: &Path,
    binary: &Path,
    runner: &CoverageRunner,
    path_equiv: Option<&PathEquivalence>,
    coverage_dir: &Path,
) -> String {
    let mut tool_args: Vec<String> = vec![
        "report".into(),
        binary.to_string_lossy().into_owned(),
        "-instr-profile".into(),
        profdata.to_string_lossy().into_owned(),
        "-use-color=false".into(),
    ];
    if let Some(pe) = path_equiv {
        tool_args.push(pe.flag.clone());
    }
    let mut mounts = coverage_mounts(coverage_dir, binary);
    if let Some(pe) = path_equiv {
        mounts.push(Mount {
            host_path: pe.host_source_dir.clone(),
            read_only: true,
        });
    }
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
    path_equiv: Option<&PathEquivalence>,
    coverage_dir: &Path,
) -> Result<String, String> {
    let mut tool_args: Vec<String> = vec![
        "show".into(),
        "-instr-profile".into(),
        profdata.to_string_lossy().into_owned(),
        binary.to_string_lossy().into_owned(),
        "-format=text".into(),
        "-show-line-counts-or-regions".into(),
        "-use-color=false".into(),
    ];
    if let Some(pe) = path_equiv {
        tool_args.push(pe.flag.clone());
    }

    let mut mounts = coverage_mounts(coverage_dir, binary);
    if let Some(pe) = path_equiv {
        mounts.push(Mount {
            host_path: pe.host_source_dir.clone(),
            read_only: true,
        });
    }

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
    runner: &CoverageRunner,
) -> String {
    let (profdata, binary) = match resolve_profdata_and_binary(
        coverage_dir,
        binary_path,
        sha,
        profraw_path,
        runner,
    ) {
        Ok(x) => x,
        Err(e) => return e,
    };

    let embedded = embedded_source_paths(&profdata, &binary, runner, coverage_dir);
    let path_equiv = detect_path_equivalence(&embedded, binary_path);
    let report = run_report(
        &profdata,
        &binary,
        runner,
        path_equiv.as_ref(),
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
    let (profdata, binary) = match resolve_profdata_and_binary(
        coverage_dir,
        binary_path,
        sha,
        profraw_path,
        runner,
    ) {
        Ok(x) => x,
        Err(e) => return e,
    };

    let embedded = embedded_source_paths(&profdata, &binary, runner, coverage_dir);
    let path_equiv = detect_path_equivalence(&embedded, binary_path);

    match run_show(
        &profdata,
        &binary,
        file_path,
        runner,
        path_equiv.as_ref(),
        coverage_dir,
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

        let image_pos = args.iter().position(|a| a == "fuzzing-image:latest").unwrap();
        let tool_pos = args.iter().position(|a| a == "llvm-profdata").unwrap();
        assert!(
            image_pos < tool_pos,
            "image must appear before tool name"
        );

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
        assert_eq!(mounts.len(), 1, "no extra mount when binary is inside coverage_dir");
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
}
