// SPDX-FileCopyrightText: © 2026 LEDGER SAS
// SPDX-License-Identifier: MIT

use anyhow::Result;
use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize, Clone)]
pub struct CrashReport {
    pub id: i32,
    pub report: Vec<u8>,
    pub input: Vec<u8>,
    pub filename: String,
    pub program: Vec<u8>,
    pub scheduler_name: String,
    pub created_at: NaiveDateTime,
}

#[derive(Deserialize, Serialize, Clone)]
pub struct AdminEvent {
    pub id: i32,
    pub event_type: String,
    pub message: String,
    pub metadata: Option<String>,
    pub created_at: NaiveDateTime,
    pub processed: bool,
}

/// Trait pour les fournisseurs de notification
pub trait NotificationProvider: Send + Sync {
    /// Envoie une notification pour un crash
    fn send_crash_notification(&self, report: &CrashReport) -> Result<()>;

    /// Send a notification for an administrative event
    fn send_admin_notification(&self, event: &AdminEvent) -> Result<()>;

    /// Check if the provider is enabled
    fn is_enabled(&self) -> bool;
}
