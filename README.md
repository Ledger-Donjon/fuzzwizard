# fuzzwizard

Scheduler and triager for 24/7 fuzzing

This repository is composed by several components:
- scheduler : it is a library which allow to collect target to fuzz and check if there is any updates in these targets to update them. When a crash is detected it is send to the backup.
A simple working example of a scheduler is provided in main.rs.
It could be configured with a config file which is expected as a parameter. An example is given in tests/test_targets.
It contains, in particular :
    - the directory where the taregts are expected
    - the location of the script file needed to build the target
- backend : a backend which allow to interact with the DB to upload and analyse crashes
- slackwizzard: component which sends alerts on slack when a new crash occured

Each of the previous component could be started independently
But a last component, the launcher has been created to start all the process in the same time and add additionnal monitoring the the components.
It could take a configuration file path in argument otherwise it will take the one included in fuzzwizzard.
In this configuration file you could set the scheduler and scheduler_config path in order to start fuzzwizzard using the scheduler library.

## Installation
Setup database
```
cd backend
cargo install diesel_cli --no-default-features --features postgres
docker compose up
export DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5432/fuzzwizard
diesel setup
```

## Environment variables

### backend
- DATABASE_URL: PostgreSQL connection string (e.g. `postgres://user:pass@localhost:5432/fuzzwizard`)

### slackwizard
- SLACK_TOKEN: Slack bot token (e.g. `xoxb-...`)
- SLACK_CHANNEL: Slack channel ID or name (e.g. `C0123456789` or `#alerts`)

### launcher
- SLACK_TOKEN, SLACK_CHANNEL must be set (see above)


# Run:
```bash
RUST_LOG=info cargo run -p launcher
```

What it does:
- Checks required env vars are set
- Prompts for a targets directory (used by the scheduler to watch and run targets)
- Launches backend and slackwizard
- Monitors processes, sends a Slack alert if a process exits
- Displays running scheduler-driven processes and their uptime
