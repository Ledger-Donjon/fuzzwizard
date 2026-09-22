// SPDX-FileCopyrightText: © 2026 LEDGER SAS
// SPDX-License-Identifier: MIT

//! `Widget` implementation for [`App`] and rendering helpers.

use std::time::{Duration, Instant, SystemTime};

use chrono::{DateTime, Local, Utc};
use humantime::format_duration;
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style, Stylize},
    symbols::border,
    text::{Line, Span, Text},
    widgets::{Block, Clear, Paragraph, Row, Table, Tabs, Widget},
};
use scheduler::{TaskStatus, COVERAGE_SOURCE_UNAVAILABLE_MARKER};

use super::App;
use crate::launcher::ProcessStatus;
use super::types::CoverageMode;

/// Human readable time elapsed since `instant`, rounded to the second.
pub(crate) fn elapsed_since(instant: Instant) -> String {
    format_duration(Duration::from_secs(instant.elapsed().as_secs())).to_string()
}

/// Split `line` into chunks of at most `width` characters, preferring to break
/// on whitespace.
///
/// Unlike `Wrap { trim: true }` this keeps leading whitespace, which carries the
/// nesting of annotated source. Chunk boundaries always fall on character
/// boundaries, since coverage output contains multi-byte characters.
pub(crate) fn wrap_line(line: &str, width: usize) -> Vec<&str> {
    if width == 0 {
        return vec![line];
    }

    let mut chunks = Vec::new();
    let mut start = 0;
    let mut chars_in_chunk = 0;
    let mut last_space: Option<(usize, usize)> = None;

    for (chars_seen, (index, c)) in line.char_indices().enumerate() {
        if chars_in_chunk == width {
            let (end, chars_before_end) = last_space.unwrap_or((index, chars_seen));
            chunks.push(&line[start..end]);
            start = end;
            chars_in_chunk = chars_seen - chars_before_end;
            last_space = None;
        }
        chars_in_chunk += 1;
        if c == ' ' || c == '\t' {
            last_space = Some((index + c.len_utf8(), chars_seen + 1));
        }
    }
    chunks.push(&line[start..]);

    chunks
}

/// Truncate `text` to at most `max_chars` characters, marking the cut with an
/// ellipsis.
///
/// Coverage tables hold source paths, which may contain multi-byte characters,
/// so the cut has to fall on a character boundary: slicing at a byte offset
/// panics as soon as it lands inside a character.
pub(crate) fn truncate_cell(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let kept: String = text.chars().take(max_chars.saturating_sub(3)).collect();
    format!("{kept}...")
}

impl App {
    /// Wrap instruction spans into multiple lines if they don't fit.
    pub(crate) fn wrap_instruction_line(spans: Vec<Span>, available_width: u16) -> Vec<Line> {
        if available_width < 20 {
            return vec![Line::from(spans)];
        }

        let total_text: String = spans
            .iter()
            .map(|span| format!("{span}"))
            .collect();

        if total_text.len() as u16 <= available_width {
            return vec![Line::from(spans)];
        }

        let spans_clone: Vec<Span> = spans.to_vec();

        let mut lines = Vec::new();
        let mut current_line = Vec::new();
        let mut current_length = 0u16;

        for span in spans_clone {
            let span_text = format!("{span}");
            let span_len = span_text.len() as u16;
            let separator_len = if current_length > 0 { 3 } else { 0 };

            if current_length > 0 && current_length + separator_len + span_len > available_width {
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

        if !current_line.is_empty() {
            lines.push(Line::from(current_line));
        }

        if lines.is_empty() {
            lines.push(Line::from(spans));
        }

        lines
    }
}

impl Widget for &App {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let vertical = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Min(0),
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
            let block = Block::bordered()
                .title(" Fuzzwizard TUI ")
                .border_set(border::THICK);
            block.render(vertical[0], buf);
        }

        let content_area = vertical[1];

        let instruction_spans: Vec<Span> = if self.tabs.index == 0 {
            vec![
                " Quit ".into(),
                "<Q> ".blue().bold(),
                " | Navigate ".into(),
                "<Left/Right> ".blue().bold(),
                " | Scroll ".into(),
                "<Up/Down> ".blue().bold(),
                " | Reload Config ".into(),
                "<C> ".blue().bold(),
            ]
        } else {
            vec![
                " Quit ".into(),
                "<Q> ".blue().bold(),
                " | Navigate ".into(),
                "<Left/Right> ".blue().bold(),
                " | Tasks ".into(),
                "<Up/Down> ".blue().bold(),
                " | Config ".into(),
                "<C> ".blue().bold(),
                " | Menu ".into(),
                "<M> ".blue().bold(),
                " | Coverage ".into(),
                "<D> ".blue().bold(),
                " | Project Cov ".into(),
                "<P> ".blue().bold(),
            ]
        };

        let border_inner_width = content_area.width.saturating_sub(2);
        let instruction_lines = App::wrap_instruction_line(instruction_spans, border_inner_width);
        let multiline_footer = instruction_lines.len() > 1;

        let footer_height = if multiline_footer {
            instruction_lines.len() as u16
        } else {
            0
        };

        let (main_content_area, footer_area, footer_lines, block) = if multiline_footer {
            let main_h = content_area.height.saturating_sub(footer_height);
            (
                Rect {
                    height: main_h,
                    ..content_area
                },
                Some(Rect {
                    x: content_area.x + 1,
                    y: content_area.y + main_h,
                    width: border_inner_width,
                    height: footer_height,
                }),
                Some(instruction_lines),
                Block::bordered().border_set(border::THICK),
            )
        } else {
            let single = instruction_lines.into_iter().next().unwrap_or_default();
            (
                content_area,
                None,
                None,
                Block::bordered()
                    .title_bottom(single.centered())
                    .border_set(border::THICK),
            )
        };

        if self.tabs.index == 0 {
            self.render_processes_tab(main_content_area, buf, block);
        } else {
            self.render_scheduler_tab(main_content_area, buf, block);
        }

        if let (Some(f_area), Some(f_lines)) = (footer_area, footer_lines) {
            let footer_text = Text::from(f_lines);
            Paragraph::new(footer_text)
                .alignment(ratatui::layout::Alignment::Center)
                .render(f_area, buf);
        }

        if self.coverage.view_open {
            self.render_coverage_overlay(content_area, buf);
        }

        if self.task_menu_open && self.tabs.index > 0 {
            self.render_task_menu(content_area, buf);
        }

        if self.config_reload_prompt {
            self.render_config_reload_prompt(content_area, buf);
        }
    }
}

impl App {
    fn render_processes_tab(&self, area: Rect, buf: &mut Buffer, block: Block) {
        use chrono::TimeDelta;

        let mut lines = vec![
            Line::from("Launcher Processes:".bold()),
            Line::from(""),
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

        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            "Config Status".bold(),
            " - ".into(),
            self.launcher_cache.config_status.clone().yellow(),
        ]));

        lines.push(Line::from(""));
        lines.push(Line::from("Schedulers:".bold()));
        lines.push(Line::from(""));

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

                let build_status = match (scheduler.building_since, scheduler.last_build_at) {
                    (Some(started), _) => {
                        format!("building for {}", elapsed_since(started)).yellow()
                    }
                    (None, Some(last)) => format!("{} ago", elapsed_since(last)).cyan(),
                    (None, None) => "never built".dim(),
                };

                lines.push(Line::from(vec!["    Last Build: ".into(), build_status]));
            }
        }

        lines.push(Line::from(""));
        lines.push(Line::from("Recent Crashes (Last 10):".bold()));
        lines.push(Line::from(""));

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
            .render(area, buf);
    }

    fn render_scheduler_tab(&self, area: Rect, buf: &mut Buffer, block: Block) {
        use chrono::TimeDelta;

        let scheduler_index = self.tabs.index - 1;
        if scheduler_index >= self.launcher_cache.schedulers.len() {
            let text = Text::from("No schedulers available");
            Paragraph::new(text)
                .block(block)
                .scroll((self.scroll_offset, 0))
                .render(area, buf);
            return;
        }

        let selected_scheduler = &self.launcher_cache.schedulers[scheduler_index];
        let is_scheduler_stopped = matches!(selected_scheduler.status, ProcessStatus::Stopped);

        let vertical_split = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(6),
                Constraint::Min(0),
            ])
            .split(area);

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

        header_lines.push(Line::from(""));
        header_lines.push(Line::from(vec![Span::styled(
            "Tasks:".to_string(),
            tasks_header_style,
        )]));

        let header_text = Text::from(header_lines);
        Paragraph::new(header_text)
            .block(Block::default())
            .render(vertical_split[0], buf);

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
                let mut rows: Vec<Row> = Vec::new();

                for (idx, task_info) in tasks.iter().enumerate() {
                    let status_text = match task_info.is_running {
                        TaskStatus::Running => "Running",
                        TaskStatus::Crashed => "Crashed",
                        TaskStatus::Stopped => "Stopped",
                    };

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

                    let is_selected = idx == self.task_selection_index;
                    let base_style = if is_scheduler_stopped {
                        if is_selected {
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
                        Constraint::Percentage(35),
                        Constraint::Percentage(25),
                        Constraint::Percentage(15),
                        Constraint::Percentage(25),
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
    }

    fn render_coverage_overlay(&self, content_area: Rect, buf: &mut Buffer) {
        Clear.render(content_area, buf);
        let overlay_bg = Color::Rgb(18, 18, 28);
        for y in content_area.y..content_area.y + content_area.height {
            for x in content_area.x..content_area.x + content_area.width {
                let cell = &mut buf[(x, y)];
                cell.set_bg(overlay_bg);
                cell.set_fg(Color::DarkGray);
                cell.set_char(' ');
            }
        }

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

        let coverage_label = if self.coverage.project_mode {
            "Project Coverage"
        } else {
            "Coverage"
        };

        let (title, coverage_text, show_file_list, degraded) = match self.coverage.mode {
            CoverageMode::Loading => {
                let title = format!(" {coverage_label} ");
                let text = "Loading coverage data...".to_string();
                (title, text, false, false)
            }
            CoverageMode::FileList => {
                let title = format!(
                    " {coverage_label} Summary ({}) ",
                    self.coverage.data_row_count()
                );
                let text = String::new();
                (title, text, true, false)
            }
            CoverageMode::FileView => {
                let file_name = self
                    .selected_file_path()
                    .and_then(|p| p.split('/').next_back().map(String::from))
                    .unwrap_or_else(|| "Unknown".to_string());
                let n = self.coverage.data_row_count();
                let text = if let Some(ref data) = self.coverage.file_data {
                    data.clone()
                } else {
                    "Loading file coverage...".to_string()
                };
                let degraded = text.starts_with(COVERAGE_SOURCE_UNAVAILABLE_MARKER);
                let suffix = if degraded {
                    " - source unavailable "
                } else {
                    " "
                };
                let title = format!(
                    " File: {} ({}/{}){}",
                    file_name,
                    self.coverage.summary_index,
                    n.max(1),
                    suffix
                );
                (title, text, false, degraded)
            }
        };

        let table_area = popup_area;

        let border_color = if degraded { Color::Yellow } else { Color::Cyan };
        let popup_block = Block::bordered()
            .title(title)
            .border_set(border::THICK)
            .border_style(Style::default().fg(border_color))
            .style(Style::default().bg(Color::DarkGray).fg(Color::White));

        if show_file_list {
            if !self.coverage.summary_rows.is_empty() {
                let num_cols = self.coverage.summary_rows[0].len();

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

                let (header_cells, data_rows) = if !self.coverage.summary_rows.is_empty() {
                    let header_row = &self.coverage.summary_rows[0];
                    let header_cells: Vec<ratatui::widgets::Cell> = header_row
                        .iter()
                        .enumerate()
                        .map(|(i, cell)| {
                            let max_len = if Some(i) == file_col { 60 } else { 15 };
                            let cell_text = truncate_cell(cell, max_len);
                            ratatui::widgets::Cell::from(cell_text)
                                .style(Style::default().add_modifier(Modifier::BOLD))
                        })
                        .collect();

                    let mut rows = Vec::new();
                    for (idx, row) in self.coverage.summary_rows.iter().skip(1).enumerate() {
                        let actual_row_index = idx + 1;
                        let is_selected = actual_row_index == self.coverage.summary_index;

                        let cells: Vec<ratatui::widgets::Cell> = row
                            .iter()
                            .enumerate()
                            .map(|(i, cell)| {
                                let max_len = if Some(i) == file_col { 60 } else { 15 };
                                let cell_text = truncate_cell(cell, max_len);
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

                let available_height = table_area.height.saturating_sub(2);
                let selected_data_idx = self.coverage.summary_index.saturating_sub(1);
                let start_idx = selected_data_idx
                    .saturating_sub(available_height.saturating_sub(1) as usize)
                    .min(data_rows.len().saturating_sub(available_height as usize));
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

                table.render(table_area, buf);
            } else if let Some(ref summary_data) = self.coverage.data {
                let summary_lines: Vec<Line> = summary_data
                    .lines()
                    .take(20)
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
            let available_width = popup_area.width.saturating_sub(2) as usize;
            let wrapped_lines: Vec<Line> = coverage_text
                .lines()
                .flat_map(|line| wrap_line(line, available_width))
                .map(|chunk| Line::from(chunk.to_string()))
                .collect();

            let coverage_text_widget = Text::from(wrapped_lines);
            Paragraph::new(coverage_text_widget)
                .block(popup_block)
                .scroll((self.coverage.scroll_offset, 0))
                .wrap(ratatui::widgets::Wrap { trim: false })
                .render(popup_area, buf);
        }

        let instruction_spans = match self.coverage.mode {
            CoverageMode::Loading => {
                vec![
                    " Q ".into(),
                    " main view ".into(),
                    " | ".into(),
                    " Esc ".into(),
                    " cancel ".into(),
                ]
            }
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

    fn render_task_menu(&self, content_area: Rect, buf: &mut Buffer) {
        let scheduler_index = self.tabs.index - 1;
        if let Some(tasks) = self.tasks_cache.get(&scheduler_index) {
            if self.task_selection_index < tasks.len() {
                let menu_width = 30;
                let menu_height = self.task_menu_actions.len() as u16 + 4;
                let popup_area = Rect {
                    x: content_area.x + content_area.width / 2 - menu_width / 2,
                    y: content_area.y + content_area.height / 2 - menu_height / 2,
                    width: menu_width,
                    height: menu_height,
                };

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

    fn render_config_reload_prompt(&self, content_area: Rect, buf: &mut Buffer) {
        Clear.render(content_area, buf);

        for y in content_area.y..content_area.y + content_area.height {
            for x in content_area.x..content_area.x + content_area.width {
                let cell = &mut buf[(x, y)];
                cell.set_bg(Color::Black);
                cell.set_fg(Color::White);
                cell.set_char(' ');
                cell.set_style(Style::default().add_modifier(Modifier::DIM));
            }
        }

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
