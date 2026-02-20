# AGENTS.md - Coding Agent Guidelines for payments-rs

This file is an index. Load only the specific doc(s) relevant to your task to minimize context usage.

## Before Starting Any Task

**1. Estimate the size** of the change using t-shirt sizing:

| Size | Lines of change | Action |
|------|----------------|--------|
| XS | < 50 | Proceed directly |
| S | 50–250 | Proceed directly |
| M | 250–750 | Proceed directly |
| L | 750–2,500 | Proceed directly |
| XL | > 2,500 | **Stop — split into increments first** |

If the estimate is XL, create a work file in `work/` that decomposes the task into L-or-smaller increments, then work through them one PR at a time. See [agents/incremental-work.md](agents/incremental-work.md) for the work file format.

**2. Check `work/`** for an active task file on the same topic before starting new work. If one exists, resume from the first unchecked task.

<!-- Uncomment and populate when you have active work files:
| File | Description |
|---|---|
| [work/example-task.md](work/example-task.md) | Description of the task |
-->

## Generic Docs

These docs apply to all projects using this agent structure:

| Doc | When to load |
|---|---|
| [agents/bug-fixes.md](agents/bug-fixes.md) | Resolving bugs (includes regression test requirement) |
| [agents/coverage.md](agents/coverage.md) | Any edit that adds or modifies functions (100% function coverage required) |
| [agents/rust/coverage.md](agents/rust/coverage.md) | Rust-specific coverage tooling (cargo-llvm-cov) |
| [agents/incremental-work.md](agents/incremental-work.md) | Managing a work file for a multi-increment task |

## Project-Specific Docs

<!-- Add your project-specific docs here. Examples:

| Doc | When to load |
|---|---|
| [docs/agents/build-and-test.md](docs/agents/build-and-test.md) | Running builds, tests, linting, or formatting |

-->
