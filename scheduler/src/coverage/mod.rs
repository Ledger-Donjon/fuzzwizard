// SPDX-FileCopyrightText: © 2026 LEDGER SAS
// SPDX-License-Identifier: MIT

#[cfg(test)]
mod tests;

mod llvm;
mod render;
mod runner;
mod source;
mod summary;

use std::path::{Path, PathBuf};

use log::warn;

pub use render::COVERAGE_SOURCE_UNAVAILABLE_MARKER;
pub use runner::CoverageRunner;
pub use summary::{
    extract_summary_metrics, CoverageFileEntry, CoverageMetricEntry, CoverageSummary,
};

use llvm::{embedded_source_paths, export_json, merge_profdata, run_report, run_show, ShowOutcome};
use render::render_missing_source_view;
use source::{locate_sources, SourceLocation};

/// A coverage query: one merged profile over a set of instrumented binaries.
///
/// A single fuzzer and a whole project only differ by how many binaries the
/// request carries, so both go through the same code path.
pub struct CoverageRequest {
    /// Directory holding the `.profraw` files. Also where the merged profile is
    /// written, and the only read-write mount of a containerised run.
    pub coverage_dir: PathBuf,
    /// File name of the merged profile inside `coverage_dir`.
    pub profdata_name: String,
    /// Raw profiles to merge.
    pub profraw_paths: Vec<PathBuf>,
    /// Instrumented binaries handed to `llvm-cov` as objects.
    pub binaries: Vec<PathBuf>,
    /// On-disk binary locations used to find the source tree. They differ from
    /// [`Self::binaries`], which point at the copies kept for coverage and
    /// therefore say nothing about where the sources live.
    pub source_anchors: Vec<PathBuf>,
    pub runner: CoverageRunner,
}

/// The profile and source location shared by every command of a request.
struct ResolvedRequest {
    profdata: PathBuf,
    source_loc: Option<SourceLocation>,
}

impl CoverageRequest {
    fn resolve(&self) -> Result<ResolvedRequest, String> {
        let profraw_refs: Vec<&Path> = self.profraw_paths.iter().map(PathBuf::as_path).collect();
        let profdata_out = self.coverage_dir.join(&self.profdata_name);
        let profdata = merge_profdata(
            &profraw_refs,
            &profdata_out,
            &self.coverage_dir,
            &self.runner,
        )?;

        let embedded =
            embedded_source_paths(&profdata, &self.binaries, &self.runner, &self.coverage_dir);
        let source_loc = locate_sources(&embedded, &self.source_anchors);
        Ok(ResolvedRequest {
            profdata,
            source_loc,
        })
    }

    /// Coverage summary (`llvm-cov report`), prefixed by `title` and the paths
    /// the numbers come from. Errors are returned as the report text, which is
    /// what the TUI displays.
    pub fn report(&self, title: &str) -> String {
        let resolved = match self.resolve() {
            Ok(r) => r,
            Err(e) => return e,
        };
        let report = run_report(
            &resolved.profdata,
            &self.binaries,
            &self.runner,
            resolved.source_loc.as_ref(),
            &self.coverage_dir,
        );

        format!(
            "{title}\nCoverage Directory: {}\nProfile: {}\nBinaries: {}\n\n{report}",
            self.coverage_dir.display(),
            resolved.profdata.display(),
            self.binaries.len(),
        )
    }

    /// Line-by-line annotated source.
    ///
    /// `file_path` selects a single file; `None` returns every file. When
    /// `llvm-cov` cannot read the sources, region-level coverage is rendered
    /// instead of an empty panel.
    pub fn annotated_source(&self, file_path: Option<&Path>) -> String {
        let resolved = match self.resolve() {
            Ok(r) => r,
            Err(e) => return e,
        };

        let unreadable = match run_show(
            &resolved.profdata,
            &self.binaries,
            file_path,
            &self.runner,
            resolved.source_loc.as_ref(),
            &self.coverage_dir,
        ) {
            Ok(ShowOutcome::Annotated(s)) if !s.trim().is_empty() => return s,
            Ok(ShowOutcome::Annotated(_)) => Vec::new(),
            Ok(ShowOutcome::NoSource(paths)) => paths,
            Err(e) => return e,
        };

        warn!(
            "No annotated source for {file_path:?} (binaries={:?}, sources={:?}); \
             llvm-cov could not read {unreadable:?}; falling back to region-level coverage",
            self.binaries, resolved.source_loc
        );
        let export = export_json(
            &resolved.profdata,
            &self.binaries,
            &self.runner,
            &self.coverage_dir,
            false,
        );
        render_missing_source_view(file_path, export.as_ref())
    }

    /// Structured coverage summary for persistence, or `None` when there is
    /// nothing to measure.
    pub fn summary(&self) -> Option<CoverageSummary> {
        if self.profraw_paths.is_empty() || self.binaries.is_empty() {
            return None;
        }
        let resolved = self.resolve().ok()?;
        let json = export_json(
            &resolved.profdata,
            &self.binaries,
            &self.runner,
            &self.coverage_dir,
            true,
        )?;
        extract_summary_metrics(&json)
    }
}
