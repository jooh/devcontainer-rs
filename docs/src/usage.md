# Usage

Run these commands from your project directory. The CLI reads
`.devcontainer/devcontainer.json`, falling back to `.devcontainer.json`.
Use `--workspace-folder /path/to/project` to target another directory.

## Start a development container

If your project already has a configuration, use it. Otherwise, create
`.devcontainer/devcontainer.json`:

```json
{
  "name": "My project",
  "image": "mcr.microsoft.com/devcontainers/base:ubuntu",
  "features": {
    "ghcr.io/devcontainers/features/git:1": {}
  },
  "postCreateCommand": "git --version"
}
```

Start the container:

```bash
devcontainer up --workspace-folder .
```

`up` builds the image when needed, mounts your workspace, and runs the configured
lifecycle commands. Running it again reuses the existing container when possible.
Image, Dockerfile, and Docker Compose configurations are supported.

## Run commands inside it

```bash
devcontainer exec --workspace-folder . git --version
devcontainer exec --workspace-folder . sh
```

Use `exec` for a shell, a test command, or any other program installed in the
container. It returns the program's exit code, so it also works in scripts and CI.

## Add tools with Features

Features are reusable installation steps, configured in the `features` object.
The example above installs Git from the Dev Containers Feature collection.
Feature options go in the value object; for example, `{"version": "latest"}`.
Choose Features compatible with your base image.

After changing the configuration, recreate the container to apply the changes:

```bash
devcontainer up --workspace-folder . --remove-existing-container
```

This replaces the existing container. Keep work you need in the mounted workspace
or a persistent volume.

## Build or inspect a configuration

Build an image without starting a container:

```bash
devcontainer build --workspace-folder .
```

Inspect the resolved configuration:

```bash
devcontainer read-configuration --workspace-folder .
```

For Podman, persistent runtime defaults, and image refresh controls, see
[enhancements](enhancements.md). For all commands and options, see the
[CLI reference](cli.md), or run `devcontainer <command> --help`.
