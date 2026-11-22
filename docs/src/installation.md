# Installation

**devcontainer-rs** is a native Rust implementation of the Dev Containers CLI.
It runs your existing `devcontainer.json` configurations without a Node.js
runtime. Choose one installation method below.

## Homebrew

```bash
brew install jooh/tap/devcontainer-rs
devcontainer --version
```

The tap is [`jooh/homebrew-tap`](https://github.com/jooh/homebrew-tap),
which Homebrew names `jooh/tap`.

## Python tools

Install the packaged native binary with [uv](https://docs.astral.sh/uv/):

```bash
uv tool install devcontainer-rs
devcontainer --version
```

Or run it without a permanent installation:

```bash
uvx --from devcontainer-rs devcontainer --version
```

## npm

If you already use Node.js:

```bash
npm install -g @devcontainer-rs/cli
devcontainer --version
```

For a single invocation, use `npx @devcontainer-rs/cli --version`.
These packages launch the native binary; Node.js is only needed for the wrapper.

## Standalone binary

Download the archive and matching SHA-256 checksum from
[GitHub Releases](https://github.com/jooh/devcontainer-rs/releases), verify the
checksum, and extract the `devcontainer` executable into a directory on your
`PATH`. Prebuilt binaries are available for Linux and macOS on x64 and arm64,
including musl builds for Linux. Windows users can run the Linux version in WSL2.

## Before you start

Container commands need a running Docker or Podman engine, and fetching Features
from registries requires `curl`. Compose projects also need Docker Compose or a
compatible Compose executable. If another
`devcontainer` is installed, check that your shell resolves the intended binary.

Continue with the [usage guide](usage.md).
