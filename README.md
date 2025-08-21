# fuzzwizard

Scheduler and triager for 24/7 fuzzing

## Environment variables

### backend
- DATABASE_URL: PostgreSQL connection string (e.g. `postgres://user:pass@localhost:5432/fuzzwizard`)

### slackwizard
- SLACK_TOKEN: Slack bot token (e.g. `xoxb-...`)
- SLACK_CHANNEL: Slack channel ID or name (e.g. `C0123456789` or `#alerts`)

### launcher
- DATABASE_URL, SLACK_TOKEN, SLACK_CHANNEL must be set (see above)

Run:
```bash
cargo run -p launcher
```

What it does:
- Checks required env vars are set
- Prompts for a targets directory (used by the scheduler to watch and run targets)
- Launches backend and slackwizard
- Monitors processes, sends a Slack alert if a process exits
- Displays running scheduler-driven processes and their uptime
