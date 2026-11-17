# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [1.0.0] - 2026-11-16

### Added
- Read and replace Debian APT mirrors and the active virtual environment's PIP index from the Dependency Management page.
- Automatically select Chinese or English for the Dashboard and dependency restore page from the browser's preferred language, with English as the fallback.
- Serialize Web job saves and deletions through the scheduler to prevent concurrent updates from restoring deleted jobs in memory.
- Add a dedicated Dependency Management dashboard page and authenticated job/script deletion, with running-job protection and job history cleanup.
- Web Dashboard job creation/editing with SQLite persistence and immediate scheduler updates, plus executable script file management in a persistent directory.
- Local Unix socket control channel for `trigger`, `status`, and `stop`, available in both foreground and background daemon mode.
- Release binaries for core, Web, and dependency-management feature profiles on each supported target.
- Container dependency-manifest restore with queryable progress, retry support, and scheduler startup gating.

### Changed
- Leave package version resolution to APT/PIP and remove the approximate downgrade warning.
- Updated direct dependencies to their latest stable releases, resolved transitive dependencies compatible with Rust 1.89.0, and raised the minimum supported Rust version accordingly.
- Support Linux only; remove other operating system compatibility code and release targets.
- Reworked daemon lifecycle: `run` now starts in foreground by default, while `run --daemon` starts a detached background supervisor without relying on state files.
- `status`, `trigger`, `stop`, and `history` now use the same local IPC control plane; `history` no longer depends on the HTTP API.
- The local IPC socket path is fixed and is no longer configurable.
- Scheduler concurrency now uses a global ready queue for different jobs and skips overlapping cron ticks when the same job is already `queued`, `starting`, or `running`.
- Job output logging and persisted history output now share the same merged stdout/stderr order.
- Global `settings.working_dir` is now applied when a job does not override its own working directory. When FlashCron runs as root, Unix `jobs.<name>.user` selects the job account and an unset value defaults to `nobody`; non-root instances ignore this field.
- Removed obsolete configuration knobs `watch_config`, `pid_file`, and `capture_output`; hot reload is always enabled and history output persistence is derived from `web` plus `max_history_size`.
- Unix job timeouts now terminate the complete child process group, including descendants.
- The `dep-manager` feature now enables `web` automatically.
- Ignore TOML jobs in Web configuration with a warning, and reject unknown configuration fields instead of silently ignoring removed options.

### Fixed
- Retained the Unix job process group ID so timeout cleanup still reaches descendants after the shell exits.
- Package restore pages now update retry controls as status changes, accept API tokens, and display polling and retry errors.

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

[1.0.0]: https://github.com/antonchen/flashcron/compare/v0.2.0...v1.0.0
[0.2.0]: https://github.com/antonchen/flashcron/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/antonchen/flashcron/releases/tag/v0.1.0
