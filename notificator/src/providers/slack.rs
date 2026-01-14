// SPDX-FileCopyrightText: © 2026 LEDGER SAS
// SPDX-License-Identifier: MIT

use anyhow::{Context, Result};
use log::info;
use reqwest::header::{HeaderMap, AUTHORIZATION, CONTENT_TYPE};
use serde::Serialize;
use std::path::Path;

use crate::traits::{CrashReport, NotificationProvider};

#[derive(Serialize)]
struct Payload {
    text: String,
    channel: String,
}

pub struct SlackNotificationProvider {
    token: String,
    channel: String,
    enabled: bool,
}

impl SlackNotificationProvider {
    pub fn new(token: String, channel: String, enabled: bool) -> Self {
        Self {
            token,
            channel,
            enabled,
        }
    }

    /// Send a Slack message via the API
    fn send_slack_message(&self, text: &str) -> Result<()> {
        let mut headers = HeaderMap::new();
        headers.insert(
            CONTENT_TYPE,
            "application/json"
                .parse()
                .context("Failed to parse content type header")?,
        );
        headers.insert(
            AUTHORIZATION,
            format!("Bearer {}", self.token)
                .parse()
                .context("Failed to parse authorization header")?,
        );

        let client = reqwest::blocking::Client::new();
        let res = client
            .post("https://slack.com/api/chat.postMessage")
            .headers(headers)
            .json(&Payload {
                channel: self.channel.clone(),
                text: text.to_string(),
            })
            .send()
            .context("Failed to send Slack message")?;

        if !res.status().is_success() {
            anyhow::bail!("Slack API returned error: {}", res.status());
        }

        Ok(())
    }
}

impl NotificationProvider for SlackNotificationProvider {
    fn send_crash_notification(&self, report: &CrashReport) -> Result<()> {
        if !self.enabled {
            return Ok(()); // Silently ignored if disabled
        }

        info!("Sending Slack notification for crash: {}", report.id);

        let created_fmt = report.created_at.format("%Y-%m-%dT%H-%M-%S");
        let stem = Path::new(&report.filename)
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let dir = format!("/tmp/tmp_{}_{}", created_fmt, stem);

        let to_send = format!(
            "New crash for {} (id: {})\n\nTo retrieve artifacts locally:\n\n$ curl -X POST http://127.0.0.1:3000/collect_crashes -H 'Content-Type: application/json' -d '{}'\n\nFiles will be created under {}\n- {} (program binary)\n- crash_info.txt (report)\n- crash_input.txt (input)\n",
            &report.filename, report.id, report.id, dir, &report.filename
        );

        self.send_slack_message(&to_send)?;

        info!(
            "Slack notification sent successfully for crash {}",
            report.id
        );
        Ok(())
    }

    fn send_admin_notification(&self, event: &crate::traits::AdminEvent) -> Result<()> {
        if !self.enabled {
            return Ok(()); // Silently ignored if disabled
        }

        info!("Sending Slack notification for admin event: {}", event.id);

        let to_send = format!(
            "Administrative event: {}\n\n{}",
            event.event_type, event.message
        );

        self.send_slack_message(&to_send)?;

        info!(
            "Slack notification sent successfully for admin event {}",
            event.id
        );
        Ok(())
    }

    fn is_enabled(&self) -> bool {
        self.enabled
    }
}
