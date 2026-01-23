<!-- SPDX-FileCopyrightText: © 2026 LEDGER SAS -->
<!-- SPDX-License-Identifier: MIT -->

# Installation Guide

This guide covers the installation and setup of Fuzzwizard, including all dependencies and required commands.

## Prerequisites

### System Requirements

- **Operating System**: Linux or macOS
- **Rust**: Version 1.70 or later
- **PostgreSQL**: Version 12 or later (or Docker/Podman for containerized setup)
- **Docker or Podman** (optional): For containerized fuzzer execution

### Required Tools

1. **Build Tools** 
   - **Ubuntu/Debian**: 
     ```bash
     sudo apt install build-essential pkg-config libssl-dev libpq-dev
     ```
    
   - **macOS**: Xcode Command Line Tools are usually installed automatically with Homebrew or Rust
     - If needed: `xcode-select --install`
     - PostgreSQL development libraries are included when installing PostgreSQL via Homebrew

2. **Rust and Cargo**
   - Install from [rustup.rs](https://rust-lang.org/tools/install/)
   - Verify installation: `rustc --version` and `cargo --version`

3. **PostgreSQL**
   - **Option 1**: Install PostgreSQL locally
     ```bash
     # Ubuntu/Debian
     sudo apt install postgresql postgresql-contrib
     
     # macOS (Homebrew)
     brew install postgresql
     ```
   - **Option 2**: Use Docker Compose (recommended for development)
     - A `docker-compose.yml` file is provided in the `backend/` directory
     - See "Set Up the Database" section below for the command to start PostgreSQL

4. **Diesel CLI** (for database migrations)
   ```bash
   cargo install diesel_cli --no-default-features --features postgres
   ```


5. **Docker or Podman** (optional, for containerized fuzzing)
   - Docker: Install from [docker.com](https://www.docker.com/)
     - **Linux**: After installation, add your user to the `docker` group to avoid permission errors:
       ```bash
       sudo usermod -aG docker $USER
       # Log out and log back in for changes to take effect
       ```
   - Podman: Install from [podman.io](https://podman.io/)

## Installation Steps

### Part 1: Setup

#### 1. Clone the Repository

```bash
git clone <repository-url>
cd fuzzwizard
```

#### 2. Build the Project

```bash
cargo build --release
```

This will compile all components:
- `backend`: REST API server
- `fuzztui`: Terminal UI and launcher
- `notificator`: Notification service
- `scheduler`: Fuzzer scheduler library


#### 3. Set Up the Database

##### Option A: Using Docker Compose (Recommended for Development)

A `docker-compose.yml` file is provided in the `backend/` directory for easy PostgreSQL setup:

```bash
cd backend
docker-compose up -d
```

This starts a PostgreSQL container with:
- User: `postgres`
- Password: `postgres`
- Database: `fuzzwizard`
- Port: `5432` (bound to `127.0.0.1`)
- Image: `postgres:17.4-alpine`

The docker-compose file creates a persistent volume for the database data, so your data will persist across container restarts.

##### Option B: Using Local PostgreSQL

1. Create a database:
   ```bash
   sudo -u postgres createdb fuzzwizard
   ```

2. Set the connection string:
   ```bash
   export DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5432/fuzzwizard
   ```

#### 4. Run Database Migrations

```bash
# Set DATABASE_URL if not already set
export DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5432/fuzzwizard

# Run migrations
cd backend
diesel setup
```

This creates all necessary database tables:
- `crashes`: Stores crash reports
- `admin_events`: Stores administrative events

### Part 2: Configuration

This section covers creating the configuration files needed to run Fuzzwizard.

**Quick Start Option**: If you want to test Fuzzwizard quickly, you can use the test project which includes an automated setup script. See the [fuzzwizard_scheduler_test README](../../fuzzwizard_scheduler_test/README.md) for details. The test project will generate all necessary configuration files automatically.

#### 1. Create Configuration File

Create a `config.json` file (see [config.md](config.md) for details):

```json
{
  "schedulers": [
    {
      "name": "my_scheduler",
      "config": "/path/to/scheduler_config.json"
    }
  ],
  "database_url": "postgres://postgres:postgres@127.0.0.1:5432/fuzzwizard",
  "logdir": "./logs",
  "container_binary": null,
  "backend_binary": null,
  "notificator_binary": null,
  "notifications": {
    "notificator_state_file": "/tmp/notificator_state.txt",
    "providers": {
      "slack": {
        "enabled": false,
        "token": "",
        "channel": ""
      },
      "file": {
        "enabled": true,
        "output_file": "./logs/crashes.log"
      }
    }
  }
}
```

**Note**: All configuration (database URL, Slack tokens, etc.) is done via the `config.json` file. No environment variables are required for normal operation.

**Optional**: You can set `RUST_LOG` environment variable to control logging level:
```bash
export RUST_LOG=info  # or debug, warn, error
```

#### 2. Launch Fuzzwizard

```bash
cargo run -p fuzztui -- config.json
```

Or using the release build:

```bash
./target/release/fuzztui config.json
```

**Alternative**: If you're using the test project (`fuzzwizard_scheduler_test`), you can launch FuzzTUI directly with the generated configuration:

```bash
cd /path/to/fuzzwizard
cargo run -p fuzztui -- /path/to/fuzzwizard_scheduler_test/data/config/config.json
```
