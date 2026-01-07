# Contributing to Plenum

Thank you for your interest in contributing to Plenum! This document provides guidelines and instructions for development.

## Development Setup

### Prerequisites

- Rust 1.70 or later (stable toolchain)
- Git

### Getting Started

1. Clone the repository:
   ```bash
   git clone https://github.com/yourusername/plenum.git
   cd plenum
   ```

2. Build the project:
   ```bash
   cargo build
   ```

3. Run tests:
   ```bash
   cargo test
   ```

## Development Workflow

### Build Commands

```bash
# Build in debug mode
cargo build

# Build in release mode (optimized)
cargo build --release

# Build and run the binary
cargo run -- --help

# Check code without building
cargo check --all-targets
```

### Testing

```bash
# Run all tests
cargo test

# Run tests with output
cargo test -- --nocapture

# Run a specific test
cargo test test_name

# Run tests for a specific module
cargo test module_name
```

### Code Quality

Before submitting a PR, ensure your code passes all quality checks:

```bash
# Format code
cargo fmt

# Check formatting without modifying files
cargo fmt --check

# Run linter
cargo clippy --all-targets --all-features

# Run security audit
cargo audit
```

### Pre-Commit Checklist

Before committing, run:

```bash
cargo fmt
cargo clippy --all-targets --all-features
cargo test
```

## Contribution Guidelines

### Core Principles

Plenum is designed for autonomous AI agents, not humans. All contributions must adhere to these principles:

1. **Agent-first, machine-only**
   - No interactive runtime UX (REPL or TUI)
   - JSON-only output to stdout
   - No human-friendly formatting

2. **No query language abstraction**
   - SQL remains vendor-specific
   - No compatibility layers
   - PostgreSQL SQL ≠ MySQL SQL ≠ SQLite SQL

3. **Explicit over implicit**
   - No inferred databases, schemas, limits, or permissions
   - Missing inputs MUST fail fast

4. **Least privilege**
   - Read-only is the default mode
   - Writes and DDL require explicit capabilities

5. **Determinism**
   - Identical inputs produce identical outputs

### Before Adding Code

Ask: **"Does this make autonomous agents safer, more deterministic, or more constrained?"**

If the answer is no, it does not belong in Plenum.

### What NOT to Add

Do NOT implement:
- ORMs or query builders
- Migrations
- Interactive shells
- Implicit defaults
- Connection pooling across invocations
- Caching
- Schema inference heuristics
- Human UX features

### Code Style

- Follow the existing code style
- Use `rustfmt` for formatting (enforced in CI)
- Address all `clippy` warnings
- Write clear, concise comments
- Prefer explicit over clever code

### Testing Requirements

- All new features must have tests
- Tests must be deterministic
- No external cloud services in tests
- Use snapshot tests for JSON output validation

### Documentation

- Update README.md if adding user-facing features
- Update CLAUDE.md if changing core principles
- Update PROJECT_PLAN.md if changing architecture
- Add inline documentation for public APIs

## Pull Request Process

1. Create a feature branch from `main`
2. Make your changes following the guidelines above
3. Ensure all tests pass
4. Run `cargo fmt` and `cargo clippy`
5. Update documentation as needed
6. Submit a pull request with a clear description

### PR Requirements

- [ ] Code compiles without errors
- [ ] All tests pass
- [ ] Code is formatted (`cargo fmt`)
- [ ] No clippy warnings (`cargo clippy`)
- [ ] Documentation is updated
- [ ] Commit messages are clear and descriptive

## Questions?

See [CLAUDE.md](CLAUDE.md) for core principles and architecture.
See [PROJECT_PLAN.md](PROJECT_PLAN.md) for implementation roadmap.
See [RESEARCH.md](RESEARCH.md) for design decisions and rationale.

## License

By contributing to Plenum, you agree that your contributions will be licensed under the MIT OR Apache-2.0 license.
