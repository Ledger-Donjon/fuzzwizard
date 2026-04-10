<!-- SPDX-FileCopyrightText: © 2026 LEDGER SAS -->
<!-- SPDX-License-Identifier: MIT -->

# fuzzwizard

Scheduler and triager for 24/7 fuzzing.

## Overview

Fuzzwizard is a continuous fuzzing system that automatically manages fuzzing tasks, detects crashes, and sends notifications. It consists of:

- **scheduler**: Library to collect and manage fuzzing targets, detect updates, and send crashes to the backend
- **backend**: REST API to store and analyze crashes in PostgreSQL
- **notificator**: Sends alerts (Slack, file, etc.) when new crashes are detected
- **fuzztui**: Terminal UI to monitor and manage all components

## Quick Start

### Prerequisites

- Rust (latest stable)
- PostgreSQL
- Docker/Podman (for database setup)

### Installation

1. Setup the database:
```bash
cd backend
cargo install diesel_cli --no-default-features --features postgres
docker compose up
export DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5432/fuzzwizard
diesel setup
```

2. Run the launcher:
```bash
RUST_LOG=info cargo run -p fuzztui -- config.json
```

The launcher will start all components (backend, notificator, schedulers) and provide a TUI interface for monitoring.

See [doc/install.md](doc/install.md) for complete installation documentation.

## Configuration

See [doc/config.md](doc/config.md) for complete configuration documentation.

## Environment Variables

- `DATABASE_URL`: PostgreSQL connection string (e.g. `postgres://user:pass@localhost:5432/fuzzwizard`)

## Documentation

Complete documentation is available in the [doc/](doc/) directory:

- [Installation Guide](doc/install.md)
- [Configuration](doc/config.md)
- [FuzzTUI](doc/fuzztui.md)
- [Backend API](doc/backend.md)
- [Scheduler](doc/scheduler.md)
- [Notificator](doc/notificator.md)

## License

Licensed under the MIT License (see [`LICENSE`](./LICENSE)). © 2026 LEDGER SAS and contributors.

The project follows the [REUSE 3.3](https://reuse.software/) specification; every file carries an `SPDX-License-Identifier` header.

## Third-party components

Fuzzwizard depends on third-party open-source components listed (with their respective licences) in [`doc/THIRD_PARTY_LICENSES.html`](./doc/THIRD_PARTY_LICENSES.html), automatically generated from `Cargo.lock` via `cargo-about`. An SPDX SBOM is also published with each release.
