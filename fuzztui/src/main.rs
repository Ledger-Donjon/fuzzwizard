use chrono::{Local, NaiveDateTime, TimeDelta};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind};
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, RwLock, mpsc};
use std::thread;
use std::time::{Duration, Instant};
mod launcher;
use ftail::Ftail;
use humantime::format_duration;
use launcher::{Launcher, ProcessStatus, require_env};
use log::{LevelFilter, info};
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
}

#[derive(Debug, Clone)]
struct LauncherDisplayCache {
    procs: Vec<ProcessDisplayInfo>,
    config_status: String,
    schedulers: Vec<SchedulerDisplayInfo>,
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
    tasks_info: Vec<TaskInfo>,
    crashes: Vec<CrashInfo>,
    last_crashes_date: Option<NaiveDateTime>,
    last_update: Option<Instant>,
    update_sender: Option<mpsc::Sender<Option<NaiveDateTime>>>,
    update_receiver: Option<mpsc::Receiver<UpdateMessage>>,
    pending_update: bool,
    status_update_sender: Option<mpsc::Sender<()>>,
    status_update_receiver: Option<mpsc::Receiver<()>>,
    launcher_cache: LauncherDisplayCache,
}

impl App {
    pub fn default(launcher: Arc<RwLock<Launcher>>, initial_date: Option<NaiveDateTime>) -> Self {
        // Create channels for async crash updates
        let (update_sender, update_receiver_main) = mpsc::channel();
        let (update_sender_main, update_receiver) = mpsc::channel();

        // Create channel for launcher status update completion
        let (status_update_sender, status_update_receiver) = mpsc::channel();

        // Spawn background thread for crash updates only
        thread::spawn(move || {
            loop {
                // Wait for update request with last_crashes_date
                match update_receiver_main.recv() {
                    Ok(Some(last_date)) => {
                        info!("Background thread: Updating crashes");
                        let crashes_result = Self::update_crashes_async(Some(last_date));

                        if let Ok((crashes, new_last_date)) = crashes_result {
                            let _ = update_sender_main.send(UpdateMessage {
                                crashes,
                                last_crashes_date: new_last_date,
                            });
                        }
                    }
                    Ok(None) => {
                        info!("Background thread: Updating crashes (initial)");
                        let crashes_result = Self::update_crashes_async(None);

                        if let Ok((crashes, last_date)) = crashes_result {
                            let _ = update_sender_main.send(UpdateMessage {
                                crashes,
                                last_crashes_date: last_date,
                            });
                        }
                    }
                    Err(_) => {
                        break; // Channel closed, exit thread
                    }
                }
            }
        });

        let mut app = Self {
            tabs: TabsState::new(vec![]),
            launcher: launcher.clone(),
            exit: false,
            tasks_info: Vec::new(),
            crashes: Vec::new(),
            last_crashes_date: initial_date,
            last_update: None,
            update_sender: Some(update_sender),
            update_receiver: Some(update_receiver),
            pending_update: false,
            status_update_sender: Some(status_update_sender),
            status_update_receiver: Some(status_update_receiver),
            launcher_cache: LauncherDisplayCache {
                procs: Vec::new(),
                config_status: String::new(),
                schedulers: Vec::new(),
            },
        };
        // Initialize cache from launcher
        app.update_launcher_cache();
        app.sync_tabs_with_schedulers();
        app.update_tasks_info();
        // Initial synchronous update
        app
    }

    fn update_crashes_async(
        last_crashes_date: Option<NaiveDateTime>,
    ) -> Result<(Vec<CrashInfo>, Option<NaiveDateTime>), ()> {
        let client = reqwest::blocking::Client::new();
        let mut url = Url::parse("http://127.0.0.1:3000/crashes").map_err(|_| ())?;
        let mut new_date = Local::now().naive_local();

        if let Some(ref most_recent_date) = last_crashes_date {
            url.query_pairs_mut().append_pair(
                "after",
                &most_recent_date.format("%Y-%m-%d %H:%M:%S%.f").to_string(),
            );
        } else {
            url.query_pairs_mut().append_pair(
                "after",
                &new_date.format("%Y-%m-%d %H:%M:%S%.f").to_string(),
            );
            info!("now: {}", new_date.format("%Y-%m-%d %H:%M:%S%.f"));
        }
        let response = client.get(url.as_str()).send().map_err(|_| ())?;

        if response.status().is_success() {
            let crashes_json: Vec<serde_json::Value> = response.json().map_err(|_| ())?;
            if crashes_json.is_empty() {
                info!("no crashes found");
                return Ok((Vec::new(), Some(new_date)));
            }

            let mut new_crashes: Vec<CrashInfo> = crashes_json
                .into_iter()
                .filter_map(|crash| {
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
                        created_at: NaiveDateTime::parse_from_str(
                            &created_at,
                            "%Y-%m-%dT%H:%M:%S%.f",
                        )
                        .ok()?,
                    })
                })
                .collect();

            if !new_crashes.is_empty() {
                new_crashes.sort_by(|a, b| b.created_at.cmp(&a.created_at));
                new_date = new_crashes.first().map(|c| c.created_at).unwrap();
                Ok((new_crashes, Some(new_date)))
            } else {
                Ok((Vec::new(), Some(new_date)))
            }
        } else {
            Ok((Vec::new(), Some(new_date)))
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
            self.update_tasks_info();

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
                    self.pending_update = false;
                }
            }

            // Check for launcher status update completion (non-blocking)
            if let Some(ref receiver) = self.status_update_receiver {
                while receiver.try_recv().is_ok() {
                    self.pending_update = false;
                }
            }

            // Update proc status (fast, non-blocking)
            {
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
                    // Trigger background crash update (non-blocking)
                    if let Some(ref sender) = self.update_sender {
                        let _ = sender.send(self.last_crashes_date);
                        self.pending_update = true;
                    }

                    // Update launcher status asynchronously using a background thread
                    // Thread-safe: We use Arc<RwLock<Launcher>> which is Send and Sync
                    // RwLock allows multiple readers (for display) while one writer (for update)
                    let launcher_clone = Arc::clone(&self.launcher);
                    if let Some(ref sender) = self.status_update_sender {
                        let sender_clone = sender.clone();
                        thread::spawn(move || {
                            if let Ok(mut launcher) = launcher_clone.write() {
                                launcher.update_status();
                            }
                            let _ = sender_clone.send(());
                        });
                    }
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
        match key_event.code {
            KeyCode::Char('q') => self.exit(),
            KeyCode::Left => self.tabs.previous(),
            KeyCode::Right => self.tabs.next(),
            _ => {}
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
        // If update_status() is running, we'll keep the previous tasks_info
        if let Ok(mut launcher) = self.launcher.try_write() {
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
        let instructions = Line::from(vec![
            " Quit ".into(),
            "<Q> ".blue().bold(),
            " | Navigate ".into(),
            "<Left/Right> ".blue().bold(),
        ]);

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
                        format!("Uptime: {}", uptime_str).cyan(),
                    ]));
                    lines.push(Line::from(vec![
                        "    Info: ".into(),
                        scheduler.info.clone().dim(),
                    ]));

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
            if scheduler_index < self.launcher_cache.schedulers.len() {
                let selected_scheduler = &self.launcher_cache.schedulers[scheduler_index];

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

    let str_config_file = std::fs::read_to_string(config_path.clone())
        .unwrap_or_else(|_| panic!("Failed to read config file {}", config_path.display()));
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

    // Wrap launcher in Arc<RwLock<>> for thread-safe access
    // RwLock allows multiple readers (for display) while one writer (for update)
    let launcher = Arc::new(RwLock::new(launcher));

    let app_result = App::default(launcher, None).run(&mut terminal);

    ratatui::restore();
    app_result
}
