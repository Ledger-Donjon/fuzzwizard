// SPDX-FileCopyrightText: © 2026 LEDGER SAS
// SPDX-License-Identifier: MIT

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Deserialize, Serialize, Clone)]
pub struct NotificationConfig {
    #[serde(rename = "notificator_state_file")]
    pub notificator_state_file: String,
    pub providers: ProvidersConfig,
}

#[derive(Deserialize, Serialize, Clone)]
pub struct ProvidersConfig {
    pub slack: SlackConfig,
    pub file: FileConfig,
}

#[derive(Deserialize, Serialize, Clone)]
pub struct SlackConfig {
    pub enabled: bool,
    pub token: String,
    pub channel: String,
}

#[derive(Deserialize, Serialize, Clone)]
pub struct FileConfig {
    pub enabled: bool,
    pub output_file: String,
}

#[derive(Deserialize, Serialize)]
pub struct RootConfig {
    pub schedulers: Vec<serde_json::Value>,
    pub database_url: String,
    pub logdir: String,
    #[serde(rename = "container_binary")]
    pub container_binary: Option<String>,
    pub notifications: NotificationConfig,
}

impl RootConfig {
    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        let contents = std::fs::read_to_string(path.as_ref())
            .with_context(|| format!("Failed to read config file: {}", path.as_ref().display()))?;
        let config: Self =
            serde_json::from_str(&contents).context("Failed to parse config file as JSON")?;
        Ok(config)
    }
}
