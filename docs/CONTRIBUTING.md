# Contributing to FlashCron

Thank you for your interest in contributing to FlashCron! This document provides guidelines and information for contributors.

## Code of Conduct

Be respectful, inclusive, and constructive. We're all here to build something great together.

## How to Contribute

### Reporting Bugs

1. Check if the bug has already been reported in [Issues](https://github.com/alfredo-baratta/flashcron/issues)
2. If not, create a new issue with:
   - Clear, descriptive title
   - Steps to reproduce
   - Expected vs actual behavior
   - Environment details (OS, Rust version, FlashCron version)
   - Relevant logs or error messages

### Suggesting Features

1. Check existing issues and discussions for similar ideas
2. Create a new issue with:
   - Clear description of the feature
   - Use case and motivation
   - Proposed implementation (optional)

### Pull Requests

1. Fork the repository
2. Create a feature branch: `git checkout -b feature/my-feature`
3. Make your changes
4. Add tests for new functionality
5. Run the test suite: `cargo test`
6. Run clippy: `cargo clippy -- -D warnings`
7. Run rustfmt: `cargo fmt`
8. Commit with clear messages
9. Push and create a Pull Request

## Development Setup

### Prerequisites

- Rust 1.70 or later
- Git

### Building

```bash
# Clone the repository
git clone https://github.com/alfredo-baratta/flashcron
cd flashcron

# Build debug version
cargo build

# Build release version
cargo build --release
```

### Testing

```bash
# Run all tests
cargo test

# Run specific test
cargo test test_name

# Run tests with output
cargo test -- --nocapture

# Run benchmarks
cargo bench
```

### Code Style

- Follow Rust conventions and idioms
- Use `cargo fmt` for formatting
- Use `cargo clippy` for linting
- Document public APIs with doc comments
- Keep functions focused and small
- Prefer explicit over implicit

### Performance Considerations

FlashCron is designed for efficiency. When contributing:

- Avoid unnecessary allocations in hot paths
- Prefer stack allocation over heap when possible
- Use iterators instead of collecting into vectors
- Profile changes that might affect performance
- Add benchmarks for performance-critical code

### Commit Messages

Use clear, descriptive commit messages:

```
feat: add retry support for failed jobs

- Add retry_count and retry_delay to job config
- Implement exponential backoff
- Add tests for retry behavior
```

Prefixes:
- `feat:` - New feature
- `fix:` - Bug fix
- `docs:` - Documentation only
- `test:` - Adding tests
- `refactor:` - Code refactoring
- `perf:` - Performance improvement
- `chore:` - Maintenance tasks

## Project Structure

```
flashcron/
├── src/
│   ├── main.rs          # CLI entry point
│   ├── lib.rs           # Library exports
│   ├── error.rs         # Error types
│   ├── config/          # Configuration parsing
│   │   ├── mod.rs
│   │   ├── job.rs
│   │   └── settings.rs
│   ├── scheduler/       # Core scheduler
│   │   ├── mod.rs
│   │   ├── engine.rs
│   │   └── state.rs
│   └── executor/        # Job execution
│       └── mod.rs
├── tests/               # Integration tests
├── benches/             # Benchmarks
├── docs/                # Documentation
└── examples/            # Example configs
```

## Release Process

Releases are automated via GitHub Actions when a tag is pushed:

1. Update version in `Cargo.toml`
2. Update `CHANGELOG.md`
3. Create and push tag: `git tag v0.1.0 && git push --tags`

## Questions?

Open a discussion or issue on GitHub. We're happy to help!

## License

By contributing, you agree that your contributions will be licensed under the MIT License.
