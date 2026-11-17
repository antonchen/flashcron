# FlashCron Agent Instructions

## Project Intent

FlashCron is a Linux-only, container-oriented cron service. Do not introduce compatibility layers for other operating systems. Keep the three intended product variants distinct:

- `core`: daemon and CLI without the web interface or dependency management.
- `web`: daemon, CLI, HTTP dashboard/API, and SQLite job history.
- `dep-manager`: dependency management for container use; this profile also needs the web interface and SQLite dependency manifest.

These are product profiles, not three separate applications. Preserve the existing Cargo feature model unless a task explicitly changes it. Check Dockerfiles and release workflows when changing feature-dependent behavior; source availability does not mean a published image contains that feature.

## Design Decisions

- The expected deployment is one FlashCron instance inside a container. Multi-user host socket isolation is out of scope; do not expand routine work into host-level permission hardening.
- Removing a job from the current config may remove its persisted history and statistics. This is intentional retention behavior.
- A container rebuild must restore configured dependencies from the persisted SQLite manifest. Do not treat a previous container's `Installed` status as proof that a dependency exists in the new container.
- Package restoration must not have a FlashCron-imposed execution timeout. HTTP and IPC must become available while restore runs; scheduler job dispatch must wait until restore finishes.
- During dependency restoration, replace the normal dashboard with restore status. Expose status through HTTP and IPC, including unmodified stdout/stderr and their merged observation order. Two separate pipes cannot establish the process-wide original write order.
- Keep dependency restoration observable and queryable. Do not block the service control plane while waiting for APT/PIP commands.
- IPC request size and read-timeout limits are not a product requirement yet. Do not introduce a numeric limit without an explicit task.

## Implementation Guidance

- Read `docs/ARCHITECTURE.md` and `docs/EXECUTION_FLOWS.md` for the component map and intended flows.
- Write idiomatic Rust and follow nearby module patterns and public interfaces.
- Avoid unnecessary dependencies and allocations on scheduler hot paths. Profile only when a change is likely to affect real performance.
- Keep scheduler operations asynchronous: use the existing priority queue and Tokio message passing; do not block Tokio worker threads.
- Preserve the project's structured `log` key-value style for runtime state changes. Do not include secrets in log fields.
- Handle child process stdout and stderr, cancellation, and configured job timeouts without blocking the runtime.
- Keep the dashboard self-contained in `src/api/index.html`; handle failed frontend HTTP requests.
- When behavior changes, inspect callers, tests, feature flags, Dockerfiles, release workflows, examples, README files, and `docs/CHANGELOG.md` as applicable.
- Add focused tests for meaningful behavior changes and regressions. Avoid tests that only restate implementation details.
- For Rust changes, keep formatting and lint checks clean and run relevant tests when authorized. Do not run builds or tests for documentation-only changes.
- Preserve user changes in the worktree. Do not commit, deploy, or run external side effects unless requested.
- For package installation tests, do not invoke real APT/PIP operations; use controlled commands or test doubles.
