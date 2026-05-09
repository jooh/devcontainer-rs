# Native CLI Architecture

## Overview

The binary entrypoint in `cmd/devcontainer/src/main.rs` stays intentionally thin and forwards straight into the library crate. From there the flow is:

1. `src/lib.rs`: process-wide argument validation, help dispatch, and unsupported-path handling.
2. `src/commands/`: top-level command dispatch and JSON/exit-code adaptation.
3. `src/runtime/`: native container runtime orchestration for `build`, `up`, `set-up`, `run-user-commands`, and `exec`.
4. Shared helpers such as `src/config.rs`, `src/process_runner.rs`, and `src/output.rs`.

## Crate layout

- `cmd/devcontainer/src/lib.rs`: top-level runtime entrypoint and shared process-wide guards.
- `cmd/devcontainer/src/cli.rs`: help text, log-format parsing, and command-surface metadata.
- `cmd/devcontainer/src/commands/`: thin command-family adapters.
- `cmd/devcontainer/src/runtime/`: native runtime subsystems for container-backed commands.
- `cmd/devcontainer/src/config.rs`: config-path resolution, JSONC parsing, and local variable substitution.
- `cmd/devcontainer/src/process_runner.rs`: subprocess helpers for captured and streaming execution.
- `cmd/devcontainer/src/output.rs`: text/json log rendering helpers.

## Command ownership

- `commands/configuration.rs`: `read-configuration`, `outdated`, and `upgrade`.
- `commands/exec.rs`: `exec` exit-code adaptation for captured vs streaming runtime results.
- `commands/collections.rs`: `features` and `templates` command families.
- `commands/common.rs`: shared CLI option parsing, config loading, manifest helpers, and packaging/file-copy helpers.
- `runtime/mod.rs`: public runtime entrypoints only. This file should stay orchestration-only.

## Runtime subsystems

- `runtime/build.rs`: image resolution, Dockerfile/context handling, build args, and push flow.
- `runtime/compose.rs`: compose-config inspection, compose CLI invocation, and compose service/container resolution.
- `runtime/container.rs`: container lookup, create/start/remove behavior, and label-based targeting.
- `runtime/context.rs`: config loading, inspect fallback, workspace resolution, user selection, and remote env merging.
- `runtime/engine.rs`: container-engine request construction plus normalized process execution/error handling.
- `runtime/exec.rs`: `exec` argv parsing and in-container exec request construction.
- `runtime/lifecycle.rs`: initialize/lifecycle command parsing, stage selection, and parallel execution.
- `runtime/metadata.rs`: container metadata serialization/merge and workspace-mount target parsing.

Dependency direction: `commands/*` may depend on `runtime/*`, but `runtime/*` should not depend on command-specific presentation logic. Keep JSON payload formatting in `commands/*` or `runtime/mod.rs`, and keep low-level engine/config behavior in the subsystem modules.

## Tests

- Rust unit tests live next to the implementation modules, including pure-helper coverage in `runtime/context.rs`, `runtime/exec.rs`, `runtime/lifecycle.rs`, and `runtime/metadata.rs`.
- Rust integration tests live under `cmd/devcontainer/tests/`.
- `cmd/devcontainer/tests/support/runtime_harness.rs` is the shared fake-engine harness for runtime integration coverage.
- The runtime smoke suite is split by concern: build, container lifecycle, context resolution, exec behavior, and lifecycle behavior.
- `acceptance/` holds repo-owned manual acceptance fixtures and the suite manifest for contributor-guided runtime checks.
- Repo-owned compatibility fixtures live under `src/test/parity/`.
- Node scripts in `build/` are repository maintenance tooling. They cover upstream/spec drift, generated CLI metadata/reference files, parity inventory/dashboard files, upstream test coverage, acceptance fixture manifests, native-only startup, no-node-runtime regressions, the parity harness, distribution package checks, and repo-owned devcontainer config validation.

## Compatibility assets

- `upstream/` is the only canonical location for upstream CLI TypeScript sources.
- `spec/` is the only canonical location for upstream schemas and normative spec docs.
- Node maintenance scripts must not become part of runtime execution. Release packaging should consume built artifacts through the dedicated package-preparation scripts rather than adding a Node bridge to the native CLI.

## Maintenance rules

- Keep new runtime logic in the Rust crate, not in root-level Node/TypeScript code.
- Prefer adding or extending a runtime subsystem over growing `lib.rs`, `main.rs`, or `runtime/mod.rs`.
- Remove test-only planners or duplicate helper paths when the real runtime implementation already has coverage.
- When a compatibility check needs new fixtures, add them under `src/test/parity/` and keep them tied to the pinned submodule revisions.
