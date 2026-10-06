// SPDX-FileCopyrightText: © 2026 LEDGER SAS
// SPDX-License-Identifier: MIT

//! Unit tests for the private helpers of the [`super`] module.

use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;

use super::llvm::{coverage_mounts, merge_profdata, parse_unreadable_sources};
use super::render::{
    merge_ranges, parse_functions, render_missing_source_view, same_source_file,
    COVERAGE_SOURCE_UNAVAILABLE_MARKER,
};
use super::runner::{
    build_container_args, is_tool_not_found, llvm_tool_container, tool_not_found_error,
    CoverageRunner, Mount,
};
use super::source::{
    dedup_ancestors, is_safe_source_mount, locate_sources, resolve_safe_source_mount,
};
use super::summary::{extract_file_metrics, extract_summary_metrics, MAX_COVERAGE_FILES};

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
    assert!(is_tool_not_found(&status, "", "llvm-profdata"));
}

#[test]
fn is_tool_not_found_stderr_pattern() {
    let status = ExitStatus::from_raw(1 << 8);
    assert!(is_tool_not_found(
        &status,
        "OCI runtime exec failed: executable file not found in $PATH",
        "llvm-profdata"
    ));
    assert!(is_tool_not_found(
        &status,
        "llvm-profdata: No such file or directory",
        "llvm-profdata"
    ));
}

#[test]
fn is_tool_not_found_normal_failure() {
    let status = ExitStatus::from_raw(1 << 8);
    assert!(!is_tool_not_found(
        &status,
        "merge failed: corrupted data",
        "llvm-profdata"
    ));
}

/// A missing input file must not be mistaken for a missing tool, otherwise the
/// user is told to rebuild their image instead of fixing their profile paths.
#[test]
fn is_tool_not_found_ignores_missing_data_file() {
    let status = ExitStatus::from_raw(1 << 8);
    assert!(!is_tool_not_found(
        &status,
        "/data/foo.profraw: No such file or directory",
        "llvm-profdata"
    ));
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
    let binaries = vec![PathBuf::from("/data/coverage/fuzzer1/fuzzer_abc.bin")];
    let mounts = coverage_mounts(coverage_dir, &binaries);
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
    let binaries = vec![PathBuf::from("/opt/targets/fuzzer1")];
    let mounts = coverage_mounts(coverage_dir, &binaries);
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
    let loc = locate_sources(&embedded, &[binary]).expect("sources should be located");

    assert!(
        loc.flag.is_none(),
        "no path-equivalence needed when the recorded path exists"
    );
    assert_eq!(loc.mounts, vec![src_dir.canonicalize().unwrap()]);
}

#[test]
fn locate_sources_derives_path_equivalence_when_tree_moved() {
    let tmp = assert_fs::TempDir::new().unwrap();
    std::fs::write(tmp.path().join("parser.c"), "int x;\n").unwrap();
    let binary = tmp.path().join("fuzzer");

    let embedded = vec!["/build/elsewhere/parser.c".to_string()];
    let loc = locate_sources(&embedded, &[binary]).expect("sources should be located");

    let flag = loc.flag.expect("moved tree needs a path-equivalence flag");
    assert!(flag.starts_with("-path-equivalence=/build/elsewhere,"));
    assert_eq!(loc.mounts, vec![tmp.path().canonicalize().unwrap()]);
}

#[test]
fn locate_sources_none_when_nothing_on_disk() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let embedded = vec!["/build/gone/parser.c".to_string()];
    assert!(locate_sources(&embedded, &[tmp.path().join("fuzzer")]).is_none());
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

#[test]
fn merge_profdata_empty_input_returns_error() {
    let runner = CoverageRunner::Host { llvm_bin_dir: None };
    let result = merge_profdata(
        &[],
        Path::new("/tmp/out.profdata"),
        Path::new("/tmp"),
        &runner,
    );
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("No profraw files available"));
}

#[test]
fn coverage_mounts_multi_deduplicates_coverage_dir() {
    let coverage_dir = Path::new("/data/coverage");
    let binaries = vec![
        PathBuf::from("/data/coverage/fuzzer_a/fuzzer_a.bin"),
        PathBuf::from("/data/coverage/fuzzer_b/fuzzer_b.bin"),
    ];
    let mounts = coverage_mounts(coverage_dir, &binaries);
    assert_eq!(mounts.len(), 1, "all binaries are under coverage_dir");
    assert_eq!(mounts[0].host_path, PathBuf::from("/data/coverage"));
}

#[test]
fn coverage_mounts_multi_adds_external_binary_dirs() {
    let coverage_dir = Path::new("/data/coverage");
    let binaries = vec![
        PathBuf::from("/data/coverage/fuzzer_a.bin"),
        PathBuf::from("/opt/targets/fuzzer_b"),
    ];
    let mounts = coverage_mounts(coverage_dir, &binaries);
    assert_eq!(mounts.len(), 2);
    assert_eq!(mounts[1].host_path, PathBuf::from("/opt/targets"));
    assert!(mounts[1].read_only);
}

#[test]
fn locate_sources_tries_each_binary() {
    let tmp = assert_fs::TempDir::new().unwrap();
    std::fs::write(tmp.path().join("parser.c"), "int x;\n").unwrap();
    let binary = tmp.path().join("fuzzer");
    let other = PathBuf::from("/nonexistent/bin/fuzzer2");

    let embedded = vec!["/build/elsewhere/parser.c".to_string()];
    let loc =
        locate_sources(&embedded, &[other, binary]).expect("should find via the second binary");
    assert!(loc.flag.is_some());
}

#[test]
fn locate_sources_none_when_no_binary_matches() {
    let embedded = vec!["/build/gone/parser.c".to_string()];
    let binaries = [
        PathBuf::from("/nonexistent/a"),
        PathBuf::from("/nonexistent/b"),
    ];
    assert!(locate_sources(&embedded, &binaries).is_none());
}

/// Sources resolved to a system directory must never be bind-mounted into the
/// coverage container, even read-only.
#[test]
fn is_safe_source_mount_rejects_system_directories() {
    assert!(!is_safe_source_mount(Path::new("/")));
    assert!(!is_safe_source_mount(Path::new("/etc")));
    assert!(!is_safe_source_mount(Path::new("/root/.ssh")));
    assert!(!is_safe_source_mount(Path::new("/home")));
    assert!(is_safe_source_mount(Path::new("/home/user/project/src")));
    assert!(is_safe_source_mount(Path::new("/srv/build/src")));
    assert!(is_safe_source_mount(Path::new("/var/tmp/build/src")));
    assert!(!is_safe_source_mount(Path::new("/var/log/app")));
}

#[test]
fn resolve_safe_source_mount_rejects_symlink_to_system_dir() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let link = tmp.path().join("src");
    std::os::unix::fs::symlink("/etc", &link).unwrap();
    assert!(
        resolve_safe_source_mount(&link).is_none(),
        "a symlink whose target is forbidden must not become a bind-mount"
    );
}

#[test]
fn resolve_safe_source_mount_returns_canonical_real_dir() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let src = tmp.path().join("project").join("src");
    std::fs::create_dir_all(&src).unwrap();
    let resolved = resolve_safe_source_mount(&src).expect("real project src should be mountable");
    assert_eq!(resolved, src.canonicalize().unwrap());
}

#[test]
fn extract_summary_metrics_full_payload() {
    let json: serde_json::Value = serde_json::json!({
        "data": [{
            "totals": {
                "lines": {"count": 100, "covered": 75},
                "functions": {"count": 20, "covered": 15},
                "regions": {"count": 50, "covered": 30},
                "branches": {"count": 40, "covered": 10}
            }
        }]
    });
    let summary = extract_summary_metrics(&json).unwrap();
    assert_eq!(summary.metrics.len(), 4);
    let lines = summary
        .metrics
        .iter()
        .find(|m| m.metric_name == "lines")
        .unwrap();
    assert_eq!(lines.covered, 75);
    assert_eq!(lines.total, 100);
    let branches = summary
        .metrics
        .iter()
        .find(|m| m.metric_name == "branches")
        .unwrap();
    assert_eq!(branches.covered, 10);
    assert_eq!(branches.total, 40);
}

#[test]
fn extract_summary_metrics_partial_payload() {
    let json: serde_json::Value = serde_json::json!({
        "data": [{
            "totals": {
                "lines": {"count": 50, "covered": 25},
                "functions": {"count": 10, "covered": 8}
            }
        }]
    });
    let summary = extract_summary_metrics(&json).unwrap();
    assert_eq!(summary.metrics.len(), 2);
}

#[test]
fn extract_summary_metrics_empty_totals() {
    let json: serde_json::Value = serde_json::json!({
        "data": [{
            "totals": {
                "lines": {"count": 0, "covered": 0}
            }
        }]
    });
    assert!(extract_summary_metrics(&json).is_none());
}

#[test]
fn extract_summary_metrics_missing_data() {
    let json: serde_json::Value = serde_json::json!({"version": "1.0"});
    assert!(extract_summary_metrics(&json).is_none());
}

fn file_entry(name: &str, lines: i64, covered: i64) -> serde_json::Value {
    serde_json::json!({
        "filename": name,
        "summary": {
            "lines": {"count": lines, "covered": covered},
            "functions": {"count": 4, "covered": 2},
            "regions": {"count": 8, "covered": 3},
            "branches": {"count": 2, "covered": 1}
        }
    })
}

#[test]
fn file_metrics_come_with_the_totals() {
    let json: serde_json::Value = serde_json::json!({
        "data": [{
            "totals": {"lines": {"count": 100, "covered": 75}},
            "files": [file_entry("/src/a.rs", 60, 30), file_entry("/src/b.rs", 40, 45)]
        }]
    });
    let summary = extract_summary_metrics(&json).unwrap();
    assert_eq!(summary.files.len(), 2);
    let a = summary
        .files
        .iter()
        .find(|f| f.path == "/src/a.rs")
        .unwrap();
    assert_eq!(a.lines_covered, 30);
    assert_eq!(a.lines_total, 60);
    assert_eq!(a.branches_total, 2);
}

#[test]
fn files_without_a_mapping_are_dropped() {
    let json = serde_json::json!({
        "data": [{"files": [serde_json::json!({
            "filename": "/src/empty.rs",
            "summary": {
                "lines": {"count": 0, "covered": 0},
                "functions": {"count": 0, "covered": 0},
                "regions": {"count": 0, "covered": 0}
            }
        })]}]
    });
    assert!(extract_file_metrics(&json).is_empty());
}

#[test]
fn a_missing_section_defaults_to_zero() {
    let json = serde_json::json!({
        "data": [{"files": [serde_json::json!({
            "filename": "/src/a.rs",
            "summary": {"lines": {"count": 10, "covered": 4}}
        })]}]
    });
    let files = extract_file_metrics(&json);
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].branches_total, 0);
    assert_eq!(files[0].lines_covered, 4);
}

#[test]
fn the_file_list_is_capped_on_the_worst_offenders() {
    let files: Vec<serde_json::Value> = (0..MAX_COVERAGE_FILES + 10)
        .map(|i| file_entry(&format!("/src/f{i}.rs"), i as i64 + 1, 0))
        .collect();
    let json = serde_json::json!({"data": [{"files": files}]});
    let kept = extract_file_metrics(&json);
    assert_eq!(kept.len(), MAX_COVERAGE_FILES);
    assert!(kept.iter().all(|f| f.lines_total > 10));
}

#[test]
fn missing_files_section_is_not_an_error() {
    let json = serde_json::json!({"data": [{"totals": {}}]});
    assert!(extract_file_metrics(&json).is_empty());
}
