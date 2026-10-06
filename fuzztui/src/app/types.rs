// SPDX-FileCopyrightText: © 2026 LEDGER SAS
// SPDX-License-Identifier: MIT

use std::path::PathBuf;
use std::time::Instant;

use chrono::NaiveDateTime;
use scheduler::{Sha, TaskStatus};

use crate::launcher::ProcessStatus;

#[derive(Debug)]
pub(crate) struct TabsState {
    pub titles: Vec<String>,
    pub index: usize,
}

impl TabsState {
    pub fn new(titles: Vec<String>) -> Self {
        Self { titles, index: 0 }
    }
    pub fn next(&mut self) {
        if !self.titles.is_empty() {
            self.index = (self.index + 1) % self.titles.len();
        }
    }

    pub fn previous(&mut self) {
        if !self.titles.is_empty() {
            if self.index > 0 {
                self.index -= 1;
            } else {
                self.index = self.titles.len() - 1;
            }
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct TaskInfo {
    pub sha: Sha,
    pub sha_hex: String,
    pub path: PathBuf,
    pub is_running: TaskStatus,
    pub started_at: Option<Instant>,
    pub crashed_at: Option<Instant>,
    pub stopped_at: Option<Instant>,
}

#[derive(Debug, Clone)]
pub(crate) struct CrashInfo {
    pub id: i32,
    pub filename: String,
    pub scheduler_name: String,
    pub created_at: NaiveDateTime,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum CoverageMode {
    Loading,
    FileList,
    FileView,
}

pub(crate) enum CoveragePayload {
    Summary(String),
    FileDetail(String),
}

pub(crate) struct CoverageResult {
    pub request_id: u64,
    pub payload: CoveragePayload,
}

#[derive(Debug, Clone)]
pub(crate) struct ProcessDisplayInfo {
    pub name: String,
    pub status: ProcessStatus,
    pub started_at: Instant,
    pub info: String,
}

#[derive(Debug, Clone)]
pub(crate) struct SchedulerDisplayInfo {
    pub name: String,
    pub status: ProcessStatus,
    pub started_at: Instant,
    pub info: String,
    pub last_build_at: Option<Instant>,
    pub building_since: Option<Instant>,
    pub state_file_path: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct LauncherDisplayCache {
    pub procs: Vec<ProcessDisplayInfo>,
    pub config_status: String,
    pub schedulers: Vec<SchedulerDisplayInfo>,
}

impl LauncherDisplayCache {
    pub fn empty() -> Self {
        Self {
            procs: Vec::new(),
            config_status: String::new(),
            schedulers: Vec::new(),
        }
    }
}

/// State for the coverage view (summary table + file-by-file view).
#[derive(Debug, Clone)]
pub(crate) struct CoverageViewState {
    pub view_open: bool,
    pub data: Option<String>,
    pub scroll_offset: u16,
    pub mode: CoverageMode,
    pub file_data: Option<String>,
    pub summary_rows: Vec<Vec<String>>,
    pub file_paths: Vec<Option<String>>,
    pub file_col: Option<usize>,
    pub summary_index: usize,
    pub project_mode: bool,
}

impl CoverageViewState {
    pub fn empty() -> Self {
        Self {
            view_open: false,
            data: None,
            scroll_offset: 0,
            mode: CoverageMode::FileList,
            file_data: None,
            summary_rows: Vec::new(),
            file_paths: Vec::new(),
            file_col: None,
            summary_index: 0,
            project_mode: false,
        }
    }

    pub fn data_row_count(&self) -> usize {
        self.summary_rows.len().saturating_sub(1)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct UpdateMessage {
    pub crashes: Vec<CrashInfo>,
    pub last_crashes_date: Option<NaiveDateTime>,
}
