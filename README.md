# devcontainer-rs

[![PyPI](https://img.shields.io/pypi/v/devcontainer-rs.svg)](https://pypi.org/project/devcontainer-rs/)
[![npm](https://img.shields.io/npm/v/devcontainer-rs.svg)](https://www.npmjs.com/package/devcontainer-rs)
[![Homebrew](https://img.shields.io/badge/Homebrew-tap-informational?logo=homebrew)](https://github.com/jooh/homebrew-tap)

This repository hosts a native Rust implementation of the Dev Containers CLI, with compatibility tracked against the pinned upstream TypeScript sources in `upstream/` and the pinned specification assets in `spec/`.

The shipped runtime is the Rust binary in `cmd/devcontainer`. Node is kept only for lightweight compatibility tooling such as upstream/spec drift checks, generated compatibility inventories, and the parity smoke harness.

## Install

Run the CLI without adding it permanently to your environment:

```bash
uvx --from devcontainer-rs devcontainer --version
npx @devcontainer-rs/cli --version
```

Install it with Homebrew:

```bash
brew install jooh/devcontainer-rs/devcontainer-rs
devcontainer --version
```

Homebrew resolves `jooh/devcontainer-rs` from the tap repository
`jooh/homebrew-tap`.

## Why

The main point of all this is to distribute a fat binary that implements dev containers without bringing in the whole node stack. In enterprise contexts this can be helpful.

Eventually we may also *extend* the upstream devcontainers/cli with additional functionality.

## Repository layout

- `cmd/devcontainer/`: native Rust CLI crate.
- `cmd/devcontainer/src/runtime/`: native runtime subsystems for container-backed commands.
- `acceptance/`: repo-owned manual acceptance scenarios and suite manifest.
- `upstream/`: canonical upstream `devcontainers/cli` baseline.
- `spec/`: canonical upstream `devcontainers/spec` schemas and docs.
- `build/`: repo-owned compatibility guard scripts.
- `src/test/parity/`: parity fixtures and golden files for repo-owned checks.
- `docs/`: contributor and release documentation for the native CLI.

Compatibility contract: this repository targets the exact submodule revision pinned at `HEAD:upstream`.

Specification contract: schema-sensitive behavior targets the exact submodule revision pinned at `HEAD:spec`.

## Submodules

Initialize submodules before running checks or editing compatibility-sensitive code:

```bash
git submodule update --init --recursive
```

If `upstream/` or `spec/` is missing or uninitialized, run the same command again and rerun the checks.

## Local development

Run the complete local gate before pushing:

```bash
make tests
```

Rust validation:

```bash
cargo fmt --manifest-path cmd/devcontainer/Cargo.toml --all -- --check
cargo clippy --manifest-path cmd/devcontainer/Cargo.toml --all-targets --all-features -- -D warnings
cargo check --manifest-path cmd/devcontainer/Cargo.toml --all-targets --all-features
cargo doc --manifest-path cmd/devcontainer/Cargo.toml --no-deps --document-private-items
cargo test --manifest-path cmd/devcontainer/Cargo.toml --locked
cargo deny --manifest-path cmd/devcontainer/Cargo.toml check -A license-not-encountered
```

Compatibility/tooling validation:

```bash
npm test
make actionlint-check
make shellcheck
```

Manual acceptance suite shape:

```bash
make acceptance-fixtures-check
```

The Node-based checks do not require installing project dependencies; they use built-in Node modules only. Node 20+ is still required to run them.

Generated command reference:

```bash
npm run generate-cli-reference
```

Generated parity inventory:

```bash
npm run generate-parity-inventory
```

Enable the repository-managed pre-commit hook:

```bash
npm run install-git-hooks
```

## Upstream and spec workflow

When updating upstream compatibility baselines:

```bash
git submodule update --init --recursive
git -C upstream fetch origin
git -C upstream checkout <new-upstream-commit>
git add upstream
git rev-parse HEAD:upstream
npm run check-upstream-submodule
npm run check-upstream-compatibility
npm run check-command-matrix
npm run check-parity-inventory
npm run check-parity-harness
```

When changing schema-sensitive behavior, also verify:

```bash
git rev-parse HEAD:spec
npm run check-spec-drift
```

If a pinned submodule revision changes, update the matching generated baseline files in `docs/upstream/`.

## Contributor notes

- Architecture, command flow, and runtime module ownership: `docs/architecture.md`
- Generated upstream command reference: `docs/upstream/command-reference.md`
- Generated parity inventory: `docs/upstream/parity-inventory.md`
- Native distribution and release notes: `docs/standalone/distribution.md`
- Runtime and compatibility guardrails: `docs/standalone/cutover.md`
