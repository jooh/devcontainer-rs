# Native distribution

## Release artifacts

- GitHub Releases is the active distribution channel.
- `dist-workspace.toml` is the release artifact baseline for cargo-dist.
- `.github/workflows/devcontainer-release.yml` builds release archives for Linux x64 (glibc), Linux x64 (musl), macOS x64, and macOS arm64 with cargo-dist.
- Each release artifact currently includes a compressed archive and a SHA-256 checksum file.
- npm publishes two wrapper entrypoints:
  - `devcontainer-rs` for `npx devcontainer-rs`
  - `@devcontainer-rs/cli` for `npx @devcontainer-rs/cli`
- npm also publishes scoped native packages under `@devcontainer-rs/devcontainer-*` for the supported target matrix.
- npm publication uses npm Trusted Publishers from `.github/workflows/devcontainer-release.yml`; the workflow does not use a long-lived `NPM_TOKEN`.
- PyPI publishes the same native CLI as the `devcontainer-rs` package. Installing it exposes the `devcontainer` executable on `PATH`.
- Homebrew publishes the `devcontainer-rs` formula to the `jooh/devcontainer-rs-tap` tap, backed by the `jooh/homebrew-devcontainer-rs-tap` repository. The formula installs the same native `devcontainer` executable from GitHub Release archives.

## npm install flow

The npm wrappers ship JavaScript launchers only. They resolve the correct prebuilt native package for the host OS, CPU, and Linux libc at install/run time without downloading binaries after installation.

```bash
npx devcontainer-rs --version
npx @devcontainer-rs/cli --version
npm install -g devcontainer-rs
```

Global installation exposes both `devcontainer-rs` and `devcontainer` on `PATH`. The scoped `@devcontainer-rs/cli` wrapper exposes `devcontainer`.

## PyPI install flow

PyPI is a convenience channel for enterprise environments that already approve Python package installation:

```bash
uv tool install devcontainer-rs
```

`pipx install devcontainer-rs` is also supported. The package ships platform-specific wheels with the Rust binary embedded; it does not provide a Python API and does not download a binary during installation or first run.

The initial PyPI wheel set matches the standalone release targets:

- Linux x64 glibc (`manylinux2014_x86_64`)
- Linux x64 musl (`musllinux_1_2_x86_64`)
- macOS x64
- macOS arm64

## Homebrew install flow

Homebrew is distributed through a tap because the Homebrew core formula named `devcontainer` already tracks the upstream Node.js implementation:

```bash
brew install jooh/devcontainer-rs-tap/devcontainer-rs
devcontainer --version
```

Release automation renders `Formula/devcontainer-rs.rb` after GitHub Release assets are published, then commits and pushes the formula update to the tap repository. The workflow requires a `HOMEBREW_TAP_TOKEN` secret with write access to `jooh/homebrew-devcontainer-rs-tap`.

## Local build flow

- `scripts/standalone/build.sh <target>` builds the Rust release binary and places it under `dist/standalone/`.
- `scripts/standalone/build-linux-x64-musl.sh` builds the Linux x64 musl artifact for older-glibc distro compatibility.
- `scripts/standalone/smoke.sh <binary>` runs the repo-owned smoke commands against a built artifact.
- `~/.cargo/bin/dist build --artifacts=local --target <triple> --allow-dirty` builds the cargo-dist archive into `target/distrib/`.
- `node build/prepare-npm-packages.js --artifacts-dir target/distrib --output-dir dist/npm` stages the publishable npm wrapper/native packages from dist outputs.
- `make npm-package-smoke` verifies the wrapper/native tarballs with local `npm pack` installs and command execution.
- `make pypi-wheel-smoke` builds a local wheel with `uv`, installs it into an isolated environment, and runs the standalone smoke against the installed `devcontainer` executable.
- `make homebrew-formula-check` verifies the formula renderer and `make homebrew-publish-workflow-check` verifies the release workflow wiring for the tap update.

## Current limitations

- The repository no longer ships or maintains the old bundled-Node installer path.
- Release automation does not currently sign artifacts or notarize macOS builds.
- PyPI publication requires a PyPI Trusted Publisher configured for `.github/workflows/devcontainer-release.yml` and the `pypi` GitHub environment.
- npm publication requires npm Trusted Publisher entries for each npm package, pointing at the same `.github/workflows/devcontainer-release.yml` workflow.
- Homebrew publication requires `HOMEBREW_TAP_TOKEN`; without it the release job creates GitHub/PyPI/npm artifacts but the tap update fails.
- Compatibility tooling in `package.json` is not part of the runtime distribution path.
