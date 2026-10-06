// SPDX-FileCopyrightText: © 2026 LEDGER SAS
// SPDX-License-Identifier: MIT

//! [`App`] struct, initialization, event loop, and cache synchronization.

mod coverage_ui;
mod input;
pub(crate) mod render;
pub(crate) mod types;

#[cfg(test)]
mod tests;

use std::io;
use std::sync::{mpsc, Arc, RwLock};
use std::thread;
use std::time::{Duration, Instant};

use chrono::NaiveDateTime;
use crossterm::event::{self, Event, KeyEventKind};
use log::{error, info};
use ratatui::{DefaultTerminal, Frame};
use reqwest::Url;
use scheduler::TaskStatus;

use crate::launcher::Launcher;
use types::*;

#[derive(Debug)]
pub(crate) struct App {
    pub tabs: TabsState,
    pub launcher: Arc<RwLock<Launcher>>,
    pub exit: bool,
    pub tasks_cache: std::collections::HashMap<usize, Vec<TaskInfo>>,
    pub crashes: Vec<CrashInfo>,
    pub last_crashes_date: Option<NaiveDateTime>,
    pub last_update: Option<Instant>,
    pub update_sender: Option<mpsc::Sender<Option<NaiveDateTime>>>,
    pub update_receiver: Option<mpsc::Receiver<UpdateMessage>>,
    pub pending_update: bool,
    pub launcher_update_sender: Option<mpsc::Sender<()>>,
    pub status_update_receiver: Option<mpsc::Receiver<()>>,
    pub launcher_cache: LauncherDisplayCache,
    pub scroll_offset: u16,
    pub config_reload_prompt: bool,
    pub task_selection_index: usize,
    pub task_menu_open: bool,
    pub task_menu_selection: usize,
    pub task_menu_actions: Vec<String>,
    pub coverage: CoverageViewState,
    pub coverage_receiver: mpsc::Receiver<CoverageResult>,
    pub coverage_sender: mpsc::Sender<CoverageResult>,
    pub coverage_request_id: u64,
}

impl App {
    pub fn default(launcher: Arc<RwLock<Launcher>>) -> Self {
        let (update_sender, update_receiver_main) = mpsc::channel();
        let (update_sender_main, update_receiver) = mpsc::channel();

        let (launcher_update_sender, launcher_update_receiver) = mpsc::channel();
        let (status_update_sender, status_update_receiver) = mpsc::channel();

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
                if let Err(e) = status_update_sender_for_thread.send(()) {
                    error!("Error sending status update completion notification: {e:?}");
                }
            }
        });

        let (coverage_sender, coverage_receiver) = mpsc::channel();

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
            launcher_cache: LauncherDisplayCache::empty(),
            scroll_offset: 0,
            config_reload_prompt: false,
            task_selection_index: 0,
            task_menu_open: false,
            task_menu_selection: 0,
            task_menu_actions: Vec::new(),
            coverage: CoverageViewState::empty(),
            coverage_receiver,
            coverage_sender,
            coverage_request_id: 0,
        };
        app.update_launcher_cache();
        app.sync_tabs_with_schedulers();
        app.update_tasks_info();
        app
    }

    pub(crate) fn parse_crash_json(crash: &serde_json::Value) -> Option<CrashInfo> {
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

    pub(crate) fn update_crashes_async(
        last_crashes_date: Option<NaiveDateTime>,
    ) -> Result<(Vec<CrashInfo>, Option<NaiveDateTime>), ()> {
        let client = reqwest::blocking::Client::new();
        info!("last_crashes_date: {last_crashes_date:?}");

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
                    new_crashes.sort_by_key(|c| std::cmp::Reverse(c.created_at));
                    let last_date = new_crashes.first().map(|c| c.created_at);
                    return Ok((new_crashes, last_date));
                } else {
                    return Ok((Vec::new(), None));
                }
            } else {
                return Ok((Vec::new(), None));
            }
        }

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
            new_crashes.sort_by_key(|c| std::cmp::Reverse(c.created_at));
            let new_date = new_crashes.first().map(|c| c.created_at).unwrap();
            Ok((new_crashes, Some(new_date)))
        } else {
            Ok((Vec::new(), last_crashes_date))
        }
    }

    pub(crate) fn update_launcher_cache(&mut self) {
        if let Ok(launcher) = self.launcher.try_read() {
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

            self.launcher_cache.config_status = launcher.config_status.clone();

            self.launcher_cache.schedulers = launcher
                .scheduler_processes
                .iter()
                .map(|s| SchedulerDisplayInfo {
                    name: s.name.clone(),
                    status: s.status,
                    started_at: s.started_at,
                    info: s.info.clone(),
                    last_build_at: s.last_build_at(),
                    building_since: s.building_since(),
                    state_file_path: s.config.state_file.clone(),
                })
                .collect();
        }
    }

    pub fn run(&mut self, terminal: &mut DefaultTerminal) -> io::Result<()> {
        terminal.clear()?;
        while !self.exit {
            self.update_launcher_cache();
            self.sync_tabs_with_schedulers();
            self.update_tasks_info();

            if let Some(ref receiver) = self.update_receiver {
                while let Ok(update_msg) = receiver.try_recv() {
                    self.crashes.append(&mut update_msg.crashes.clone());
                    self.crashes
                        .sort_by_key(|c| std::cmp::Reverse(c.created_at));
                    self.crashes.truncate(10);

                    if let Some(date) = update_msg.last_crashes_date {
                        self.last_crashes_date = Some(date);
                    }
                }
            }

            if let Some(ref receiver) = self.status_update_receiver {
                while receiver.try_recv().is_ok() {
                    self.pending_update = false;
                }
            }

            if self.tabs.index == 0 {
                if let Ok(mut launcher) = self.launcher.try_write() {
                    launcher.update_proc_status();
                }
            }

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
                if let Some(ref sender) = self.update_sender {
                    let _ = sender.send(self.last_crashes_date);
                    self.pending_update = true;
                }
                if let Some(ref sender) = self.launcher_update_sender {
                    let _ = sender.send(());
                }
            }

            while let Ok(result) = self.coverage_receiver.try_recv() {
                if !self.coverage.view_open || result.request_id != self.coverage_request_id {
                    continue;
                }
                match result.payload {
                    CoveragePayload::Summary(text) => {
                        let (rows, paths, file_col) = Self::parse_coverage_summary(&text);
                        self.coverage.summary_index = if rows.len() > 1 { 1 } else { 0 };
                        self.coverage.summary_rows = rows;
                        self.coverage.file_paths = paths;
                        self.coverage.file_col = file_col;
                        self.coverage.data = Some(text);
                        self.coverage.file_data = None;
                        self.coverage.mode = CoverageMode::FileList;
                    }
                    CoveragePayload::FileDetail(text) => {
                        self.coverage.file_data = Some(text);
                        self.coverage.mode = CoverageMode::FileView;
                        self.coverage.scroll_offset = 0;
                    }
                }
            }

            terminal.draw(|frame: &mut Frame<'_>| self.draw(frame))?;
            self.handle_events()?;
        }
        Ok(())
    }

    pub(crate) fn draw(&self, frame: &mut Frame) {
        frame.render_widget(self, frame.area());
    }

    pub(crate) fn handle_events(&mut self) -> io::Result<()> {
        if event::poll(Duration::from_millis(100))? {
            match event::read()? {
                Event::Key(key_event) if key_event.kind == KeyEventKind::Press => {
                    self.handle_key_event(key_event)
                }
                _ => {}
            }
        }
        Ok(())
    }

    pub(crate) fn sync_tabs_with_schedulers(&mut self) {
        if let Ok(launcher) = self.launcher.try_read() {
            let mut new_titles = vec!["Processes".to_string()];

            let scheduler_names: Vec<String> = launcher
                .scheduler_processes
                .iter()
                .map(|s| s.name.clone())
                .collect();
            new_titles.extend(scheduler_names);

            if new_titles != self.tabs.titles {
                let old_index = self.tabs.index;
                self.tabs.titles = new_titles;
                if !self.tabs.titles.is_empty() && old_index >= self.tabs.titles.len() {
                    self.tabs.index = self.tabs.titles.len() - 1;
                } else if self.tabs.titles.is_empty() {
                    self.tabs.index = 0;
                }
            }
        }
    }

    pub(crate) fn update_tasks_info(&mut self) {
        if let Ok(mut launcher) = self.launcher.try_write() {
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
    }

    pub(crate) fn exit(&mut self) {
        if let Ok(mut launcher) = self.launcher.write() {
            launcher.shutdown();
        }
        self.exit = true;
    }
}
