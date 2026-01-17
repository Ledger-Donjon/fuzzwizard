<!-- SPDX-FileCopyrightText: © 2026 LEDGER SAS -->
<!-- SPDX-License-Identifier: MIT -->

# Fuzzwizard Documentation

Welcome to the Fuzzwizard documentation, a 24/7 fuzzing system with automatic crash management and Slack alerts.

## Overview

Fuzzwizard is composed of several components that work together to provide a complete continuous fuzzing solution:

- **[fuzztui](fuzztui.md)** : TUI (Terminal User Interface) to monitor and manage the system
- **[backend](backend.md)** : REST API to store and manage crashes in a PostgreSQL database
- **[notificator](notificator.md)** : Notification service that sends alerts (Slack, file, etc.) when a new crash is detected
- **[scheduler](scheduler.md)** : Library to manage the lifecycle of fuzzing tasks

## Architecture

```
┌─────────────┐
│   fuzztui   │ ──► User interface (TUI)
└──────┬──────┘
       │
       │ monitors
       ▼
┌─────────────┐
│  launcher   │ ──► Manages processes (backend, notificator, schedulers)
└──────┬──────┘
       │
       ├──► ┌─────────────┐
       │    │   backend   │ ──► REST API + PostgreSQL (Port: 3000)
       │    └──────┬──────┘
       │           │
       │           │ stores
       │           ▼
       │    ┌─────────────┐
       │    │ crashes DB  │
       │    └─────────────┘
       │
       ├──► ┌─────────────┐
       │    │ notificator │ ──► Polls backend and sends notifications (Slack, file, etc.)
       │    └─────────────┘
       │
       └──► ┌─────────────┐
            │ schedulers  │ ──► Manage fuzzers and send crashes to backend
            └─────────────┘
```

### Communication protocols

- **HTTP REST** : Communication between schedulers and backend (`POST /crashes`, `GET /crashes`)
- **JSON files** : Shared state files to control fuzzers (`/tmp/{scheduler_name}_state.json`)
- **Slack API** : Alerts via `chat.postMessage` (via notificator)
- **mpsc channels** : Internal communication in fuzztui (threads)



## Detailed documentation

### Main documents

- **[install.md](install.md)** - Complete installation guide with dependencies and commands
- **[config.md](config.md)** - Complete documentation of configuration files 
- **[state_file.md](state_file.md)** - Documentation of the fuzzer state file allowing to stop/restart fuzzers

### Component documents

- [fuzztui.md](fuzztui.md) - TUI interface and launcher
- [backend.md](backend.md) - REST API and database
- [notificator.md](notificator.md) - Multi-provider notification service (Slack, file, etc.)
- [scheduler.md](scheduler.md) - Fuzzer management library
