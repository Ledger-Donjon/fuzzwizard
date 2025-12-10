use chrono::{NaiveDateTime, TimeDelta};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind};
use std::io;
use std::path::PathBuf;
use std::time::{Duration, Instant};
mod launcher;
use ftail::Ftail;
use hex;
use humantime::format_duration;
use launcher::{Launcher, ProcessStatus, require_env};
use log::{LevelFilter, error, info};
use ratatui::{
    DefaultTerminal, Frame,
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect},
    style::Stylize,
    symbols::border,
    text::{Line, Text},
    widgets::{Block, Paragraph, Tabs, Widget},
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
}

#[derive(Debug, Clone)]
pub struct CrashInfo {
    pub id: i32,
    pub filename: String,
    pub scheduler_name: String,
    pub created_at: NaiveDateTime,
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

#[derive(Debug)]
pub struct App<'a> {
    tabs: TabsState,
    launcher: Option<&'a mut Launcher>,
    exit: bool,
    tasks_info: Vec<TaskInfo>,
    crashes: Vec<CrashInfo>,
    last_crashes_date: Option<NaiveDateTime>,
    last_update: Option<Instant>,
}

impl<'a> App<'a> {
    pub fn default(launcher: &'a mut Launcher, initial_date: Option<NaiveDateTime>) -> Self {
        let mut app = Self {
            tabs: TabsState::new(vec![]),
            launcher: Some(launcher),
            exit: false,
            tasks_info: Vec::new(),
            crashes: Vec::new(),
            last_crashes_date: initial_date,
            last_update: None,
        };
        app.sync_tabs_with_schedulers();
        app.update_tasks_info();
        app.update_crashes();
        app
    }
    pub fn run(&mut self, terminal: &mut DefaultTerminal) -> io::Result<()> {
        terminal.clear()?;
        while !self.exit {
            self.sync_tabs_with_schedulers();
            self.update_tasks_info();
            use std::time::{Duration, Instant};
            // Static to store last update time, only update every minute
            {
                if self.tabs.index == 0 {
                    self.launcher.as_mut().unwrap().update_proc_status();
                }

                let now = Instant::now();
                let mut should_update = false;

                match self.last_update {
                    Some(last) => {
                        if now.duration_since(last) >= Duration::from_secs(60) {
                            should_update = true;
                            self.last_update = Some(now);
                        }
                    }
                    None => {
                        should_update = true;
                        self.last_update = Some(now);
                    }
                }
                if should_update {
                    info!("Updating views");
                    self.update_crashes();
                    self.launcher.as_mut().unwrap().update_status();
                }
            }
            //self.launcher.as_mut().unwrap().collect_proc_info();
            //self.launcher.as_mut().unwrap().collect_scheduler();
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
        match key_event.code {
            KeyCode::Char('q') => self.exit(),
            KeyCode::Left => self.tabs.previous(),
            KeyCode::Right => self.tabs.next(),
            _ => {}
        }
    }
    fn sync_tabs_with_schedulers(&mut self) {
        if let Some(launcher) = &self.launcher {
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
        if let Some(launcher) = &mut self.launcher {
            // Only update tasks info if we're on a scheduler tab (index > 0)
            if self.tabs.index > 0 {
                let scheduler_index = self.tabs.index - 1; // Adjust for "Processes" tab at index 0
                if scheduler_index < launcher.scheduler_processes.len() {
                    let scheduler = &mut launcher.scheduler_processes[scheduler_index];
                    let tasks_with_status = scheduler.scheduler.get_tasks_with_status();
                    self.tasks_info = tasks_with_status
                        .into_iter()
                        .map(|(sha, path, is_running)| TaskInfo {
                            sha,
                            sha_hex: hex::encode(sha),
                            path,
                            is_running,
                        })
                        .collect();
                } else {
                    self.tasks_info.clear();
                }
            } else {
                // Clear tasks info when on Processes tab
                self.tasks_info.clear();
            }
        }
    }

    fn update_crashes(&mut self) {
        let client = reqwest::blocking::Client::new();

        // Build the URL - use last_crashes_date if available
        let mut url = Url::parse("http://127.0.0.1:3000/crashes").unwrap();

        if let Some(ref most_recent_date) = self.last_crashes_date {
            // Only get crashes newer than the last date we checked
            // The backend expects format: "%Y-%m-%d %H:%M:%S%.f"
            url.query_pairs_mut().append_pair(
                "after",
                &most_recent_date.format("%Y-%m-%d %H:%M:%S%.f").to_string(),
            );
        }
        info!("Url to collect crashes: {}", url.as_str());
        // If last_crashes_date is None, we'll get all crashes (but only keep the 10 most recent)

        match client.get(url.as_str()).send() {
            Ok(response) => {
                if response.status().is_success() {
                    match response.json::<Vec<serde_json::Value>>() {
                        Ok(crashes_json) => {
                            // Parse new crashes
                            let mut new_crashes: Vec<CrashInfo> = crashes_json
                                .into_iter()
                                .filter_map(|crash| {
                                    let id = crash.get("id")?.as_i64()? as i32;
                                    let filename = crash.get("filename")?.as_str()?.to_string();
                                    let scheduler_name =
                                        crash.get("scheduler_name")?.as_str()?.to_string();

                                    // Parse created_at - it might be a string or an object
                                    let created_at =
                                        if let Some(date_str) = crash.get("created_at")?.as_str() {
                                            date_str.to_string()
                                        } else {
                                            // Try to format from timestamp object
                                            format!("{:?}", crash.get("created_at"))
                                        };
                                    info!("created_at: {}", created_at);

                                    Some(CrashInfo {
                                        id,
                                        filename,
                                        scheduler_name,
                                        created_at: NaiveDateTime::parse_from_str(
                                            &created_at,
                                            "%Y-%m-%dT%H:%M:%S%.f",
                                        )
                                        .unwrap(),
                                    })
                                })
                                .collect();

                            if !new_crashes.is_empty() {
                                // Sort by created_at (most recent first)
                                new_crashes.sort_by(|a, b| b.created_at.cmp(&a.created_at));

                                // Add new crashes to existing ones
                                self.crashes.append(&mut new_crashes);

                                // Sort all crashes and keep only the 10 most recent
                                self.crashes.sort_by(|a, b| b.created_at.cmp(&a.created_at));
                                self.crashes.truncate(10);

                                // Update last_crashes_date to the most recent crash date
                                if let Some(most_recent) = self.crashes.first() {
                                    self.last_crashes_date = Some(most_recent.created_at);
                                }
                            }
                        }
                        Err(e) => {
                            // Failed to parse JSON, keep existing crashes
                            error!("Failed to parse crashes JSON: {:?}", e);
                        }
                    }
                }
            }
            Err(_) => {
                // Failed to connect to backend, keep existing crashes
                // Don't print error on every update to avoid spam
            }
        }
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
        if let Some(launcher) = &mut self.launcher {
            launcher.shutdown();
        }
        self.exit = true;
    }
}

impl<'a> Widget for &App<'a> {
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
        let instructions = Line::from(vec![
            " Quit ".into(),
            "<Q> ".blue().bold(),
            " | Navigate ".into(),
            "<Left/Right> ".blue().bold(),
        ]);

        let block = Block::bordered()
            .title_bottom(instructions.centered())
            .border_set(border::THICK);

        if let Some(launcher) = &self.launcher {
            // Check if we're on the Processes tab (index 0)
            if self.tabs.index == 0 {
                // Display processes information
                let mut lines = vec![
                    Line::from("Launcher Processes:".bold()),
                    Line::from(""), // Empty line separator
                ];

                if launcher.procs.is_empty() {
                    lines.push(Line::from("  No processes running".dim()));
                } else {
                    for proc in &launcher.procs {
                        let status_text = match proc.status {
                            ProcessStatus::Running => "Running".green(),
                            ProcessStatus::Stopped => "Stopped".red(),
                        };
                        let uptime_td =
                            TimeDelta::new(proc.started_at.elapsed().as_secs() as i64, 0)
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
                            format!("Uptime: {}", uptime_str).cyan(),
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
                    launcher.config_status.clone().yellow(),
                ]));

                // Add schedulers section
                lines.push(Line::from("")); // Empty line separator
                lines.push(Line::from("Schedulers:".bold()));
                lines.push(Line::from("")); // Empty line separator

                if launcher.scheduler_processes.is_empty() {
                    lines.push(Line::from("  No schedulers running".dim()));
                } else {
                    for scheduler in &launcher.scheduler_processes {
                        let status_text = match scheduler.status {
                            ProcessStatus::Running => "Running".green(),
                            ProcessStatus::Stopped => "Stopped".red(),
                        };
                        let uptime =
                            TimeDelta::new(scheduler.started_at.elapsed().as_secs() as i64, 0)
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
                            format!("Uptime: {}", uptime_str).cyan(),
                        ]));
                        lines.push(Line::from(vec![
                            "    Info: ".into(),
                            scheduler.info.clone().dim(),
                        ]));

                        let build_tds = if let Some(last_build_at) = scheduler.last_build_at {
                            let build_td =
                                TimeDelta::new(last_build_at.elapsed().as_secs() as i64, 0)
                                    .expect("Failed to create TimeDelta")
                                    .to_std()
                                    .unwrap();
                            format_duration(build_td).to_string()
                        } else {
                            "Never build".to_string()
                        };

                        lines.push(Line::from(vec![
                            "    Last Build: ".into(),
                            format!("{} ago", build_tds).cyan(),
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
                Paragraph::new(text).block(block).render(content_area, buf);
            } else {
                // Display scheduler information (index > 0)
                let scheduler_index = self.tabs.index - 1; // Adjust for "Processes" tab at index 0
                if scheduler_index < launcher.scheduler_processes.len() {
                    let selected_scheduler = &launcher.scheduler_processes[scheduler_index];

                    // Build the text content
                    let mut lines = vec![
                        Line::from(vec![
                            "Name: ".into(),
                            selected_scheduler.name.clone().yellow().bold(),
                        ]),
                        Line::from(vec![
                            "Status: ".into(),
                            selected_scheduler.status.to_string().cyan(),
                        ]),
                        Line::from(vec![
                            "Info: ".into(),
                            selected_scheduler.info.clone().into(),
                        ]),
                        Line::from(""), // Empty line separator
                        Line::from("Tasks:".bold()),
                    ];

                    // Add tasks information
                    if self.tasks_info.is_empty() {
                        lines.push(Line::from("  No tasks running".dim()));
                    } else {
                        for task_info in &self.tasks_info {
                            let status_text = match task_info.is_running {
                                TaskStatus::Running => "Running".green(),
                                TaskStatus::Crashed => "Crashed".red(),
                                TaskStatus::Stopped => "Stopped".red(),
                            };
                            lines.push(Line::from(vec![
                                "  • ".into(),
                                format!("0x{}", &task_info.sha_hex[..16]).cyan(), // Show first 16 chars
                                " - ".into(),
                                task_info
                                    .path
                                    .file_name()
                                    .and_then(|n| n.to_str())
                                    .unwrap_or("unknown")
                                    .to_string()
                                    .into(),
                                " - ".into(),
                                status_text,
                            ]));
                        }
                    }

                    let text = Text::from(lines);
                    Paragraph::new(text).block(block).render(content_area, buf);
                } else {
                    let text = Text::from("No schedulers available");
                    Paragraph::new(text).block(block).render(content_area, buf);
                }
            }
        } else {
            let text = Text::from("No launcher available");
            Paragraph::new(text).block(block).render(content_area, buf);
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

    let slack_token = require_env("SLACK_TOKEN");
    let slack_channel = require_env("SLACK_CHANNEL");

    let str_config_file = std::fs::read_to_string(config_path.clone()).expect(&format!(
        "Failed to read config file {}",
        config_path.display()
    ));
    let config_json = serde_json::from_str::<launcher::Config>(&str_config_file)
        .expect("Failed to parse config.json");

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

    Ftail::new()
        .single_file(
            std::path::Path::new(&log_file_path),
            true,
            LevelFilter::Info,
        )
        .init()
        .expect("Failed to initialize logger");

    // Initialize last_crashes_date with current date/time
    // Format: "%Y-%m-%d %H:%M:%S%.f" (e.g., "2025-01-15 14:30:45.123")
    //let initial_date = chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.f").to_string();

    let mut terminal = ratatui::init();

    let mut launcher = Launcher::new(slack_token, slack_channel, config_path, config_json);
    launcher.init();

    let app_result = App::default(&mut launcher, None).run(&mut terminal);

    ratatui::restore();
    app_result
}
