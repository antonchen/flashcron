# FlashCron

<div align="center">

**A Rust cron daemon for Linux containers**

[![CI](https://github.com/antonchen/flashcron/workflows/CI/badge.svg)](https://github.com/antonchen/flashcron/actions)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.89%2B-orange.svg)](https://www.rust-lang.org)

[简体中文](README.md) | [Features](#features) | [Installation](#installation) | [Quick Start](#quick-start) | [Documentation](docs/) | [Contributing](#contributing)

</div>

---

## Features

### Core Functionality
- **Standard Cron Expressions**: Full 5-field cron syntax support
- **Timezone Support**: Run jobs in any timezone, with flexible priority logic
- **TOML Configuration**: Human-readable, version-control friendly
- **Hot Reload**: Automatic config reload on file changes
- **Graceful Shutdown**: Waits for running jobs up to `shutdown_timeout`

### Job Management
- **Web Job Management**: Create and edit jobs in the Dashboard; saves apply immediately.
- **Standalone Scripts**: Edit scripts in a persistent directory and select their paths to fill the command field.
- **Dependency Management**: Add or remove APT/PIP dependencies, configure the Debian APT mirror and PIP index, and restore the SQLite manifest at container startup.
- **Timeout Handling**: Kill runaway jobs automatically
- **Retry Support**: Configurable retry count and delay
- **Environment Variables**: Per-job environment injection
- **Working Directory**: Per-job working directory
- **Concurrent Limits**: Prevent resource exhaustion

### Observability
- **Web Dashboard**: Built-in HTTP interface to monitor job status and history
- **Structured Logging**: Human-readable or JSON format
- **Execution History**: Track job success/failure rates

### Linux Support
- **Linux only**: Release binaries for x86_64 GNU, x86_64 musl, and aarch64 GNU

---

## Installation

### Docker (Recommended)

```bash
docker pull ghcr.io/antonchen/flashcron:latest
```

### Prebuilt Binaries

Download the pre-compiled binary for your platform from the [GitHub Releases](https://github.com/antonchen/flashcron/releases) page.

Each supported Linux target has three binaries: the unsuffixed asset (for example, `flashcron-linux-x86_64.tar.gz`) is the core daemon; the `-web` variant includes the dashboard and API; the `-dep-manager` variant adds dependency management and startup restore.

### From Cargo

```bash
cargo install flashcron
```

### From Source

```bash
git clone https://github.com/antonchen/flashcron
cd flashcron
cargo build --release
```

---

## Quick Start

### 1. Generate Configuration

```bash
flashcron init -o flashcron.toml
```

### 2. Edit Configuration

```toml
[settings]
log_level = "info"
max_concurrent_jobs = 10
sql_file = "flashcron.db"
script_dir = "scripts"
```

The default build enables `web`: create and edit jobs in the Dashboard. TOML `[jobs.*]` sections are ignored with a warning in the logs. To manage jobs through TOML, build with `--no-default-features`; the core profile keeps TOML job loading and hot reload.

Configuration accepts only fields defined by the selected feature profile. Removed or unsupported options fail validation instead of being silently ignored.

### 3. Validate Configuration

```bash
flashcron validate -c flashcron.toml
```

### 4. Start the Daemon

```bash
flashcron run -c flashcron.toml
```

Foreground mode is the default. For background daemon mode:

```bash
flashcron run -c flashcron.toml --daemon
```

---

## Timezone Support

FlashCron supports IANA timezones, resolved in this order:
1. `TZ` Environment Variable
2. `settings.timezone` in TOML
3. System Timezone
4. UTC (Fallback)

```toml
[settings]
timezone = "Asia/Shanghai" # Or "System", "UTC"
```

---

## CLI Reference

| Command | Description |
|---------|-------------|
| `flashcron run -c config.toml` | Start the daemon in foreground |
| `flashcron run -c config.toml --daemon` | Start the daemon in background |
| `flashcron validate -c config.toml` | Validate configuration |
| `flashcron list -c config.toml` | List all jobs |
| `flashcron schedule -c config.toml` | Show upcoming runs |
| `flashcron trigger <job>` | Trigger a job through local IPC |
| `flashcron status` | Show daemon status through local IPC |
| `flashcron stop` | Request graceful daemon shutdown through local IPC |
| `flashcron history` | Show execution history through local IPC |
| `flashcron init -o config.toml` | Generate default config |

### Options

| Option | Description |
|--------|-------------|
| `-c, --config <PATH>` | Configuration file path |
| `-l, --log-level <LEVEL>` | Log level (trace, debug, info, warn, error) |
| `--json` | Output logs in JSON format |
| `--daemon` | Run in background daemon mode |

### Local Control Channel

`trigger`, `status`, `stop`, and `history` all use a local Unix socket to talk to the daemon, so they work in both default foreground mode and `--daemon` background mode.

---

## Configuration Reference

### Global Settings

`api_host`, `api_port`, `api_token`, `script_dir`, `job_history_size`, and `max_history_size` are available only with the `web` feature; omit them from core configuration.

```toml
[settings]
working_dir = "/app"        # Optional default working directory for all jobs
log_level = "info"           # trace, debug, info, warn, error
json_logs = false            # JSON format for log aggregators
# Web only
api_host = "0.0.0.0"       # Web Dashboard / API host
api_port = 8080              # Web Dashboard / API port
api_token = "secret"         # API Auth Token (random UUID is persisted if not set)
# failure_hook = "/usr/local/bin/flashcron-failure-hook"  # Called after final failure; JSON is sent on stdin
sql_file = "flashcron.db"    # SQLite database file path
script_dir = "scripts"       # Web script directory; managed files use mode 0755
max_concurrent_jobs = 10     # 0 = unlimited
shell = "/bin/sh"            # Default shell
job_history_size = 100         # Max history entries to keep per job
max_history_size = 10000       # Max total history entries to keep globally, 0 = disable execution history
timezone = "System"          # Timezone (e.g., "System", "UTC", "Asia/Shanghai")
shutdown_timeout = 30   # Seconds to wait on shutdown
print_output = false         # Whether to print job output to logs
```

### Job Configuration

The `[jobs.*]` configuration below is for the core profile without `web`. In Web mode, job metadata is stored in SQLite and managed through the Dashboard.

```toml
[jobs.example]
schedule = "*/5 * * * *"     # Cron expression (required)
command = "echo 'Hello'"     # Command to execute (required)
description = "Example job"  # Optional description
enabled = true               # Enable/disable job
working_dir = "/app"         # Working directory
environment = { KEY = "value" }  # Environment variables
timeout = 300                # Timeout in seconds (0 = none)
shell = "/bin/bash"          # Override default shell
user = "deploy"              # Root daemon only; defaults to nobody when unset; ignored by non-root daemons
retry_count = 3              # Retry on failure
retry_delay = 60             # Seconds between retries
max_output_size = 1048576    # Max stdout/stderr capture (bytes)
run_on_startup = false       # Run immediately on daemon start
print_output = false         # Override global print_output setting
```

### Cron Expression Format

```
┌───────────── minute (0-59)
│ ┌───────────── hour (0-23)
│ │ ┌───────────── day of month (1-31)
│ │ │ ┌───────────── month (1-12)
│ │ │ │ ┌───────────── day of week (1-7, Sunday = 7)
│ │ │ │ │
* * * * *
```

**Examples:**

| Expression | Description |
|------------|-------------|
| `* * * * *` | Every minute |
| `*/5 * * * *` | Every 5 minutes |
| `0 * * * *` | Every hour |
| `0 0 * * *` | Daily at midnight |
| `0 2 * * *` | Daily at 2 AM |
| `0 0 * * 7` | Weekly on Sunday |
| `0 0 1 * *` | Monthly on the 1st |
| `0 9-17 * * 1-5` | Weekdays 9 AM - 5 PM hourly |
| `30 4 1,15 * *` | At 4:30 on 1st and 15th |

---

---

## Deployment

### Docker Compose

```yaml
services:
  flashcron:
    image: antonhub/flashcron:latest
    container_name: flashcron
    restart: always
    environment:
      - TZ=Asia/Shanghai
    volumes:
      - ./flashcron.toml:/data/config.toml:ro
      - /storage/flashcron:/data
    logging:
      driver: json-file
      options:
        max-size: "10m"
        max-file: "3"
```

### systemd Service

```ini
# /etc/systemd/system/flashcron.service
[Unit]
Description=FlashCron - Lightning-fast Cron Daemon
After=network.target

[Service]
Type=simple
User=flashcron
ExecStart=/usr/local/bin/flashcron run -c /etc/flashcron/config.toml
Restart=on-failure
RestartSec=5
Environment=RUST_LOG=info

[Install]
WantedBy=multi-user.target
```

```bash
sudo systemctl enable flashcron
sudo systemctl start flashcron
```

---

## Contributing

Contributions are welcome! See [CONTRIBUTING.md](docs/CONTRIBUTING.md) for guidelines.

```bash
# Clone the repository
git clone https://github.com/antonchen/flashcron
cd flashcron

# Run tests
cargo test

# Run benchmarks
cargo bench

# Build release
cargo build --release
```

---

## License

MIT License - see [LICENSE](LICENSE) for details.

---

## Author & Maintenance

FlashCron began as a fork of [flashcron](https://github.com/alfredo-baratta/flashcron) and is maintained here.

*   **Original Author:** Alfredo Baratta ([alfredobaratta@outlook.com](mailto:alfredobaratta@outlook.com))
*   **Current Maintainer:**  [Anton Chen](https://github.com/antonchen)

---

## Acknowledgments

- [cron](https://crates.io/crates/cron) - Cron expression parsing
- [tokio](https://tokio.rs) - Async runtime
- [clap](https://clap.rs) - CLI framework
- [log](https://github.com/rust-lang/log) - Logging facade
- [fern](https://github.com/daboross/fern) - Logging dispatcher
- [axum](https://github.com/tokio-rs/axum) - Web framework
- [tower-http](https://github.com/tower-rs/tower-http) - HTTP middleware
- [tailwind-css](https://tailwindcss.com) - UI styling (via CDN)

---

<div align="center">

**Scheduled work for Linux containers**

</div>
