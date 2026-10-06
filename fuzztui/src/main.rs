// SPDX-FileCopyrightText: © 2026 LEDGER SAS
// SPDX-License-Identifier: MIT

mod app;
mod launcher;

use std::io;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use ftail::Ftail;
use log::LevelFilter;

use app::App;
use launcher::Launcher;

fn main() -> io::Result<()> {
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

    let log_file_path = format!("{}/fuzztui.log", config_json.logdir);
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

    let launcher = Arc::new(RwLock::new(launcher));

    let app_result = App::default(launcher).run(&mut terminal);

    ratatui::restore();
    app_result
}
