// SPDX-FileCopyrightText: © 2026 LEDGER SAS
// SPDX-License-Identifier: MIT

//! Keyboard event handling, task menus, and task actions.

use crossterm::event::{KeyCode, KeyEvent};
use log::error;
use scheduler::TaskStatus;

use super::App;
use crate::launcher::ProcessStatus;
use super::types::CoverageMode;

impl App {
    pub(crate) fn handle_key_event(&mut self, key_event: KeyEvent) {
        // Handle config reload prompt
        if self.config_reload_prompt {
            match key_event.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    if let Ok(mut launcher) = self.launcher.write() {
                        launcher.reload_config();
                    }
                    self.config_reload_prompt = false;
                    self.update_launcher_cache();
                }
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    self.config_reload_prompt = false;
                }
                _ => {}
            }
            return;
        }

        // Handle coverage view
        if self.coverage.view_open {
            match key_event.code {
                KeyCode::Up => match self.coverage.mode {
                    CoverageMode::Loading => {}
                    CoverageMode::FileList => {
                        if self.coverage.summary_rows.len() > 1 && self.coverage.summary_index > 1 {
                            self.coverage.summary_index -= 1;
                        }
                    }
                    CoverageMode::FileView => {
                        if self.coverage.scroll_offset > 0 {
                            self.coverage.scroll_offset -= 1;
                        }
                    }
                },
                KeyCode::Down => match self.coverage.mode {
                    CoverageMode::Loading => {}
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
                KeyCode::Enter
                    if self.coverage.mode == CoverageMode::FileList
                        && self.coverage.summary_index > 0
                        && self.coverage.summary_index < self.coverage.summary_rows.len() =>
                {
                    self.load_coverage_file();
                }
                KeyCode::Char('q') => {
                    self.reset_coverage_view();
                }
                KeyCode::Esc | KeyCode::Char('b') => {
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
                KeyCode::Up if self.task_menu_selection > 0 => {
                    self.task_menu_selection -= 1;
                }
                KeyCode::Down
                    if self.task_menu_selection
                        < self.task_menu_actions.len().saturating_sub(1) =>
                {
                    self.task_menu_selection += 1;
                }
                KeyCode::Enter => {
                    self.execute_task_action();
                    self.task_menu_open = false;
                }
                KeyCode::Esc | KeyCode::Char('m') => {
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
                self.scroll_offset = 0;
                self.task_selection_index = 0;
                self.task_menu_open = false;
                self.reset_coverage_view();
            }
            KeyCode::Right => {
                self.tabs.next();
                self.scroll_offset = 0;
                self.task_selection_index = 0;
                self.task_menu_open = false;
                self.reset_coverage_view();
            }
            KeyCode::Up => {
                if self.tabs.index == 0 {
                    if self.scroll_offset > 0 {
                        self.scroll_offset -= 1;
                    }
                } else {
                    if self.task_selection_index > 0 {
                        self.task_selection_index -= 1;
                    }
                }
            }
            KeyCode::Down => {
                if self.tabs.index == 0 {
                    if self.scroll_offset < 10000 {
                        self.scroll_offset += 1;
                    }
                } else {
                    let scheduler_index = self.tabs.index - 1;
                    if let Some(tasks) = self.tasks_cache.get(&scheduler_index) {
                        if self.task_selection_index < tasks.len().saturating_sub(1) {
                            self.task_selection_index += 1;
                        }
                    }
                }
            }
            KeyCode::Char('m') if self.tabs.index > 0 => {
                let scheduler_index = self.tabs.index - 1;
                if scheduler_index < self.launcher_cache.schedulers.len() {
                    let scheduler_status = self.launcher_cache.schedulers[scheduler_index].status;
                    if !matches!(scheduler_status, ProcessStatus::Running) {
                        return;
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
            KeyCode::Char('d') if self.tabs.index > 0 => {
                let scheduler_index = self.tabs.index - 1;
                if let Some(task) = self.selected_task(scheduler_index) {
                    self.display_coverage(scheduler_index, Some(&task));
                }
            }
            KeyCode::Char('p') if self.tabs.index > 0 => {
                let scheduler_index = self.tabs.index - 1;
                self.display_coverage(scheduler_index, None);
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
        if self.tabs.index > 0 {
            self.task_menu_actions.push("DISPLAY COVERAGE".to_string());
            self.task_menu_actions
                .push("DISPLAY PROJECT COVERAGE".to_string());
        }
        self.task_menu_selection = 0;
        self.task_menu_open = true;
    }

    fn execute_task_action(&mut self) {
        if self.tabs.index == 0 {
            return;
        }

        let scheduler_index = self.tabs.index - 1;
        if self.task_menu_selection >= self.task_menu_actions.len() {
            return;
        }

        if scheduler_index >= self.launcher_cache.schedulers.len() {
            return;
        }
        let scheduler_status = self.launcher_cache.schedulers[scheduler_index].status;
        if !matches!(scheduler_status, ProcessStatus::Running) {
            return;
        }

        let action = self.task_menu_actions[self.task_menu_selection].clone();

        let sha = if let Some(tasks) = self.tasks_cache.get(&scheduler_index) {
            if self.task_selection_index >= tasks.len() {
                return;
            }
            tasks[self.task_selection_index].sha
        } else {
            return;
        };

        if action == "DISPLAY COVERAGE" {
            if let Some(task) = self.selected_task(scheduler_index) {
                self.display_coverage(scheduler_index, Some(&task));
            }
            return;
        }
        if action == "DISPLAY PROJECT COVERAGE" {
            self.display_coverage(scheduler_index, None);
            return;
        }

        if let Ok(mut launcher) = self.launcher.write() {
            if scheduler_index < launcher.scheduler_processes.len() {
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
}
