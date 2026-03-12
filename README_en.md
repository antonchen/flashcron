# FlashCron

<div align="center">

**A lightning-fast, ultra-efficient cron daemon written in Rust**

*Schedule tasks at the speed of light*

[![CI](https://github.com/antonchen/flashcron/workflows/CI/badge.svg)](https://github.com/antonchen/flashcron/actions)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.85%2B-orange.svg)](https://www.rust-lang.org)

[简体中文](README.md) | [Features](#features) | [Installation](#installation) | [Quick Start](#quick-start) | [Documentation](docs/) | [Contributing](#contributing)

</div>

---

## Fork Enhancements
This version is a fork based on **v0.1.0** with several key features added:

- **Built-in Web Dashboard**: A lightweight monitoring interface to track job status, success rates, and execution history in real-time (includes a RESTful API).
- **Timezone Support**: Native support for IANA timezones (e.g., `Asia/Shanghai`), ensuring precise scheduling across different regions.
- **Job Output Logging**: Real-time capture of `stdout` and `stderr` directly into system logs, configurable at both global and per-job levels.

---

## Why FlashCron?

Traditional cron daemons get the job done, but they weren't built for speed. **FlashCron** is engineered from the ground up for blazing-fast performance:

| Metric | FlashCron (Current) | FlashCron (Original) | Traditional cron | Improvement |
|--------|---------------------|----------------------|------------------|-------------|
| **Scheduler Init (1000 jobs)** | ~400 μs | ~400 μs | 10-50 ms | **Up to 125x faster** |
| **Next Run Calculation** | ~500 ns | ~500 ns | 10-100 μs | **Up to 200x faster** |
| **Memory Usage** | **10-20 MB** | 2-5 MB | 10-50 MB | **Up to 80% less** |
| **Startup Time** | <10 ms | <10 ms | 50-200 ms | **Up to 20x faster** |
| **CPU at Idle** | <0.1% | <0.1% | 0.5-2% | **Up to 95% less** |

### The Flash Philosophy

- **Priority Queue Scheduling**: O(log n) operations, no polling loops
- **Zero-Copy Parsing**: Minimal allocations in hot paths
- **Precise Sleep**: CPU stays idle until the exact moment needed
- **Concurrent Execution**: Configurable parallelism with async I/O
- **Optimized Binaries**: LTO, stripped symbols, maximum optimization

---

## Features

### Core Functionality
- **Standard Cron Expressions**: Full 5-field cron syntax support
- **Timezone Support**: Run jobs in any timezone, with flexible priority logic
- **TOML Configuration**: Human-readable, version-control friendly
- **Hot Reload**: Automatic config reload on file changes
- **Graceful Shutdown**: Waits for running jobs to complete

### Job Management
- **Timeout Handling**: Kill runaway jobs automatically
- **Retry Support**: Configurable retry count and delay
- **Environment Variables**: Per-job environment injection
- **Working Directory**: Per-job working directory
- **Concurrent Limits**: Prevent resource exhaustion

### Observability
- **Web Dashboard**: Built-in HTTP interface to monitor job status and history
- **Structured Logging**: Human-readable or JSON format
- **Execution History**: Track job success/failure rates

### Cross-Platform
- **Linux**: Native support, systemd integration
- **macOS**: Full support including Apple Silicon
- **Windows**: Native Windows support

---

## Installation

### From Releases (Recommended)

Download the pre-compiled binary for your platform from the [GitHub Releases](https://github.com/antonchen/flashcron/releases) page.

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

### Docker

```bash
docker pull ghcr.io/antonchen/flashcron:latest
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

[jobs.backup]
schedule = "0 2 * * *"  # Daily at 2 AM
command = "/usr/local/bin/backup.sh"
description = "Daily backup"
timeout = 3600
retry_count = 3

[jobs.cleanup]
schedule = "0 */6 * * *"  # Every 6 hours
command = "find /tmp -mtime +7 -delete"
description = "Clean old temp files"
enabled = true
```

### 3. Validate Configuration

```bash
flashcron validate -c flashcron.toml
```

### 4. Start the Daemon

```bash
flashcron run -c flashcron.toml
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
| `flashcron run -c config.toml` | Start the daemon |
| `flashcron validate -c config.toml` | Validate configuration |
| `flashcron list -c config.toml` | List all jobs |
| `flashcron schedule -c config.toml` | Show upcoming runs |
| `flashcron trigger <job> -c config.toml` | Trigger job manually |
| `flashcron init -o config.toml` | Generate default config |

### Options

| Option | Description |
|--------|-------------|
| `-c, --config <PATH>` | Configuration file path |
| `-l, --log-level <LEVEL>` | Log level (trace, debug, info, warn, error) |
| `--json` | Output logs in JSON format |
| `--foreground` | Run in foreground (don't daemonize) |

### Configuration Resolution

When running commands like `list`, `schedule`, `trigger`, or `validate`, FlashCron automatically determines the configuration file path in the following order of precedence:

1. **Explicit argument (`-c custom.toml`)**: Provided via CLI flag.
2. **Environment variable (`FLASHCRON_CONFIG`)**: e.g., `export FLASHCRON_CONFIG=custom.toml`.
3. **Running daemon state**: If the `flashcron run` daemon is running, other commands automatically use the configuration file path the daemon is using (tracked via `/tmp/flashcron.state`).
4. **Default file**: Fallback to `flashcron.toml` in the current directory.

This allows you to simply run `flashcron list` without having to pass the `-c` argument if the daemon is already running.

---

## Configuration Reference

### Global Settings

```toml
[settings]
log_level = "info"           # trace, debug, info, warn, error
json_logs = false            # JSON format for log aggregators
api_host = "0.0.0.0"       # Web Dashboard / API host
api_port = 8080              # Web Dashboard / API port
max_concurrent_jobs = 10     # 0 = unlimited
shell = "/bin/sh"            # Default shell
watch_config = true          # Hot reload on config changes
job_history_size = 100         # Max history entries to keep per job
max_history_size = 10000       # Max total history entries to keep globally
timezone = "System"          # Timezone (e.g., "System", "UTC", "Asia/Shanghai")
shutdown_timeout = 30   # Seconds to wait on shutdown
print_output = false         # Whether to print job output to logs
```

> **⚠️ Warning:** Execution history is stored entirely in RAM. Increasing `job_history_size` or `max_history_size` (especially for high-frequency jobs with large output) will significantly increase memory consumption.

### Job Configuration

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

## Performance Benchmarks

Measured on AMD Ryzen 5 / Intel i7 equivalent:

| Operation | Time | Notes |
|-----------|------|-------|
| Cron expression parsing | 1.8-4.9 μs | Depending on complexity |
| Next occurrence calculation | 400-500 ns | Sub-microsecond |
| Scheduler init (10 jobs) | ~10 μs | Instant startup |
| Scheduler init (100 jobs) | ~50 μs | Scales linearly |
| Scheduler init (1000 jobs) | ~400 μs | Still under 1ms |
| Config parsing (50 jobs) | ~566 μs | TOML parsing included |
| Config parsing (200 jobs) | ~2.9 ms | Large configs handled |

### Memory Profile

| State | Memory |
|-------|--------|
| Idle (10 jobs, Original) | ~2 MB |
| Idle (100 jobs, Original) | ~3 MB |
| Running (10 concurrent, Original) | ~5 MB |
| **FlashCron (Current) (1000 jobs + Web)** | **~10-20 MB** |

---

## Deployment

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

### Docker Compose

```yaml
version: '3.8'
services:
  flashcron:
    image: ghcr.io/antonchen/flashcron:latest
    volumes:
      - ./flashcron.toml:/app/config/flashcron.toml:ro
      - ./scripts:/scripts:ro
    restart: unless-stopped
    logging:
      driver: json-file
      options:
        max-size: "10m"
        max-file: "3"
```

### Kubernetes

```yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: flashcron
spec:
  replicas: 1
  selector:
    matchLabels:
      app: flashcron
  template:
    metadata:
      labels:
        app: flashcron
    spec:
      containers:
      - name: flashcron
        image: ghcr.io/antonchen/flashcron:latest
        resources:
          requests:
            memory: "8Mi"
            cpu: "10m"
          limits:
            memory: "32Mi"
            cpu: "100m"
        volumeMounts:
        - name: config
          mountPath: /app/config
      volumes:
      - name: config
        configMap:
          name: flashcron-config
```

---

## Comparison with Alternatives

| Feature | FlashCron | cron | fcron | systemd-timer |
|---------|-----------|------|-------|---------------|
| Memory usage | ~2-5 MB | ~1 MB | ~5 MB | N/A (systemd) |
| Config format | TOML | crontab | fcrontab | unit files |
| Hot reload | Yes | No | Yes | Yes |
| Retry support | Yes | No | Yes | Yes |
| Timeout | Yes | No | Yes | Yes |
| Concurrent limit | Yes | No | No | No |
| Cross-platform | Yes | Unix | Unix | Linux |
| Performance | Optimized | Standard | Standard | Standard |

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

**This is a fork of the original [flashcron](https://github.com/alfredo-baratta/flashcron) project with significant modifications and improvements.**

*   **Original Author:** Alfredo Baratta ([alfredobaratta@outlook.com](mailto:alfredobaratta@outlook.com))
*   **Current Maintainer:**  ([Anton Chen](https://github.com/antonchen))

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

**Built for speed**

*Because every millisecond counts*

</div>
