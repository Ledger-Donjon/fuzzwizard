<!-- SPDX-FileCopyrightText: © 2026 LEDGER SAS -->
<!-- SPDX-License-Identifier: MIT -->

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
- notificator: component which sends alerts (Slack, file, etc.) when a new crash occurred

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


# Run:
```bash
RUST_LOG=info cargo run -p launcher
```

What it does:
- Checks required env vars are set
- Prompts for a targets directory (used by the scheduler to watch and run targets)
- Launches backend and notificator
- Monitors processes, sends a Slack alert if a process exits
- Displays running scheduler-driven processes and their uptime


Questions pour demain: 
* pour le coverage; est ce qu'on l'intégre directement dans les projets ou est ce qu'on l'integre dans le patch 
* Gérer le rapport de coverage
* Docker de debug ? 
* Améliorer dossier de config
* Ajouter un log quand on lance un nouveau fuzzer avec son hash
* Ajouter un scheduler pour le V

DONE: 
* multiple scheduler or multiple directory avec un level de priorité
* slack configuration
* Add random init file in corpus directory 
* Faire un patch avec le token en $ENV
* Regler le probleme de CORPUS

## License

This project is licensed under the MIT License. See the [LICENSE](LICENSE) file for details.

All source files in this repository include SPDX license identifiers in accordance with the [REUSE Specification](https://reuse.software/spec/). Each file contains:
- `SPDX-FileCopyrightText`: Copyright information
- `SPDX-License-Identifier`: MIT license identifier

## Third-Party Open Source Software

This project uses several open source dependencies. The main dependencies and their licenses are:

### Rust Dependencies

The project uses Rust crates managed through Cargo. You can view all dependencies and their licenses by running:

```bash
cargo license
```

Key dependencies include:
- **axum**: Web framework (MIT)
- **diesel**: SQL ORM (MIT/Apache-2.0)
- **tokio**: Async runtime (MIT)
- **serde**: Serialization framework (MIT/Apache-2.0)
- **ratatui**: Terminal UI library (MIT)
- **chrono**: Date and time library (MIT/Apache-2.0)

For a complete list of all dependencies and their licenses, please refer to the `Cargo.toml` files in each component directory or run `cargo license` in the project root.

All third-party dependencies are used in accordance with their respective licenses. We are committed to respecting open source licenses and maintaining compliance with all applicable license terms.


