# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- Manual job triggers, dashboard tab filtering, and secure `api_token` authentication.

## [0.2.0] - 2026-03-12

### Added
- Auto-resolution for configuration files: subcommands like `list`, `schedule`, and `history` will automatically detect and use the configuration file from a running `flashcron run` instance, or from the `FLASHCRON_CONFIG` environment variable, eliminating the need to repeatedly pass `-c`.
- Built-in HTTP Web Dashboard and API to monitor job status and execution history (enabled by default via `web` feature).
- Configuration options `api_host` and `api_port` to customize the Web API and Dashboard server listener (defaults to `0.0.0.0:8080`).
- `print_output` global and per-job configuration to toggle command output logging.
- Timezone configuration support for global and per-job scheduling, allowing jobs to run in specific local times.

### Changed
- Replaced global `history_size` configuration with `job_history_size` (default: 100) and `max_history_size` (default: 10000) to allow per-job history limits while preventing overall memory exhaustion.
- Migrated logging framework from `tracing` to `log` and `fern` for a more lightweight footprint and better control over formatting.
- Renamed `shutdown_grace_period` to `shutdown_timeout` to clarify its function, and fixed the issue where the shutdown signal would cause immediate aborts without respecting this timeout value.

## [0.1.0] - 2025-01-XX

### Added
- Initial release of FlashCron
- Core scheduler engine with priority queue for efficient job scheduling
- Full cron expression support (5-field format)
- TOML configuration format with validation
- Job execution with timeout and retry support
- Environment variable injection per job
- Working directory configuration per job
- Structured logging with tracing (human-readable and JSON formats)
- CLI commands: `run`, `validate`, `list`, `trigger`, `schedule`, `init`
- Hot configuration reload via file watcher
- Graceful shutdown handling with configurable grace period
- Cross-platform support (Linux, macOS, Windows)
- GitHub Actions CI/CD workflows
- Docker support with multi-arch images (amd64, arm64)
- Comprehensive test suite (36 tests)
- Benchmark suite with Criterion

### Performance Metrics
- Memory footprint: ~2-5 MB
- Startup time: <10ms
- CPU at idle: <0.1%
- Scheduler init (100 jobs): ~50 μs
- Scheduler init (1000 jobs): ~400 μs
- Cron parsing: 1.8-4.9 μs
- Next occurrence calculation: 400-500 ns

### Green Features
- Efficient priority queue scheduling (no polling)
- Minimal wake-ups with precise sleep calculations
- Zero allocation in hot paths
- Aggressive release optimizations (LTO, strip)
- Tiny binary size (~3 MB stripped)

[Unreleased]: https://github.com/antonchen/flashcron/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/antonchen/flashcron/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/antonchen/flashcron/releases/tag/v0.1.0
