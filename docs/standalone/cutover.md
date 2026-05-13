# Runtime cutover status

- The distributed CLI runtime is native Rust only.
- Runtime invocation of Node is forbidden; `build/check-no-node-runtime.js` enforces that contract.
- `DEVCONTAINER_NATIVE_ONLY=1` remains a local regression guard for unsupported paths.

## Active guardrails

- `cargo fmt --manifest-path cmd/devcontainer/Cargo.toml --all -- --check`
- `cargo clippy --manifest-path cmd/devcontainer/Cargo.toml --all-targets --all-features -- -D warnings`
- `cargo check --manifest-path cmd/devcontainer/Cargo.toml --all-targets --all-features`
- `cargo doc --manifest-path cmd/devcontainer/Cargo.toml --no-deps --document-private-items`
- `cargo test --manifest-path cmd/devcontainer/Cargo.toml --locked`
- `node build/check-native-only.js`
- `node build/check-no-node-runtime.js`
- `node build/check-parity-harness.js`
- `node build/generate-cli-reference.js --check`
- `node build/generate-parity-inventory.js --check`

## Current parity scope

The automated repo-owned parity checks currently verify:

- command-matrix drift against pinned upstream CLI sources
- generated command-reference drift against the pinned upstream CLI sources
- generated native parity-inventory drift against the current Rust source
- schema drift against the pinned spec submodule
- native startup/help behavior without Node on `PATH`
- pinned `read-configuration` parity scenarios, including upstream-style workspace output

The native runtime now also has repo-owned Rust integration coverage for `build`, `up`, `set-up`, `run-user-commands`, and `exec` using a podman-compatible fake engine harness, including Docker Compose project-name and existing-container coverage for `build` and `up`.

The Rust CLI now declares every pinned upstream command path, but command-depth parity is still uneven. The generated inventory in `docs/upstream/parity-inventory.md` is the current source of truth for missing option references and known command-level gaps.

The highest-impact remaining gaps are still substantive:

- replacing fixture/manual Feature metadata resolution with real OCI-backed fetch and dependency behavior
- replacing fixture/manual lockfile catalogs with real registry-backed resolution
- replacing local-layout `features` / `templates` published flows with real OCI registry behavior
- broadening parity-harness and runtime coverage beyond the current pinned scenarios
