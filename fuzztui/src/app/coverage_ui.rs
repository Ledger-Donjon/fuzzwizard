// SPDX-FileCopyrightText: © 2026 LEDGER SAS
// SPDX-License-Identifier: MIT

//! Coverage overlay logic: triggering computation, parsing summaries, and
//! loading individual file views.

use std::thread;

use log::info;

use super::types::*;
use super::App;

impl App {
    /// Returns the currently selected task for the given scheduler tab, or None.
    pub(crate) fn selected_task(&self, scheduler_index: usize) -> Option<TaskInfo> {
        self.tasks_cache.get(&scheduler_index).and_then(|tasks| {
            if self.task_selection_index < tasks.len() {
                Some(tasks[self.task_selection_index].clone())
            } else {
                None
            }
        })
    }

    /// Open the coverage overlay for a single task (`Some(task)`) or
    /// project-wide (`None`).
    pub(crate) fn display_coverage(&mut self, scheduler_index: usize, task: Option<&TaskInfo>) {
        self.coverage.view_open = true;
        self.coverage.scroll_offset = 0;
        self.coverage.mode = CoverageMode::Loading;
        self.coverage.data = None;
        self.coverage.file_data = None;
        self.coverage.summary_rows = Vec::new();
        self.coverage.file_paths = Vec::new();
        self.coverage.file_col = None;
        self.coverage.summary_index = 0;
        self.coverage.project_mode = task.is_none();

        let launcher = self.launcher.clone();
        let sha = task.map(|t| t.sha);
        let tx = self.coverage_sender.clone();
        let request_id = self.next_coverage_request();

        thread::spawn(move || {
            let result = match launcher.read() {
                Ok(l) => {
                    if scheduler_index >= l.scheduler_processes.len() {
                        "Error: Scheduler not found".to_string()
                    } else {
                        let sched = &l.scheduler_processes[scheduler_index].scheduler;
                        match sha {
                            Some(s) => sched.get_task_coverage(&s),
                            None => sched.get_project_coverage(),
                        }
                    }
                }
                Err(_) => "Error: Scheduler lock poisoned (internal error)".to_string(),
            };
            let _ = tx.send(CoverageResult {
                request_id,
                payload: CoveragePayload::Summary(result),
            });
        });
    }

    /// Allocate the identifier for a coverage request about to be spawned,
    /// invalidating any result still in flight.
    pub(crate) fn next_coverage_request(&mut self) -> u64 {
        self.coverage_request_id = self.coverage_request_id.wrapping_add(1);
        self.coverage_request_id
    }

    /// Path of the file at the currently selected summary row.
    pub(crate) fn selected_file_path(&self) -> Option<String> {
        self.coverage
            .file_paths
            .get(self.coverage.summary_index)
            .and_then(|p| p.clone())
    }

    /// Parse coverage summary text (from `llvm-cov report`) into table rows and
    /// a parallel vec of file paths (one per row, None for the header).
    pub(crate) fn parse_coverage_summary(
        summary_text: &str,
    ) -> (Vec<Vec<String>>, Vec<Option<String>>, Option<usize>) {
        let mut rows = Vec::new();
        let mut file_paths: Vec<Option<String>> = Vec::new();
        let mut file_col: Option<usize> = None;
        let mut in_table = false;
        let mut header_found = false;

        for line in summary_text.lines() {
            let trimmed = line.trim();

            if trimmed.is_empty() {
                continue;
            }

            if !header_found
                && (trimmed.contains("Filename")
                    || trimmed.contains("Regions")
                    || trimmed.contains("Functions")
                    || trimmed.contains("Lines")
                    || trimmed.contains("Branches"))
            {
                header_found = true;
                in_table = true;
                let header_cols: Vec<String> =
                    trimmed.split_whitespace().map(|s| s.to_string()).collect();
                file_col = header_cols
                    .iter()
                    .position(|c| c.to_lowercase().contains("file"));
                if !header_cols.is_empty() {
                    rows.push(header_cols);
                    file_paths.push(None);
                }
                continue;
            }

            if !in_table {
                continue;
            }

            if trimmed.starts_with("---")
                || trimmed.starts_with("===")
                || trimmed.contains("TOTAL")
                || trimmed.contains("Summary")
            {
                if trimmed.contains('%') || trimmed.chars().any(|c| c.is_numeric()) {
                    let cols: Vec<String> =
                        trimmed.split_whitespace().map(|s| s.to_string()).collect();
                    if cols.len() >= 3 {
                        file_paths.push(Self::row_file_path(&cols, file_col));
                        rows.push(cols);
                    }
                }
                continue;
            }

            let cols: Vec<String> = trimmed.split_whitespace().map(|s| s.to_string()).collect();

            if cols.len() >= 3 {
                file_paths.push(Self::row_file_path(&cols, file_col));
                rows.push(cols);
            } else if cols.len() == 1
                && !trimmed.contains("Coverage")
                && !trimmed.contains("Analysis")
            {
                break;
            }
        }

        (rows, file_paths, file_col)
    }

    /// Openable file of a summary row, if any.
    fn row_file_path(cols: &[String], file_col: Option<usize>) -> Option<String> {
        file_col
            .and_then(|i| cols.get(i))
            .filter(|p| !p.to_uppercase().starts_with("TOTAL"))
            .cloned()
    }

    /// Resets coverage view state (close view and clear all coverage-related data).
    pub(crate) fn reset_coverage_view(&mut self) {
        self.coverage = CoverageViewState::empty();
    }

    pub(crate) fn load_coverage_file(&mut self) {
        if self.tabs.index == 0 {
            return;
        }

        let scheduler_index = self.tabs.index - 1;
        let file_path = match self.selected_file_path() {
            Some(p) => p,
            None => return,
        };
        info!("Loading coverage file: {file_path}");

        let is_total = file_path.to_uppercase().starts_with("TOTAL");
        let file_path_opt = if is_total {
            None
        } else {
            Some(std::path::PathBuf::from(&file_path))
        };

        let project_mode = self.coverage.project_mode;

        let task_sha = if !project_mode {
            if let Some(tasks) = self.tasks_cache.get(&scheduler_index) {
                if self.task_selection_index < tasks.len() {
                    Some(tasks[self.task_selection_index].sha)
                } else {
                    return;
                }
            } else {
                return;
            }
        } else {
            None
        };

        self.coverage.mode = CoverageMode::Loading;
        self.coverage.file_data = None;
        self.coverage.scroll_offset = 0;

        let launcher = self.launcher.clone();
        let tx = self.coverage_sender.clone();
        let request_id = self.next_coverage_request();

        thread::spawn(move || {
            let result = match launcher.read() {
                Ok(l) => {
                    if scheduler_index >= l.scheduler_processes.len() {
                        "Error: scheduler index out of range.".to_string()
                    } else {
                        let sched = &l.scheduler_processes[scheduler_index].scheduler;
                        match task_sha {
                            Some(sha) => {
                                sched.get_task_file_coverage(&sha, file_path_opt.as_deref())
                            }
                            None => sched.get_project_file_coverage(file_path_opt.as_deref()),
                        }
                    }
                }
                Err(_) => "Could not read scheduler (lock poisoned). Restart fuzztui.".to_string(),
            };
            let _ = tx.send(CoverageResult {
                request_id,
                payload: CoveragePayload::FileDetail(result),
            });
        });
    }
}
