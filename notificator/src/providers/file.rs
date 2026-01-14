// SPDX-FileCopyrightText: © 2026 LEDGER SAS
// SPDX-License-Identifier: MIT

use anyhow::{Context, Result};
use log::info;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;

use crate::traits::{CrashReport, NotificationProvider};

pub struct FileNotificationProvider {
    output_file: String,
    enabled: bool,
}

impl FileNotificationProvider {
    pub fn new(output_file: String, enabled: bool) -> Self {
        Self {
            output_file,
            enabled,
        }
    }
}

impl NotificationProvider for FileNotificationProvider {
    fn send_crash_notification(&self, report: &CrashReport) -> Result<()> {
        if !self.enabled {
            return Ok(()); // Silently ignored if disabled
        }

        info!(
            "Writing crash notification to file for crash: {}",
            report.id
        );

        // Create parent directory if necessary
        if let Some(parent) = Path::new(&self.output_file).parent() {
            std::fs::create_dir_all(parent)
                .context("Failed to create parent directory for output file")?;
        }

        // Open file in append mode
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.output_file)
            .with_context(|| format!("Failed to open output file: {}", self.output_file))?;

        // Format crash information
        let created_fmt = report.created_at.format("%Y-%m-%d %H:%M:%S%.f");
        let crash_info = format!(
            "=== Crash Report ===\n\
            ID: {}\n\
            Filename: {}\n\
            Created at: {}\n\
            Report size: {} bytes\n\
            Input size: {} bytes\n\
            Program size: {} bytes\n\
            ---\n\n",
            report.id,
            report.filename,
            created_fmt,
            report.report.len(),
            report.input.len(),
            report.program.len()
        );

        file.write_all(crash_info.as_bytes())
            .with_context(|| format!("Failed to write to output file: {}", self.output_file))?;

        info!(
            "Crash notification written to file successfully for crash {}",
            report.id
        );
        Ok(())
    }

    fn send_admin_notification(&self, event: &crate::traits::AdminEvent) -> Result<()> {
        if !self.enabled {
            return Ok(()); // Silently ignored if disabled
        }

        info!(
            "Writing admin event notification to file for event: {}",
            event.id
        );

        // Create parent directory if necessary
        if let Some(parent) = Path::new(&self.output_file).parent() {
            std::fs::create_dir_all(parent)
                .context("Failed to create parent directory for output file")?;
        }

        // Open file in append mode
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.output_file)
            .with_context(|| format!("Failed to open output file: {}", self.output_file))?;

        // Format admin event information
        let created_fmt = event.created_at.format("%Y-%m-%d %H:%M:%S%.f");
        let event_info = format!(
            "=== Admin Event ===\n\
            ID: {}\n\
            Type: {}\n\
            Message: {}\n\
            Created at: {}\n\
            ---\n\n",
            event.id, event.event_type, event.message, created_fmt
        );

        file.write_all(event_info.as_bytes())
            .with_context(|| format!("Failed to write to output file: {}", self.output_file))?;

        info!(
            "Admin event notification written to file successfully for event {}",
            event.id
        );
        Ok(())
    }

    fn is_enabled(&self) -> bool {
        self.enabled
    }
}
