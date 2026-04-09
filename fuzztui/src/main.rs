// SPDX-FileCopyrightText: © 2026 LEDGER SAS
// SPDX-License-Identifier: MIT

use chrono::{DateTime, Local, NaiveDateTime, TimeDelta, Utc};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind};
use std::io;
use std::path::PathBuf;
use std::sync::{mpsc, Arc, RwLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime};
mod launcher;
use ftail::Ftail;
use humantime::format_duration;
use launcher::{Launcher, ProcessStatus};
use log::{error, info, LevelFilter};
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style, Stylize},
    symbols::border,
    text::{Line, Span, Text},
    widgets::{Block, Clear, Paragraph, Row, Table, Tabs, Widget},
    DefaultTerminal, Frame,
};
use reqwest::Url;
use scheduler::Sha;

use scheduler::TaskStatus;

#[derive(Debug)]
pub struct TabsState {
    pub titles: Vec<String>,
    pub index: usize,
}

#[derive(Debug, Clone)]
pub struct TaskInfo {
    pub sha: Sha,
    pub sha_hex: String,
    pub path: PathBuf,
    pub is_running: TaskStatus,
    pub started_at: Option<std::time::Instant>,
    pub crashed_at: Option<std::time::Instant>,
    pub stopped_at: Option<std::time::Instant>,
}

#[derive(Debug, Clone)]
pub struct CrashInfo {
    pub id: i32,
    pub filename: String,
    pub scheduler_name: String,
    pub created_at: NaiveDateTime,
}

#[derive(Debug, Clone, PartialEq)]
enum CoverageMode {
    FileList, // Showing list of files with coverage (includes summary at top)
    FileView, // Showing file with line-by-line coverage
}

// Cache structures for display data (to avoid blocking during updates)
#[derive(Debug, Clone)]
struct ProcessDisplayInfo {
    name: String,
    status: ProcessStatus,
    started_at: Instant,
    info: String,
}

#[derive(Debug, Clone)]
struct SchedulerDisplayInfo {
    name: String,
    status: ProcessStatus,
    started_at: Instant,
    info: String,
    last_build_at: Option<Instant>,
    state_file_path: Option<String>,
}

#[derive(Debug, Clone)]
struct LauncherDisplayCache {
    procs: Vec<ProcessDisplayInfo>,
    config_status: String,
    schedulers: Vec<SchedulerDisplayInfo>,
}

/// State for the coverage view (summary table + file-by-file view).
#[derive(Debug, Clone)]
struct CoverageViewState {
    view_open: bool,
    /// Raw summary text (for fallback display when parsing fails).
    data: Option<String>,
    scroll_offset: u16,
    mode: CoverageMode,
    /// Line-by-line coverage when mode is FileView.
    file_data: Option<String>,
    /// Parsed table: row 0 = header, row 1.. = data (one row per file).
    summary_rows: Vec<Vec<String>>,
    /// File path for each summary row (index-aligned with summary_rows; None for header row).
    file_paths: Vec<Option<String>>,
    /// Column index of the Filename column in summary_rows (if detected).
    file_col: Option<usize>,
    /// Selected row index (0 = header, 1 = first file). Also identifies which file is shown in FileView.
    summary_index: usize,
}

impl CoverageViewState {
    fn empty() -> Self {
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
        }
    }

    /// Number of data rows (files).
    fn data_row_count(&self) -> usize {
        self.summary_rows.len().saturating_sub(1)
    }
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

    pub fn add_title(&mut self, title: String) {
        self.titles.push(title);
    }
}

#[derive(Debug, Clone)]
struct UpdateMessage {
    crashes: Vec<CrashInfo>,
    last_crashes_date: Option<NaiveDateTime>,
}

#[derive(Debug)]
pub struct App {
    tabs: TabsState,
    launcher: Arc<RwLock<Launcher>>,
    exit: bool,
    tasks_cache: std::collections::HashMap<usize, Vec<TaskInfo>>, // Cache tasks per scheduler index
    crashes: Vec<CrashInfo>,
    last_crashes_date: Option<NaiveDateTime>,
    last_update: Option<Instant>,
    update_sender: Option<mpsc::Sender<Option<NaiveDateTime>>>,
    update_receiver: Option<mpsc::Receiver<UpdateMessage>>,
    pending_update: bool,
    launcher_update_sender: Option<mpsc::Sender<()>>, // Request launcher update
    status_update_receiver: Option<mpsc::Receiver<()>>, // Notification of completion
    launcher_cache: LauncherDisplayCache,
    scroll_offset: u16,             // Vertical scroll offset
    config_reload_prompt: bool,     // Whether to show config reload confirmation prompt
    task_selection_index: usize,    // Index of selected task in the table (for scheduler views)
    task_menu_open: bool,           // Whether the task management menu is open
    task_menu_selection: usize,     // Index of selected action in the menu
    task_menu_actions: Vec<String>, // Available actions for the selected task
    coverage: CoverageViewState,
}

impl App {
    pub fn default(launcher: Arc<RwLock<Launcher>>) -> Self {
        // Create channels for async crash updates
        let (update_sender, update_receiver_main) = mpsc::channel();
        let (update_sender_main, update_receiver) = mpsc::channel();

        // Create channels for launcher status updates
        let (launcher_update_sender, launcher_update_receiver) = mpsc::channel();
        let (status_update_sender, status_update_receiver) = mpsc::channel();

        // Spawn background thread for crash updates
        thread::spawn(move || {
            while let Ok(date) = update_receiver_main.recv() {
                let last_date = date;

                info!("Background thread: Updating crashes");
                let crashes_result = Self::update_crashes_async(last_date);

                if let Ok((crashes, new_last_date)) = crashes_result {
                    let _ = update_sender_main.send(UpdateMessage {
                        crashes,
                        last_crashes_date: new_last_date,
                    });
                }
            }
        });

        // Spawn background thread for launcher status updates
        let launcher_for_thread = launcher.clone();
        let status_update_sender_for_thread = status_update_sender.clone();
        thread::spawn(move || {
            while launcher_update_receiver.recv().is_ok() {
                info!("Background thread: Updating launcher status");
                match launcher_for_thread.write() {
                    Ok(mut launcher) => {
                        launcher.update_status();
                    }
                    Err(e) => {
                        error!("Error acquiring launcher write lock: {e:?}");
                    }
                }
                // Notify completion
                if let Err(e) = status_update_sender_for_thread.send(()) {
                    error!("Error sending status update completion notification: {e:?}");
                }
            }
        });

        let mut app = Self {
            tabs: TabsState::new(vec![]),
            launcher: launcher.clone(),
            exit: false,
            tasks_cache: std::collections::HashMap::new(),
            crashes: Vec::new(),
            last_crashes_date: None,
            last_update: Some(Instant::now()),
            update_sender: Some(update_sender),
            update_receiver: Some(update_receiver),
            pending_update: false,
            launcher_update_sender: Some(launcher_update_sender),
            status_update_receiver: Some(status_update_receiver),
            launcher_cache: LauncherDisplayCache {
                procs: Vec::new(),
                config_status: String::new(),
                schedulers: Vec::new(),
            },
            scroll_offset: 0,
            config_reload_prompt: false,
            task_selection_index: 0,
            task_menu_open: false,
            task_menu_selection: 0,
            task_menu_actions: Vec::new(),
            coverage: CoverageViewState::empty(),
        };
        // Initialize cache from launcher
        app.update_launcher_cache();
        app.sync_tabs_with_schedulers();
        app.update_tasks_info();
        // Initial synchronous update
        app
    }

    fn parse_crash_json(crash: &serde_json::Value) -> Option<CrashInfo> {
        let id = crash.get("id")?.as_i64()? as i32;
        let filename = crash.get("filename")?.as_str()?.to_string();
        let scheduler_name = crash.get("scheduler_name")?.as_str()?.to_string();

        let created_at = if let Some(date_str) = crash.get("created_at")?.as_str() {
            date_str.to_string()
        } else {
            format!("{:?}", crash.get("created_at"))
        };

        Some(CrashInfo {
            id,
            filename,
            scheduler_name,
            created_at: NaiveDateTime::parse_from_str(created_at.as_str(), "%Y-%m-%dT%H:%M:%S%.f")
                .ok()?,
        })
    }

    fn update_crashes_async(
        last_crashes_date: Option<NaiveDateTime>,
    ) -> Result<(Vec<CrashInfo>, Option<NaiveDateTime>), ()> {
        let client = reqwest::blocking::Client::new();
        info!("last_crashes_date: {last_crashes_date:?}");
        // If no last_crashes_date, fetch the last 10 crashes and get the date of the last one
        if last_crashes_date.is_none() {
            info!("fetching last 10 crashes");
            let url = Url::parse("http://127.0.0.1:3000/crashes/last/10").map_err(|_| ())?;
            info!("url: {}", url.as_str());
            let response = client.get(url.as_str()).send().map_err(|_| ())?;
            info!("response: {response:?}");

            if response.status().is_success() {
                let crashes_json: Vec<serde_json::Value> = response.json().map_err(|_| ())?;
                if crashes_json.is_empty() {
                    info!("no crashes found");
                    return Ok((Vec::new(), None));
                }

                let mut new_crashes: Vec<CrashInfo> = crashes_json
                    .into_iter()
                    .filter_map(|crash| Self::parse_crash_json(&crash))
                    .collect();

                if !new_crashes.is_empty() {
                    new_crashes.sort_by(|a, b| b.created_at.cmp(&a.created_at));
                    // Get the date of the last (most recent) crash
                    let last_date = new_crashes.first().map(|c| c.created_at);
                    return Ok((new_crashes, last_date));
                } else {
                    return Ok((Vec::new(), None));
                }
            } else {
                return Ok((Vec::new(), None));
            }
        }

        // If we have a date, we need to fetch crashes since that date
        // Use /crashes/ids?after to get IDs, then fetch each crash individually
        let mut ids_url = Url::parse("http://127.0.0.1:3000/crashes/ids").map_err(|_| ())?;
        ids_url.query_pairs_mut().append_pair(
            "after",
            &last_crashes_date
                .unwrap()
                .format("%Y-%m-%d %H:%M:%S%.f")
                .to_string(),
        );

        info!("url: {}", ids_url.as_str());
        let ids_response = client.get(ids_url.as_str()).send().map_err(|_| ())?;
        if !ids_response.status().is_success() {
            return Ok((Vec::new(), last_crashes_date));
        }

        let crash_ids: Vec<serde_json::Value> = ids_response.json().map_err(|_| ())?;
        if crash_ids.is_empty() {
            info!("no crashes found");
            return Ok((Vec::new(), last_crashes_date));
        }

        // Fetch each crash individually
        let mut new_crashes: Vec<CrashInfo> = Vec::new();
        for crash_id_obj in crash_ids {
            if let Some(id) = crash_id_obj.get("id").and_then(|v| v.as_i64()) {
                let crash_url = format!("http://127.0.0.1:3000/crashes/{id}");
                match client.get(&crash_url).send() {
                    Ok(crash_response) => {
                        if crash_response.status().is_success() {
                            match crash_response.json::<serde_json::Value>() {
                                Ok(crash_json) => {
                                    if let Some(crash_info) = Self::parse_crash_json(&crash_json) {
                                        new_crashes.push(crash_info);
                                    } else {
                                        error!("Failed to parse crash {id} from JSON");
                                    }
                                }
                                Err(e) => {
                                    error!("Failed to parse response as JSON for crash {id}: {e}");
                                }
                            }
                        } else {
                            error!(
                                "Failed to fetch crash {}: HTTP {}",
                                id,
                                crash_response.status()
                            );
                        }
                    }
                    Err(e) => {
                        error!("Failed to fetch crash {id}: {e}");
                    }
                }
            }
        }

        if !new_crashes.is_empty() {
            new_crashes.sort_by(|a, b| b.created_at.cmp(&a.created_at));
            // Get the date of the last (most recent) crash
            let new_date = new_crashes.first().map(|c| c.created_at).unwrap();
            Ok((new_crashes, Some(new_date)))
        } else {
            Ok((Vec::new(), last_crashes_date))
        }
    }

    fn update_launcher_cache(&mut self) {
        // Try to update cache from launcher (non-blocking)
        if let Ok(launcher) = self.launcher.try_read() {
            // Update processes cache
            self.launcher_cache.procs = launcher
                .procs
                .iter()
                .map(|p| ProcessDisplayInfo {
                    name: p.name.clone(),
                    status: p.status,
                    started_at: p.started_at,
                    info: p.info.clone(),
                })
                .collect();

            // Update config status
            self.launcher_cache.config_status = launcher.config_status.clone();

            // Update schedulers cache
            self.launcher_cache.schedulers = launcher
                .scheduler_processes
                .iter()
                .map(|s| SchedulerDisplayInfo {
                    name: s.name.clone(),
                    status: s.status,
                    started_at: s.started_at,
                    info: s.info.clone(),
                    last_build_at: s.last_build_at,
                    state_file_path: s.config.state_file.clone(),
                })
                .collect();
        }
        // If try_read() fails, we keep the previous cache (no update)
    }

    pub fn run(&mut self, terminal: &mut DefaultTerminal) -> io::Result<()> {
        terminal.clear()?;
        while !self.exit {
            // Update cache from launcher (non-blocking, keeps old data if locked)
            self.update_launcher_cache();
            self.sync_tabs_with_schedulers();
            self.update_tasks_info(); // Update tasks for all schedulers

            // Check for crash updates from background thread (non-blocking)
            if let Some(ref receiver) = self.update_receiver {
                while let Ok(update_msg) = receiver.try_recv() {
                    // Merge new crashes with existing ones
                    self.crashes.append(&mut update_msg.crashes.clone());
                    self.crashes.sort_by(|a, b| b.created_at.cmp(&a.created_at));
                    self.crashes.truncate(10);

                    if let Some(date) = update_msg.last_crashes_date {
                        self.last_crashes_date = Some(date);
                    }
                }
            }

            // Check for launcher status update completion (non-blocking)
            if let Some(ref receiver) = self.status_update_receiver {
                while receiver.try_recv().is_ok() {
                    self.pending_update = false;
                }
            }

            // Update proc status (fast, non-blocking)
            if self.tabs.index == 0 {
                if let Ok(mut launcher) = self.launcher.try_write() {
                    launcher.update_proc_status();
                }
            }

            // Trigger background crash update periodically (non-blocking)
            let now = Instant::now();
            let mut should_update = false;

            match &self.last_update {
                Some(last) => {
                    if now.duration_since(*last) >= Duration::from_secs(60) {
                        should_update = true;
                        self.last_update = Some(now);
                    }
                }
                None => {
                    should_update = true;
                    self.last_update = Some(now);
                }
            }

            if should_update && !self.pending_update {
                // Trigger background crash update (non-blocking)
                if let Some(ref sender) = self.update_sender {
                    let _ = sender.send(self.last_crashes_date);
                    self.pending_update = true;
                }

                // Update launcher status asynchronously using background thread
                // Send request to permanent background thread (non-blocking)
                if let Some(ref sender) = self.launcher_update_sender {
                    let _ = sender.send(());
                }
            }

            terminal.draw(|frame: &mut Frame<'_>| self.draw(frame))?;
            self.handle_events()?;
        }
        Ok(())
    }

    fn draw(&self, frame: &mut Frame) {
        frame.render_widget(self, frame.area());
    }

    fn handle_events(&mut self) -> io::Result<()> {
        // Use poll with timeout to allow non-blocking updates
        // This allows tabs to update even when no key is pressed
        if event::poll(Duration::from_millis(100))? {
            match event::read()? {
                // it's important to check that the event is a key press event as
                // crossterm also emits key release and repeat events on Windows.
                Event::Key(key_event) if key_event.kind == KeyEventKind::Press => {
                    self.handle_key_event(key_event)
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn handle_key_event(&mut self, key_event: KeyEvent) {
        // Handle config reload prompt
        if self.config_reload_prompt {
            match key_event.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    // Reload config
                    if let Ok(mut launcher) = self.launcher.write() {
                        launcher.reload_config();
                    }
                    self.config_reload_prompt = false;
                    // Update cache after reload
                    self.update_launcher_cache();
                }
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    // Cancel reload
                    self.config_reload_prompt = false;
                }
                _ => {}
            }
            return;
        }

        // Handle coverage view
        if self.coverage.view_open {
            match key_event.code {
                KeyCode::Up => {
                    match self.coverage.mode {
                        CoverageMode::FileList => {
                            // Row 0 is header (not selectable). Data rows from 1 to len-1.
                            if self.coverage.summary_rows.len() > 1
                                && self.coverage.summary_index > 1
                            {
                                self.coverage.summary_index -= 1;
                            }
                        }
                        CoverageMode::FileView => {
                            if self.coverage.scroll_offset > 0 {
                                self.coverage.scroll_offset -= 1;
                            }
                        }
                    }
                }
                KeyCode::Down => match self.coverage.mode {
                    CoverageMode::FileList => {
                        let max_index = self.coverage.summary_rows.len().saturating_sub(1);
                        if self.coverage.summary_rows.len() > 1
                            && self.coverage.summary_index < max_index
                        {
                            self.coverage.summary_index += 1;
                        }
                    }
                    CoverageMode::FileView => {
                        if self.coverage.scroll_offset < 10000 {
                            self.coverage.scroll_offset += 1;
                        }
                    }
                },
                KeyCode::Enter => {
                    // Select file from summary table (resolve from row so order matches)
                    if self.coverage.mode == CoverageMode::FileList
                        && self.coverage.summary_index > 0
                        && self.coverage.summary_index < self.coverage.summary_rows.len()
                    {
                        self.load_coverage_file(); // resolves file from summary row and loads
                    }
                }
                KeyCode::Char('q') => {
                    // Always return to main view (close coverage)
                    self.reset_coverage_view();
                }
                KeyCode::Esc | KeyCode::Char('b') => {
                    // One level back: FileView -> FileList, FileList -> main view
                    if self.coverage.mode == CoverageMode::FileView {
                        self.coverage.mode = CoverageMode::FileList;
                        self.coverage.file_data = None;
                        self.coverage.scroll_offset = 0;
                    } else {
                        self.reset_coverage_view();
                    }
                }
                KeyCode::Char('d') => {
                    self.reset_coverage_view();
                }
                _ => {}
            }
            return;
        }

        // Handle task menu
        if self.task_menu_open {
            match key_event.code {
                KeyCode::Up => {
                    if self.task_menu_selection > 0 {
                        self.task_menu_selection -= 1;
                    }
                }
                KeyCode::Down => {
                    if self.task_menu_selection < self.task_menu_actions.len().saturating_sub(1) {
                        self.task_menu_selection += 1;
                    }
                }
                KeyCode::Enter => {
                    // Execute selected action
                    self.execute_task_action();
                    self.task_menu_open = false;
                }
                KeyCode::Esc | KeyCode::Char('m') => {
                    // Close menu
                    self.task_menu_open = false;
                }
                _ => {}
            }
            return;
        }

        match key_event.code {
            KeyCode::Char('q') => self.exit(),
            KeyCode::Char('c') => {
                self.config_reload_prompt = true;
            }
            KeyCode::Left => {
                self.tabs.previous();
                self.scroll_offset = 0; // Reset scroll when changing tabs
                self.task_selection_index = 0; // Reset task selection when changing tabs
                self.task_menu_open = false; // Close menu when changing tabs
                self.reset_coverage_view();
            }
            KeyCode::Right => {
                self.tabs.next();
                self.scroll_offset = 0; // Reset scroll when changing tabs
                self.task_selection_index = 0; // Reset task selection when changing tabs
                self.task_menu_open = false; // Close menu when changing tabs
                self.reset_coverage_view();
            }
            KeyCode::Up => {
                if self.tabs.index == 0 {
                    // In Processes view, navigate in the window
                    if self.scroll_offset > 0 {
                        self.scroll_offset -= 1;
                    }
                } else {
                    // In scheduler view, navigate in the task table
                    if self.task_selection_index > 0 {
                        self.task_selection_index -= 1;
                    }
                }
            }
            KeyCode::Down => {
                if self.tabs.index == 0 {
                    // In Processes view, navigate in the window
                    // Limit scroll to prevent overflow (ratatui will handle the rest)
                    if self.scroll_offset < 10000 {
                        self.scroll_offset += 1;
                    }
                } else {
                    // In scheduler view, navigate in the task table
                    let scheduler_index = self.tabs.index - 1;
                    if let Some(tasks) = self.tasks_cache.get(&scheduler_index) {
                        if self.task_selection_index < tasks.len().saturating_sub(1) {
                            self.task_selection_index += 1;
                        }
                    }
                }
            }
            KeyCode::Char('m') => {
                // Open task management menu if in scheduler view and a task is selected
                // Only if the scheduler is running
                if self.tabs.index > 0 {
                    let scheduler_index = self.tabs.index - 1;
                    // Check if scheduler is running
                    if scheduler_index < self.launcher_cache.schedulers.len() {
                        let scheduler_status =
                            self.launcher_cache.schedulers[scheduler_index].status;
                        if !matches!(scheduler_status, ProcessStatus::Running) {
                            return; // Scheduler is stopped, cannot interact with tasks
                        }
                    }
                    let task_status = if let Some(tasks) = self.tasks_cache.get(&scheduler_index) {
                        if !tasks.is_empty() && self.task_selection_index < tasks.len() {
                            Some(tasks[self.task_selection_index].is_running)
                        } else {
                            None
                        }
                    } else {
                        None
                    };
                    if let Some(status) = task_status {
                        self.open_task_menu(status);
                    }
                }
            }
            KeyCode::Char('d') => {
                if self.tabs.index > 0 {
                    let scheduler_index = self.tabs.index - 1;
                    if let Some(task) = self.selected_task(scheduler_index) {
                        self.display_coverage(scheduler_index, &task);
                    }
                }
            }
            _ => {}
        }
    }

    fn open_task_menu(&mut self, task_status: TaskStatus) {
        self.task_menu_actions.clear();
        match task_status {
            TaskStatus::Running => {
                self.task_menu_actions.push("STOP".to_string());
                self.task_menu_actions.push("RESTART".to_string());
            }
            TaskStatus::Stopped | TaskStatus::Crashed => {
                self.task_menu_actions.push("START".to_string());
            }
        }
        // Always add Display Coverage option if we're in a scheduler view
        if self.tabs.index > 0 {
            self.task_menu_actions.push("DISPLAY COVERAGE".to_string());
        }
        self.task_menu_selection = 0;
        self.task_menu_open = true;
    }

    fn execute_task_action(&mut self) {
        if self.tabs.index == 0 {
            return; // Not in scheduler view
        }

        let scheduler_index = self.tabs.index - 1;
        if self.task_menu_selection >= self.task_menu_actions.len() {
            return;
        }

        // Check if scheduler is running before executing action
        if scheduler_index >= self.launcher_cache.schedulers.len() {
            return;
        }
        let scheduler_status = self.launcher_cache.schedulers[scheduler_index].status;
        if !matches!(scheduler_status, ProcessStatus::Running) {
            return; // Scheduler is stopped, cannot interact with tasks
        }

        let action = self.task_menu_actions[self.task_menu_selection].clone();

        // Get SHA from cache first (before borrowing launcher)
        let sha = if let Some(tasks) = self.tasks_cache.get(&scheduler_index) {
            if self.task_selection_index >= tasks.len() {
                return;
            }
            tasks[self.task_selection_index].sha
        } else {
            return;
        };

        // Handle DISPLAY COVERAGE action separately (doesn't need launcher write lock)
        if action == "DISPLAY COVERAGE" {
            if let Some(task) = self.selected_task(scheduler_index) {
                self.display_coverage(scheduler_index, &task);
            }
            return;
        }

        // Execute action via launcher
        if let Ok(mut launcher) = self.launcher.write() {
            if scheduler_index < launcher.scheduler_processes.len() {
                // Double check scheduler is still running
                if !matches!(
                    launcher.scheduler_processes[scheduler_index].status,
                    ProcessStatus::Running
                ) {
                    return;
                }
                let scheduler = &mut launcher.scheduler_processes[scheduler_index].scheduler;
                match action.as_str() {
                    "START" => {
                        if let Err(e) = scheduler.start_task_by_sha(sha) {
                            error!("Failed to start task: {e:?}");
                        }
                    }
                    "STOP" => {
                        if let Err(e) = scheduler.stop_task_by_sha(sha) {
                            error!("Failed to stop task: {e:?}");
                        }
                    }
                    "RESTART" => {
                        if let Err(e) = scheduler.restart_task_by_sha(sha) {
                            error!("Failed to restart task: {e:?}");
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    /// Returns the currently selected task for the given scheduler tab, or None.
    fn selected_task(&self, scheduler_index: usize) -> Option<TaskInfo> {
        self.tasks_cache.get(&scheduler_index).and_then(|tasks| {
            if self.task_selection_index < tasks.len() {
                Some(tasks[self.task_selection_index].clone())
            } else {
                None
            }
        })
    }

    fn display_coverage(&mut self, scheduler_index: usize, task: &TaskInfo) {
        // Use blocking read: background thread may hold write lock during
        // `update_status()` (often longer in containers). try_read() fails spuriously then.
        match self.launcher.read() {
            Ok(launcher) => {
                if scheduler_index >= launcher.scheduler_processes.len() {
                    self.coverage.data = Some("Error: Scheduler not found".to_string());
                    self.coverage.view_open = true;
                    self.coverage.scroll_offset = 0;
                    self.coverage.mode = CoverageMode::FileList;
                    return;
                }

                let scheduler = &launcher.scheduler_processes[scheduler_index].scheduler;
                let coverage_info = scheduler.get_task_coverage(&task.sha);
                self.coverage.data = Some(coverage_info.clone());
                let (rows, paths, file_col) = Self::parse_coverage_summary(&coverage_info);
                self.coverage.summary_index = if rows.len() > 1 { 1 } else { 0 };
                self.coverage.summary_rows = rows;
                self.coverage.file_paths = paths;
                self.coverage.file_col = file_col;
                self.coverage.file_data = None;

                self.coverage.view_open = true;
                self.coverage.scroll_offset = 0;
                self.coverage.mode = CoverageMode::FileList;
            }
            Err(_) => {
                self.coverage.data =
                    Some("Error: Scheduler lock poisoned (internal error)".to_string());
                self.coverage.view_open = true;
                self.coverage.scroll_offset = 0;
                self.coverage.mode = CoverageMode::FileList;
                self.coverage.summary_rows = Vec::new();
                self.coverage.file_paths = Vec::new();
                self.coverage.file_col = None;
                self.coverage.summary_index = 0;
            }
        }
    }

    /// Path of the file at the currently selected summary row.
    fn selected_file_path(&self) -> Option<String> {
        self.coverage
            .file_paths
            .get(self.coverage.summary_index)
            .and_then(|p| p.clone())
    }

    /// Parse coverage summary text (from `llvm-cov report`) into table rows and
    /// a parallel vec of file paths (one per row, None for the header).
    fn parse_coverage_summary(
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

            // Detect table header (e.g. "Filename  Regions  ... Functions ... Lines ... Branches")
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

            // Separator / summary lines
            if trimmed.starts_with("---")
                || trimmed.starts_with("===")
                || trimmed.contains("TOTAL")
                || trimmed.contains("Summary")
            {
                if trimmed.contains('%') || trimmed.chars().any(|c| c.is_numeric()) {
                    let cols: Vec<String> =
                        trimmed.split_whitespace().map(|s| s.to_string()).collect();
                    if cols.len() >= 3 {
                        let path = file_col.and_then(|i| cols.get(i)).cloned();
                        file_paths.push(path);
                        rows.push(cols);
                    }
                }
                continue;
            }

            let cols: Vec<String> = trimmed.split_whitespace().map(|s| s.to_string()).collect();

            if cols.len() >= 3 {
                let path = file_col.and_then(|i| cols.get(i)).and_then(|p| {
                    if p.to_uppercase().starts_with("TOTAL") {
                        None
                    } else {
                        Some(p.clone())
                    }
                });
                file_paths.push(path);
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

    /// Wrap instruction spans into multiple lines if they don't fit
    fn wrap_instruction_line(spans: Vec<Span>, available_width: u16) -> Vec<Line> {
        if available_width < 20 {
            // Too narrow, just return one line (will be truncated)
            return vec![Line::from(spans)];
        }

        // First, convert spans to a string to measure total length
        let total_text: String = spans
            .iter()
            .map(|span| {
                // Get the text content from span
                // In ratatui, we can access the content via Display or by converting
                format!("{span}")
            })
            .collect();

        // If it fits in one line, return as is
        if total_text.len() as u16 <= available_width {
            return vec![Line::from(spans)];
        }

        // Clone spans for iteration since we'll need them later
        let spans_clone: Vec<Span> = spans.to_vec();

        // Otherwise, split intelligently at separator points
        let mut lines = Vec::new();
        let mut current_line = Vec::new();
        let mut current_length = 0u16;

        for span in spans_clone {
            // Get text length from span
            let span_text = format!("{span}");
            let span_len = span_text.len() as u16;

            // Check if adding this span would exceed the width
            // Account for separators (|) which are typically 3 chars including spaces
            let separator_len = if current_length > 0 { 3 } else { 0 };

            if current_length > 0 && current_length + separator_len + span_len > available_width {
                // Start a new line
                if !current_line.is_empty() {
                    lines.push(Line::from(current_line));
                }
                current_line = vec![span];
                current_length = span_len;
            } else {
                current_line.push(span);
                current_length += separator_len + span_len;
            }
        }

        // Add the last line if not empty
        if !current_line.is_empty() {
            lines.push(Line::from(current_line));
        }

        // Fallback: if we somehow have no lines, return original spans
        if lines.is_empty() {
            // This shouldn't happen, but if it does, return the original spans
            lines.push(Line::from(spans));
        }

        lines
    }

    /// Resets coverage view state (close view and clear all coverage-related data).
    fn reset_coverage_view(&mut self) {
        self.coverage = CoverageViewState::empty();
    }

    fn load_coverage_file(&mut self) {
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

        // Get task SHA from cache
        let task_sha = if let Some(tasks) = self.tasks_cache.get(&scheduler_index) {
            if self.task_selection_index < tasks.len() {
                tasks[self.task_selection_index].sha
            } else {
                return;
            }
        } else {
            return;
        };

        match self.launcher.read() {
            Ok(launcher) => {
                if scheduler_index < launcher.scheduler_processes.len() {
                    let scheduler = &launcher.scheduler_processes[scheduler_index].scheduler;
                    let file_coverage =
                        scheduler.get_task_file_coverage(&task_sha, file_path_opt.as_deref());
                    self.coverage.file_data = Some(file_coverage);
                    self.coverage.mode = CoverageMode::FileView;
                    self.coverage.scroll_offset = 0;
                } else {
                    self.coverage.file_data =
                        Some("Error: scheduler index out of range.".to_string());
                    self.coverage.mode = CoverageMode::FileView;
                }
            }
            Err(_) => {
                self.coverage.file_data =
                    Some("Could not read scheduler (lock poisoned). Restart fuzztui.".to_string());
                self.coverage.mode = CoverageMode::FileView;
            }
        }
    }
    fn sync_tabs_with_schedulers(&mut self) {
        if let Ok(launcher) = self.launcher.try_read() {
            // Start with "Processes" tab at index 0
            let mut new_titles = vec!["Processes".to_string()];

            // Add scheduler names
            let scheduler_names: Vec<String> = launcher
                .scheduler_processes
                .iter()
                .map(|s| s.name.clone())
                .collect();
            new_titles.extend(scheduler_names);

            // Only update if the tabs have changed
            if new_titles != self.tabs.titles {
                let old_index = self.tabs.index;
                self.tabs.titles = new_titles;
                // Ensure index is valid after update
                if !self.tabs.titles.is_empty() && old_index >= self.tabs.titles.len() {
                    self.tabs.index = self.tabs.titles.len() - 1;
                } else if self.tabs.titles.is_empty() {
                    self.tabs.index = 0;
                }
            }
        }
    }

    fn update_tasks_info(&mut self) {
        // Use try_write() because get_tasks_with_status() requires &mut
        // Update tasks for ALL schedulers, not just the current tab
        // If update_status() is running, we'll keep the previous tasks in cache
        if let Ok(mut launcher) = self.launcher.try_write() {
            // Update tasks for all schedulers
            for scheduler_index in 0..launcher.scheduler_processes.len() {
                let scheduler = &mut launcher.scheduler_processes[scheduler_index];
                let tasks_with_status = scheduler.scheduler.get_tasks_with_status();
                let mut tasks: Vec<TaskInfo> = tasks_with_status
                    .into_iter()
                    .map(|task_status| TaskInfo {
                        sha: task_status.sha,
                        sha_hex: hex::encode(task_status.sha),
                        path: task_status.path,
                        is_running: task_status.status,
                        started_at: Some(task_status.started_at),
                        crashed_at: task_status.crashed_at,
                        stopped_at: task_status.stopped_at,
                    })
                    .collect();

                // Sort tasks: RUNNING first, then CRASHED, then STOPPED
                tasks.sort_by(|a, b| {
                    let order = |status: &TaskStatus| match status {
                        TaskStatus::Running => 0,
                        TaskStatus::Crashed => 1,
                        TaskStatus::Stopped => 2,
                    };
                    order(&a.is_running).cmp(&order(&b.is_running))
                });

                self.tasks_cache.insert(scheduler_index, tasks);
            }
        }
        // If lock unavailable, keep existing cache (no update)
    }

    /*
    fn increment_counter(&mut self) {
        self.counter += 1;
    }

    fn decrement_counter(&mut self) {
        self.counter -= 1;
    }
    */
    fn exit(&mut self) {
        if let Ok(mut launcher) = self.launcher.write() {
            launcher.shutdown();
        }
        self.exit = true;
    }
}

impl Widget for &App {
    fn render(self, area: Rect, buf: &mut Buffer) {
        // Split the area into tabs section and content section
        let vertical = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3), // Tabs header
                Constraint::Min(0),    // Content area
            ])
            .split(area);

        // Render tabs
        if !self.tabs.titles.is_empty() {
            let titles: Vec<Line> = self
                .tabs
                .titles
                .iter()
                .map(|t| Line::from(t.as_str()))
                .collect();

            let tabs = Tabs::new(titles).select(self.tabs.index).block(
                Block::bordered()
                    .title(" Fuzzwizard TUI ")
                    .border_set(border::THICK),
            );
            tabs.render(vertical[0], buf);
        } else {
            // Show empty tabs if no schedulers
            let block = Block::bordered()
                .title(" Fuzzwizard TUI ")
                .border_set(border::THICK);
            block.render(vertical[0], buf);
        }

        // Render content for selected tab
        let content_area = vertical[1];

        // Build instructions based on current tab
        let instructions = if self.tabs.index == 0 {
            // Processes view - no Manage Task option
            Line::from(vec![
                " Quit ".into(),
                "<Q> ".blue().bold(),
                " | Navigate ".into(),
                "<Left/Right> ".blue().bold(),
                " | Scroll ".into(),
                "<Up/Down> ".blue().bold(),
                " | Reload Config ".into(),
                "<C> ".blue().bold(),
            ])
        } else {
            // Scheduler view - include Manage Task and Display Coverage options
            Line::from(vec![
                " Quit ".into(),
                "<Q> ".blue().bold(),
                " | Navigate ".into(),
                "<Left/Right> ".blue().bold(),
                " | Navigate Tasks ".into(),
                "<Up/Down> ".blue().bold(),
                " | Reload Config ".into(),
                "<C> ".blue().bold(),
                " | Manage Task ".into(),
                "<M> ".blue().bold(),
                " | Display Coverage ".into(),
                "<D> ".blue().bold(),
            ])
        };

        let block = Block::bordered()
            .title_bottom(instructions.centered())
            .border_set(border::THICK);

        // Always use cache for display (never blocks, always shows last available data)
        // Check if we're on the Processes tab (index 0)
        if self.tabs.index == 0 {
            // Display processes information
            let mut lines = vec![
                Line::from("Launcher Processes:".bold()),
                Line::from(""), // Empty line separator
            ];

            if self.launcher_cache.procs.is_empty() {
                lines.push(Line::from("  No processes running".dim()));
            } else {
                for proc in &self.launcher_cache.procs {
                    let status_text = match proc.status {
                        ProcessStatus::Running => "Running".green(),
                        ProcessStatus::Stopped => "Stopped".red(),
                    };
                    let uptime_td = TimeDelta::new(proc.started_at.elapsed().as_secs() as i64, 0)
                        .expect("Failed to create TimeDelta")
                        .to_std()
                        .unwrap();
                    let uptime_str = format_duration(uptime_td).to_string();
                    lines.push(Line::from(vec![
                        "  • ".into(),
                        proc.name.clone().yellow().bold(),
                        " - ".into(),
                        status_text,
                        " - ".into(),
                        format!("Uptime: {uptime_str}").cyan(),
                    ]));
                    lines.push(Line::from(vec![
                        "    Info: ".into(),
                        proc.info.clone().dim(),
                    ]));
                }
            }

            lines.push(Line::from("")); // Empty line separator
            lines.push(Line::from(vec![
                "Config Status".bold(),
                " - ".into(),
                self.launcher_cache.config_status.clone().yellow(),
            ]));

            // Add schedulers section
            lines.push(Line::from("")); // Empty line separator
            lines.push(Line::from("Schedulers:".bold()));
            lines.push(Line::from("")); // Empty line separator

            if self.launcher_cache.schedulers.is_empty() {
                lines.push(Line::from("  No schedulers running".dim()));
            } else {
                for scheduler in &self.launcher_cache.schedulers {
                    let status_text = match scheduler.status {
                        ProcessStatus::Running => "Running".green(),
                        ProcessStatus::Stopped => "Stopped".red(),
                    };
                    let uptime = TimeDelta::new(scheduler.started_at.elapsed().as_secs() as i64, 0)
                        .expect("Failed to create TimeDelta")
                        .to_std()
                        .unwrap();

                    let uptime_str = match scheduler.status {
                        ProcessStatus::Running => format_duration(uptime).to_string().white(),
                        ProcessStatus::Stopped => "N/A".red(),
                    };

                    lines.push(Line::from(vec![
                        "  • ".into(),
                        scheduler.name.clone().yellow().bold(),
                        " - ".into(),
                        status_text,
                        " - ".into(),
                        format!("Uptime: {uptime_str}").cyan(),
                    ]));
                    lines.push(Line::from(vec![
                        "    Info: ".into(),
                        scheduler.info.clone().dim(),
                    ]));

                    if let Some(ref state_file) = scheduler.state_file_path {
                        lines.push(Line::from(vec![
                            "    State File: ".into(),
                            state_file.clone().cyan(),
                        ]));
                    }

                    let build_tds = if let Some(last_build_at) = scheduler.last_build_at {
                        let build_td = TimeDelta::new(last_build_at.elapsed().as_secs() as i64, 0)
                            .expect("Failed to create TimeDelta")
                            .to_std()
                            .unwrap();
                        format_duration(build_td).to_string()
                    } else {
                        "Never build".to_string()
                    };

                    lines.push(Line::from(vec![
                        "    Last Build: ".into(),
                        format!("{build_tds} ago").cyan(),
                    ]));
                }
            }

            // Add recent crashes section
            lines.push(Line::from("")); // Empty line separator
            lines.push(Line::from("Recent Crashes (Last 10):".bold()));
            lines.push(Line::from("")); // Empty line separator

            if self.crashes.is_empty() {
                lines.push(Line::from("  No crashes found".dim()));
            } else {
                for crash in &self.crashes {
                    lines.push(Line::from(vec![
                        "  • ".into(),
                        format!("ID: {}", crash.id).cyan(),
                        " - ".into(),
                        crash.filename.clone().yellow(),
                        " - ".into(),
                        crash.scheduler_name.clone().green(),
                    ]));
                    lines.push(Line::from(vec![
                        "    Date: ".into(),
                        crash
                            .created_at
                            .format("%Y-%m-%d %H:%M:%S%.f")
                            .to_string()
                            .dim(),
                    ]));
                }
            }

            let text = Text::from(lines);
            Paragraph::new(text)
                .block(block)
                .scroll((self.scroll_offset, 0))
                .render(content_area, buf);
        } else {
            // Display scheduler information (index > 0)
            let scheduler_index = self.tabs.index - 1; // Adjust for "Processes" tab at index 0
            if scheduler_index < self.launcher_cache.schedulers.len() {
                let selected_scheduler = &self.launcher_cache.schedulers[scheduler_index];
                let is_scheduler_stopped =
                    matches!(selected_scheduler.status, ProcessStatus::Stopped);

                // Split content area: header (Name/Status/Info/StateFile) always visible, then table
                let vertical_split = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(6), // Fixed height for header info
                        Constraint::Min(0),    // Rest for table
                    ])
                    .split(content_area);

                // Build header content (always visible)
                // Apply dim style if scheduler is stopped
                let name_style = if is_scheduler_stopped {
                    Style::default().fg(Color::DarkGray)
                } else {
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD)
                };
                let status_style = if is_scheduler_stopped {
                    Style::default().fg(Color::DarkGray)
                } else {
                    Style::default().fg(Color::Cyan)
                };
                let info_style = if is_scheduler_stopped {
                    Style::default().fg(Color::DarkGray)
                } else {
                    Style::default()
                };
                let state_file_style = if is_scheduler_stopped {
                    Style::default().fg(Color::DarkGray)
                } else {
                    Style::default().fg(Color::Cyan)
                };
                let tasks_header_style = if is_scheduler_stopped {
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().add_modifier(Modifier::BOLD)
                };

                let mut header_lines = vec![
                    Line::from(vec![
                        "Name: ".into(),
                        Span::styled(selected_scheduler.name.clone(), name_style),
                    ]),
                    Line::from(vec![
                        "Status: ".into(),
                        Span::styled(selected_scheduler.status.to_string(), status_style),
                    ]),
                    Line::from(vec![
                        "Info: ".into(),
                        Span::styled(selected_scheduler.info.clone(), info_style),
                    ]),
                ];

                if let Some(ref state_file) = selected_scheduler.state_file_path {
                    header_lines.push(Line::from(vec![
                        "State File: ".into(),
                        Span::styled(state_file.clone(), state_file_style),
                    ]));
                }

                header_lines.push(Line::from("")); // Empty line separator
                header_lines.push(Line::from(vec![Span::styled(
                    "Tasks:".to_string(),
                    tasks_header_style,
                )]));

                let header_text = Text::from(header_lines);
                Paragraph::new(header_text)
                    .block(Block::default())
                    .render(vertical_split[0], buf);

                // Build table for tasks
                if let Some(tasks) = self.tasks_cache.get(&scheduler_index) {
                    if tasks.is_empty() {
                        let empty_text_style = if is_scheduler_stopped {
                            Style::default().fg(Color::DarkGray)
                        } else {
                            Style::default()
                        };
                        let empty_text = Text::from(vec![Line::from(vec![Span::styled(
                            "  No tasks running",
                            empty_text_style,
                        )])]);
                        Paragraph::new(empty_text)
                            .block(block)
                            .render(vertical_split[1], buf);
                    } else {
                        // Create table rows
                        let mut rows: Vec<Row> = Vec::new();

                        for (idx, task_info) in tasks.iter().enumerate() {
                            let status_text = match task_info.is_running {
                                TaskStatus::Running => "Running",
                                TaskStatus::Crashed => "Crashed",
                                TaskStatus::Stopped => "Stopped",
                            };

                            // Format StatusTime based on status
                            let status_time = match task_info.is_running {
                                TaskStatus::Running => {
                                    if let Some(started) = task_info.started_at {
                                        let elapsed = started.elapsed();
                                        let td = TimeDelta::new(elapsed.as_secs() as i64, 0)
                                            .expect("Failed to create TimeDelta")
                                            .to_std()
                                            .unwrap();
                                        format!("{} ago", format_duration(td))
                                    } else {
                                        "unknown".to_string()
                                    }
                                }
                                TaskStatus::Crashed => {
                                    if let Some(crashed) = task_info.crashed_at {
                                        let elapsed = crashed.elapsed();
                                        let crash_time_since_epoch = SystemTime::now()
                                            .duration_since(std::time::UNIX_EPOCH)
                                            .unwrap()
                                            .checked_sub(elapsed)
                                            .unwrap_or_default();
                                        let dt_utc = DateTime::<Utc>::from_timestamp(
                                            crash_time_since_epoch.as_secs() as i64,
                                            crash_time_since_epoch.subsec_nanos(),
                                        )
                                        .unwrap_or_else(Utc::now);
                                        let dt = dt_utc.with_timezone(&Local);
                                        dt.format("%Y-%m-%d %H:%M:%S").to_string()
                                    } else {
                                        "unknown".to_string()
                                    }
                                }
                                TaskStatus::Stopped => {
                                    if let Some(stopped) = task_info.stopped_at {
                                        let elapsed = stopped.elapsed();
                                        let stop_time_since_epoch = SystemTime::now()
                                            .duration_since(std::time::UNIX_EPOCH)
                                            .unwrap()
                                            .checked_sub(elapsed)
                                            .unwrap_or_default();
                                        let dt_utc = DateTime::<Utc>::from_timestamp(
                                            stop_time_since_epoch.as_secs() as i64,
                                            stop_time_since_epoch.subsec_nanos(),
                                        )
                                        .unwrap_or_else(Utc::now);
                                        let dt = dt_utc.with_timezone(&Local);
                                        dt.format("%Y-%m-%d %H:%M:%S").to_string()
                                    } else {
                                        "unknown".to_string()
                                    }
                                }
                            };

                            let sha_display = format!("0x{}", &task_info.sha_hex[..32]);
                            let filename = task_info
                                .path
                                .file_name()
                                .and_then(|n| n.to_str())
                                .unwrap_or("unknown")
                                .to_string();

                            // Apply selection highlight
                            // If scheduler is stopped, dim everything but still show selection for navigation
                            let is_selected = idx == self.task_selection_index;
                            let base_style = if is_scheduler_stopped {
                                if is_selected {
                                    // Show selection even when scheduler is stopped, but with dimmed style
                                    Style::default().bg(Color::DarkGray).fg(Color::White)
                                } else {
                                    Style::default().fg(Color::DarkGray)
                                }
                            } else if is_selected {
                                Style::default().bg(Color::Blue).fg(Color::White)
                            } else {
                                Style::default()
                            };

                            let status_color = if is_scheduler_stopped {
                                Color::DarkGray
                            } else {
                                match task_info.is_running {
                                    TaskStatus::Running => Color::Green,
                                    TaskStatus::Crashed | TaskStatus::Stopped => Color::Red,
                                }
                            };

                            let status_style = if is_selected {
                                if is_scheduler_stopped {
                                    // Show selection even when scheduler is stopped
                                    Style::default().bg(Color::DarkGray).fg(Color::White)
                                } else {
                                    Style::default().bg(Color::Blue).fg(Color::White)
                                }
                            } else {
                                Style::default().fg(status_color)
                            };

                            rows.push(Row::new(vec![
                                Span::styled(sha_display, base_style),
                                Span::styled(filename, base_style),
                                Span::styled(status_text.to_string(), status_style),
                                Span::styled(status_time, base_style),
                            ]));
                        }

                        // Create table with header
                        // Apply dim style to header if scheduler is stopped
                        let header_style = if is_scheduler_stopped {
                            Style::default()
                                .fg(Color::DarkGray)
                                .add_modifier(Modifier::BOLD)
                        } else {
                            Style::default().add_modifier(Modifier::BOLD)
                        };
                        let table = Table::new(
                            rows,
                            [
                                Constraint::Percentage(35), // SHA
                                Constraint::Percentage(25), // FileName
                                Constraint::Percentage(15), // Status
                                Constraint::Percentage(25), // StatusTime
                            ],
                        )
                        .header(Row::new(vec![
                            Span::styled("SHA", header_style),
                            Span::styled("FileName", header_style),
                            Span::styled("Status", header_style),
                            Span::styled("StatusTime", header_style),
                        ]))
                        .block(block);

                        table.render(vertical_split[1], buf);
                    }
                } else {
                    let empty_text = Text::from("  No tasks running".dim());
                    Paragraph::new(empty_text)
                        .block(block)
                        .render(vertical_split[1], buf);
                }
            } else {
                let text = Text::from("No schedulers available");
                Paragraph::new(text)
                    .block(block)
                    .scroll((self.scroll_offset, 0))
                    .render(content_area, buf);
            }
        }

        // Show coverage view as overlay if active (rendered before task menu)
        if self.coverage.view_open {
            Clear.render(content_area, buf);
            // Solid dark overlay (no DIM) so it reads as "focus panel", not "scheduler stopped"
            let overlay_bg = Color::Rgb(18, 18, 28);
            for y in content_area.y..content_area.y + content_area.height {
                for x in content_area.x..content_area.x + content_area.width {
                    let cell = &mut buf[(x, y)];
                    cell.set_bg(overlay_bg);
                    cell.set_fg(Color::DarkGray);
                    cell.set_char(' ');
                }
            }

            // Create a popup area that takes almost the entire content area
            // Leave small margins for better visibility
            let margin_x = 2;
            let margin_y = 2;
            let popup_width = content_area.width.saturating_sub(margin_x * 2);
            let popup_height = content_area.height.saturating_sub(margin_y * 2);
            let popup_area = Rect {
                x: content_area.x + margin_x,
                y: content_area.y + margin_y,
                width: popup_width,
                height: popup_height,
            };

            // Determine title and content based on mode
            let (title, coverage_text, show_file_list) = match self.coverage.mode {
                CoverageMode::FileList => {
                    let title = format!(" Coverage Summary ({}) ", self.coverage.data_row_count());
                    let text = String::new(); // Will be replaced by unified table
                    (title, text, true)
                }
                CoverageMode::FileView => {
                    let file_name = self
                        .selected_file_path()
                        .and_then(|p| p.split('/').next_back().map(String::from))
                        .unwrap_or_else(|| "Unknown".to_string());
                    let n = self.coverage.data_row_count();
                    let title = format!(
                        " File: {} ({}/{}) ",
                        file_name,
                        self.coverage.summary_index,
                        n.max(1)
                    );
                    let text = if let Some(ref data) = self.coverage.file_data {
                        data.clone()
                    } else {
                        "Loading file coverage...".to_string()
                    };
                    (title, text, false)
                }
            };

            // Use full area for unified table view
            let table_area = popup_area;

            let popup_block = Block::bordered()
                .title(title)
                .border_set(border::THICK)
                .border_style(Style::default().fg(Color::Cyan))
                .style(Style::default().bg(Color::DarkGray).fg(Color::White));

            if show_file_list {
                // Display unified summary table (contains files)
                if !self.coverage.summary_rows.is_empty() {
                    // Create table from parsed rows
                    let num_cols = if !self.coverage.summary_rows.is_empty() {
                        self.coverage.summary_rows[0].len()
                    } else {
                        1
                    };

                    let file_col = self.coverage.file_col;
                    let constraints: Vec<Constraint> = if num_cols > 1 {
                        let other_cols = num_cols - if file_col.is_some() { 1 } else { 0 };
                        let file_pct: u16 = 40;
                        let other_pct = if other_cols > 0 {
                            (100 - file_pct) / other_cols as u16
                        } else {
                            100
                        };
                        (0..num_cols)
                            .map(|i| {
                                if Some(i) == file_col {
                                    Constraint::Percentage(file_pct)
                                } else {
                                    Constraint::Percentage(other_pct)
                                }
                            })
                            .collect()
                    } else {
                        vec![Constraint::Percentage(100)]
                    };

                    // Separate header and data rows
                    let (header_cells, data_rows) = if !self.coverage.summary_rows.is_empty() {
                        // Header row (first row)
                        let header_row = &self.coverage.summary_rows[0];
                        let header_cells: Vec<ratatui::widgets::Cell> = header_row
                            .iter()
                            .enumerate()
                            .map(|(i, cell)| {
                                let max_len = if Some(i) == file_col { 60 } else { 15 };
                                let cell_text = if cell.len() > max_len {
                                    format!("{}...", &cell[..max_len - 3])
                                } else {
                                    cell.clone()
                                };
                                ratatui::widgets::Cell::from(cell_text)
                                    .style(Style::default().add_modifier(Modifier::BOLD))
                            })
                            .collect();

                        // Data rows (rest)
                        let mut rows = Vec::new();
                        for (idx, row) in self.coverage.summary_rows.iter().skip(1).enumerate() {
                            // Map data row index (0-based) to actual row index (1-based, since 0 is header)
                            let actual_row_index = idx + 1;
                            let is_selected = actual_row_index == self.coverage.summary_index;

                            let cells: Vec<ratatui::widgets::Cell> = row
                                .iter()
                                .enumerate()
                                .map(|(i, cell)| {
                                    let max_len = if Some(i) == file_col { 60 } else { 15 };
                                    let cell_text = if cell.len() > max_len {
                                        format!("{}...", &cell[..max_len - 3])
                                    } else {
                                        cell.clone()
                                    };
                                    let style = if is_selected {
                                        Style::default().bg(Color::Blue).fg(Color::White)
                                    } else {
                                        Style::default()
                                    };
                                    ratatui::widgets::Cell::from(cell_text).style(style)
                                })
                                .collect();

                            rows.push(Row::new(cells));
                        }

                        (Some(Row::new(header_cells)), rows)
                    } else {
                        (None, Vec::new())
                    };

                    // Show a window of rows that includes the selected row (row 0 = header, data from row 1)
                    let available_height = table_area.height.saturating_sub(2); // Account for header and borders
                    let selected_data_idx = self.coverage.summary_index.saturating_sub(1);
                    let start_idx = selected_data_idx
                        .saturating_sub(available_height.saturating_sub(1) as usize)
                        .min(
                            data_rows
                                .len()
                                .saturating_sub(available_height as usize)
                                .max(0),
                        );
                    let end_idx = (start_idx + available_height as usize).min(data_rows.len());
                    let visible_rows = if start_idx < data_rows.len() {
                        data_rows[start_idx..end_idx].to_vec()
                    } else {
                        Vec::new()
                    };

                    let mut table = Table::new(visible_rows, constraints).block(popup_block);

                    if let Some(header) = header_cells {
                        table = table.header(header);
                    }

                    // Render table
                    table.render(table_area, buf);
                } else if let Some(ref summary_data) = self.coverage.data {
                    // Fallback: show summary as text if parsing failed
                    let summary_lines: Vec<Line> = summary_data
                        .lines()
                        .take(20) // Limit to first 20 lines
                        .map(|line| Line::from(line.to_string()))
                        .collect();
                    let summary_text = Text::from(summary_lines);
                    Paragraph::new(summary_text)
                        .block(popup_block)
                        .wrap(ratatui::widgets::Wrap { trim: true })
                        .scroll((self.coverage.summary_index as u16, 0))
                        .render(table_area, buf);
                }
            } else {
                // Split coverage text into lines and handle word wrapping
                // We need to wrap long lines to fit within the popup width
                let available_width = popup_area.width.saturating_sub(2); // Account for borders
                let mut wrapped_lines = Vec::new();

                for line in coverage_text.lines() {
                    if line.len() as u16 <= available_width {
                        // Line fits, add as is
                        wrapped_lines.push(Line::from(line.to_string()));
                    } else {
                        // Line is too long, wrap it
                        let mut remaining = line;
                        while !remaining.is_empty() {
                            let chunk = if remaining.len() as u16 > available_width {
                                // Find a good break point (space or tab)
                                let max_chars = available_width as usize;
                                let mut break_point = max_chars;

                                // Try to break at a space or tab
                                if let Some(last_space) =
                                    remaining[..max_chars.min(remaining.len())].rfind([' ', '\t'])
                                {
                                    break_point = last_space + 1;
                                }

                                let (chunk, rest) =
                                    remaining.split_at(break_point.min(remaining.len()));
                                remaining = rest.trim_start();
                                chunk.to_string()
                            } else {
                                let result = remaining.to_string();
                                remaining = "";
                                result
                            };
                            wrapped_lines.push(Line::from(chunk));
                        }
                    }
                }

                let coverage_text_widget = Text::from(wrapped_lines);
                Paragraph::new(coverage_text_widget)
                    .block(popup_block)
                    .scroll((self.coverage.scroll_offset, 0))
                    .wrap(ratatui::widgets::Wrap { trim: true })
                    .render(popup_area, buf);
            }

            // Add instructions at the bottom (with multi-line support)
            // Use cyan/yellow for better visibility on dark gray background
            let instruction_spans = match self.coverage.mode {
                CoverageMode::FileList => {
                    vec![
                        " Enter ".into(),
                        " view file ".into(),
                        " | ".into(),
                        " Up/Down ".into(),
                        " navigate ".into(),
                        " | ".into(),
                        " Esc/B ".into(),
                        " back ".into(),
                        " | ".into(),
                        " Q ".into(),
                        " main view ".into(),
                    ]
                }
                CoverageMode::FileView => {
                    vec![
                        " Esc/B ".into(),
                        " back to list ".into(),
                        " | ".into(),
                        " Q ".into(),
                        " main view ".into(),
                        " | ".into(),
                        " Up/Down ".into(),
                        " scroll ".into(),
                    ]
                }
            };

            // Split instructions into multiple lines if needed
            let instruction_lines = App::wrap_instruction_line(instruction_spans, popup_area.width);
            let instruction_height = instruction_lines.len() as u16;

            let instruction_area = Rect {
                x: popup_area.x,
                y: popup_area.y + popup_area.height,
                width: popup_area.width,
                height: instruction_height,
            };

            let instruction_text = Text::from(instruction_lines);
            Paragraph::new(instruction_text).render(instruction_area, buf);
        }

        // Show task menu as overlay if active (rendered after coverage view)
        if self.task_menu_open && self.tabs.index > 0 {
            let scheduler_index = self.tabs.index - 1;
            if let Some(tasks) = self.tasks_cache.get(&scheduler_index) {
                if self.task_selection_index < tasks.len() {
                    // Create a popup area near the selected task - make it larger
                    let menu_width = 30;
                    let menu_height = self.task_menu_actions.len() as u16 + 4; // Add more padding
                    let popup_area = Rect {
                        x: content_area.x + content_area.width / 2 - menu_width / 2,
                        y: content_area.y + content_area.height / 2 - menu_height / 2,
                        width: menu_width,
                        height: menu_height,
                    };

                    // Create menu items
                    let mut menu_lines = vec![Line::from(vec!["Task Manager".bold()])];
                    menu_lines.push(Line::from(""));

                    for (idx, action) in self.task_menu_actions.iter().enumerate() {
                        let is_selected = idx == self.task_menu_selection;
                        let style = if is_selected {
                            Style::default().bg(Color::Blue).fg(Color::White)
                        } else {
                            Style::default()
                        };
                        menu_lines.push(Line::from(vec![
                            if is_selected {
                                "> ".into()
                            } else {
                                "  ".into()
                            },
                            Span::styled(action.clone(), style),
                        ]));
                    }

                    menu_lines.push(Line::from(""));
                    menu_lines.push(Line::from(vec![
                        "Press ".into(),
                        "Enter".blue().bold(),
                        " to select, ".into(),
                        "Esc".blue().bold(),
                        " to cancel".into(),
                    ]));

                    let menu_block = Block::bordered()
                        .title(" Actions ")
                        .border_set(border::THICK)
                        .style(Style::default().bg(Color::DarkGray).fg(Color::White));

                    let menu_text = Text::from(menu_lines);
                    Paragraph::new(menu_text)
                        .block(menu_block)
                        .render(popup_area, buf);
                }
            }
        }

        // Show config reload prompt as overlay if active (rendered last so it's on top)
        if self.config_reload_prompt {
            // First, create an overlay that masks the entire content area
            Clear.render(content_area, buf);

            // Fill the content area with a semi-transparent overlay
            for y in content_area.y..content_area.y + content_area.height {
                for x in content_area.x..content_area.x + content_area.width {
                    let cell = &mut buf[(x, y)];
                    cell.set_bg(Color::Black);
                    cell.set_fg(Color::White);
                    cell.set_char(' ');
                    // Add a dim modifier to make it look like an overlay
                    cell.set_style(Style::default().add_modifier(Modifier::DIM));
                }
            }

            // Create a centered popup area
            let popup_area = Rect {
                x: content_area.x + content_area.width / 4,
                y: content_area.y + content_area.height / 2 - 3,
                width: content_area.width / 2,
                height: 7,
            };

            let popup_block = Block::bordered()
                .title(" Reload Config ")
                .border_set(border::THICK)
                .style(Style::default().bg(Color::DarkGray).fg(Color::White));

            let popup_text = Text::from(vec![
                Line::from(""),
                Line::from("Do you want to reload the main config?"),
                Line::from(""),
                Line::from(vec![
                    " Press ".into(),
                    "Y".green().bold(),
                    " to confirm, ".into(),
                    "N".red().bold(),
                    " to cancel ".into(),
                ]),
            ]);

            Paragraph::new(popup_text)
                .block(popup_block)
                .centered()
                .render(popup_area, buf);
        }
    }
}

fn main() -> io::Result<()> {
    // Collect arguments to start all the processes
    let args: Vec<String> = std::env::args().collect();
    let config_path = if args.len() > 1 {
        PathBuf::from(&args[1])
    } else {
        eprintln!(
            "Error: missing config file argument. Please specify a config file path as the first argument."
        );
        std::process::exit(1);
    };

    let str_config_file = match std::fs::read_to_string(config_path.clone()) {
        Ok(contents) => contents,
        Err(e) => {
            eprintln!(
                "Failed to read config file {}: {}",
                config_path.display(),
                e
            );
            std::process::exit(1);
        }
    };
    let config_json = match serde_json::from_str::<launcher::Config>(&str_config_file) {
        Ok(config) => config,
        Err(e) => {
            eprintln!("Failed to parse config.json: {e}");
            std::process::exit(1);
        }
    };

    // Configure ftail to write to a file in logdir
    let log_file_path = format!("{}/fuzztui.log", config_json.logdir);
    // Ensure logdir exists
    if let Err(e) = std::fs::create_dir_all(&config_json.logdir) {
        eprintln!(
            "Failed to create log directory '{}': {}",
            config_json.logdir, e
        );
        std::process::exit(1);
    }

    if let Err(e) = Ftail::new()
        .single_file(
            std::path::Path::new(&log_file_path),
            true,
            LevelFilter::Info,
        )
        .init()
    {
        eprintln!("Failed to initialize logger: {e}");
        std::process::exit(1);
    }

    let mut terminal = ratatui::init();

    let mut launcher = Launcher::new(config_path, config_json);
    launcher.init();

    // Wrap launcher in Arc<RwLock<>> for thread-safe access
    // RwLock allows multiple readers (for display) while one writer (for update)
    let launcher = Arc::new(RwLock::new(launcher));

    let app_result = App::default(launcher).run(&mut terminal);

    ratatui::restore();
    app_result
}
