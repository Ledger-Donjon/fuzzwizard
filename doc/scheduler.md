<!-- SPDX-FileCopyrightText: © 2026 LEDGER SAS -->
<!-- SPDX-License-Identifier: MIT -->

# Scheduler Documentation

## Overview

The scheduler is a Rust library that manages the lifecycle of fuzzing tasks. It:
- Collects binaries to fuzz from directories
- Detects new binaries and launches them automatically
- Monitors running fuzzers
- Detects crashes and sends them to the backend
- Manages corpus, crashes, and coverage directories

## Architecture

```
┌──────────────────┐
│ Targets Collector│ ──► Scans directories for binaries
└────────┬─────────┘
         │
         ▼
┌──────────────────┐
│   Task Manager   │ ──► Lifecycle control
└────────┬─────────┘
         │
         ├──► ┌───────────────────┐
         │    │ Targets Directory │ ──► Binaries to fuzz
         │    └───────────────────┘
         │
         ├──► ┌───────────────────┐
         │    │ Corpus Directory  │ ──► Input corpus
         │    └───────────────────┘
         │
         ├──► ┌───────────────────┐
         │    │ Crashes Directory │ ──► Crash files
         │    └────────┬──────────┘
         │             │
         │             ▼
         │    ┌───────────────────┐
         │    │  Crash Monitor    │ ──► Detects crashes
         │    └────────┬──────────┘
         │             │
         │             ├──► ┌─────────────┐
         │             │    │ Backend API │ ──► POST /crashes
         │             │    └─────────────┘
         │             │
         │             └──► Crashes Directory
         │
         ├──► ┌───────────────────┐
         │    │Coverage Directory │ ──► Coverage data
         │    └───────────────────┘
         │
         └──► ┌────────────┐
              │ State File │ ──► JSON logging and control (RUNNING/CRASHED/STOPPED)
              └────────────┘
```

## Task Lifecycle

1. **Collection**: Scan directories for binaries
2. **Detection**: Compare with running fuzzers
3. **Launch**: Start new fuzzers, stop old ones
4. **Monitoring**: Monitor running fuzzers
5. **Crash Detection**: Detect crashes and send to backend
6. **Cleanup**: Stop fuzzers for deleted binaries

## Configuration

See [config.md](config.md) for configuration details.

## Features

### Corpus Management

Each fuzzer has its own corpus directory: `{corpus_directory}/{sha_hex}/`
- Corpus is reused when binary restarts
- Initialization file created if corpus is empty

### Crash Detection

Monitors `{crashes_directory}/` for new crash files:
- Reads crash report from stderr
- Reads input that caused crash
- Sends to backend via HTTP POST

### Container Support

Fuzzers can run in containers (Docker/Podman):
- Volumes mounted: binary, crash_dir, corpus_dir, coverage
- Container name: `docker-fw-{random_hex}-{fuzzer_name}`

### Coverage Collection

If `has_coverage` is true:
- Sends SIGUSR1 to fuzzers to trigger coverage
- Stores `.profraw` files in coverage directory using LLVM's `%m` online
  merging, so repeated live snapshots update bounded files instead of appending
  complete profiles indefinitely
- Project-wide coverage can be generated on demand by merging all task `.profraw` files into `{coverage_directory}/project.profdata` and running `llvm-cov` with every instrumented binary (multi-object)


## State File

See [state_file.md](state_file.md) for configuration details.


## See Also

- [state_file.md](state_file.md): State file format
- [backend.md](backend.md): Backend API
- [config.md](config.md): Configuration details
