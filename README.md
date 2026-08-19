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

## Stable Runtime Environment Defaults

`devcontainer-rs` supports convenience environment variables for stable local
runtime defaults. These are repo-specific extensions: explicit CLI flags take
precedence, and unset or blank environment variables preserve the current
defaults.

For Podman-based local use:

```bash
export DEVCONTAINER_DOCKER_PATH=podman
export DEVCONTAINER_DOCKER_COMPOSE_PATH=podman-compose
devcontainer up --workspace-folder .
```

To always refresh a remote image when `up` creates or recreates the container,
use the native `--pull-always` extension:

```bash
devcontainer up --workspace-folder . --pull-always
```

For plain image configurations, the runtime explicitly pulls the source image.
For Compose configurations, it resolves, merges, and interpolates the effective
configuration, then determines the active `runServices` set and its dependency
closure. Build-backed services are excluded; for each unique remaining remote
image, it directly invokes the selected engine and checks the exit status of
each call (`pull IMAGE` or `pull --platform PLATFORM IMAGE`).
Dockerfile, Compose, and Feature builds use build-stage `--pull` where needed to
refresh remote bases, without force-pulling locally generated final tags such as
Feature or UID-update images. During `up`, this refresh happens before the final
existing-container lookup and reuse/start/create decision. An existing container
can therefore still be reused after its source is refreshed; combine the option
with `--remove-existing-container` when replacement is required.

To use the same non-standard config path across commands:

```bash
export DEVCONTAINER_CONFIG=.devcontainer/podman/devcontainer.json
devcontainer up --workspace-folder .
devcontainer exec uv sync
```

`DEVCONTAINER_CONFIG` accepts the same absolute or relative paths as `--config`.
Relative paths resolve against `--workspace-folder` when it is provided, or the
current directory otherwise. Container engine path values are executable paths,
not shell command strings with arguments. Use
`DEVCONTAINER_DOCKER_COMPOSE_PATH=podman-compose`, not
`DEVCONTAINER_DOCKER_COMPOSE_PATH="podman compose"`.

| CLI flag | Environment variable |
| --- | --- |
| `--config` | `DEVCONTAINER_CONFIG` |
| `--docker-path` | `DEVCONTAINER_DOCKER_PATH` |
| `--docker-compose-path` | `DEVCONTAINER_DOCKER_COMPOSE_PATH` |
| `--buildkit` | `DEVCONTAINER_BUILDKIT` |
| `--user-data-folder` | `DEVCONTAINER_USER_DATA_FOLDER` |
| `--container-data-folder` | `DEVCONTAINER_CONTAINER_DATA_FOLDER` |
| `--dotfiles-repository` | `DEVCONTAINER_DOTFILES_REPOSITORY` |
| `--dotfiles-install-command` | `DEVCONTAINER_DOTFILES_INSTALL_COMMAND` |
| `--dotfiles-target-path` | `DEVCONTAINER_DOTFILES_TARGET_PATH` |
| `--gpu-availability` | `DEVCONTAINER_GPU_AVAILABILITY` |
| `--update-remote-user-uid-default` | `DEVCONTAINER_UPDATE_REMOTE_USER_UID_DEFAULT` |
| `--mount-workspace-git-root` | `DEVCONTAINER_MOUNT_WORKSPACE_GIT_ROOT` |
| `--mount-git-worktree-common-dir` | `DEVCONTAINER_MOUNT_GIT_WORKTREE_COMMON_DIR` |
| `--workspace-mount-consistency` | `DEVCONTAINER_WORKSPACE_MOUNT_CONSISTENCY` |

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

CI also enforces the current Rust line coverage baseline:

```bash
cargo llvm-cov --manifest-path cmd/devcontainer/Cargo.toml --all-features --workspace --fail-under-lines 95
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
