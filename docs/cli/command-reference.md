# CLI Command Reference

Generated from the pinned upstream CLI command matrix.

- Upstream commit: `2d81ee3c9ed96a7312c18c7513a17933f8f66d41`
- Source: `upstream/src/spec-node/devContainersSpecCLI.ts`

## Top-Level Commands

| Command | Description |
| --- | --- |
| `up` | Create and run dev container |
| `set-up` | Set up an existing container as a dev container |
| `build` | Build a dev container image |
| `run-user-commands` | Run user commands |
| `read-configuration` | Read configuration |
| `outdated` | Show current and available versions |
| `upgrade` | Upgrade lockfile |
| `features` | Features commands |
| `templates` | Templates commands |
| `exec` | Execute a command on a running dev container |

## Detailed Reference

### `up`

Create and run dev container

Options:
- `--additional-features`
- `--build-no-cache`
- `--buildkit`
- `--cache-from`
- `--cache-to`
- `--config`
- `--container-data-folder`
- `--container-session-data-folder`
- `--container-system-data-folder`
- `--default-user-env-probe`
- `--docker-compose-path`
- `--docker-path`
- `--dotfiles-install-command`
- `--dotfiles-repository`
- `--dotfiles-target-path`
- `--expect-existing-container`
- `--experimental-frozen-lockfile`
- `--experimental-lockfile`
- `--gpu-availability`
- `--id-label`
- `--include-configuration`
- `--include-merged-configuration`
- `--log-format`
- `--log-level`
- `--mount`
- `--mount-git-worktree-common-dir`
- `--mount-workspace-git-root`
- `--omit-config-remote-env-from-metadata`
- `--omit-syntax-directive`
- `--override-config`
- `--remote-env`
- `--remove-existing-container`
- `--secrets-file`
- `--skip-feature-auto-mapping`
- `--skip-non-blocking-commands`
- `--skip-post-attach`
- `--skip-post-create`
- `--terminal-columns`
- `--terminal-rows`
- `--update-remote-user-uid-default`
- `--user-data-folder`
- `--workspace-folder`
- `--workspace-mount-consistency`

### `set-up`

Set up an existing container as a dev container

Options:
- `--config`
- `--container-data-folder`
- `--container-id`
- `--container-session-data-folder`
- `--container-system-data-folder`
- `--default-user-env-probe`
- `--docker-path`
- `--dotfiles-install-command`
- `--dotfiles-repository`
- `--dotfiles-target-path`
- `--include-configuration`
- `--include-merged-configuration`
- `--log-format`
- `--log-level`
- `--remote-env`
- `--skip-non-blocking-commands`
- `--skip-post-create`
- `--terminal-columns`
- `--terminal-rows`
- `--user-data-folder`

### `build`

Build a dev container image

Options:
- `--additional-features`
- `--buildkit`
- `--cache-from`
- `--cache-to`
- `--config`
- `--docker-compose-path`
- `--docker-path`
- `--experimental-frozen-lockfile`
- `--experimental-lockfile`
- `--image-name`
- `--label`
- `--log-format`
- `--log-level`
- `--no-cache`
- `--omit-syntax-directive`
- `--output`
- `--platform`
- `--push`
- `--skip-feature-auto-mapping`
- `--skip-persisting-customizations-from-features`
- `--user-data-folder`
- `--workspace-folder`

### `run-user-commands`

Run user commands

Options:
- `--config`
- `--container-data-folder`
- `--container-id`
- `--container-session-data-folder`
- `--container-system-data-folder`
- `--default-user-env-probe`
- `--docker-compose-path`
- `--docker-path`
- `--dotfiles-install-command`
- `--dotfiles-repository`
- `--dotfiles-target-path`
- `--id-label`
- `--log-format`
- `--log-level`
- `--mount-git-worktree-common-dir`
- `--mount-workspace-git-root`
- `--override-config`
- `--remote-env`
- `--secrets-file`
- `--skip-feature-auto-mapping`
- `--skip-non-blocking-commands`
- `--skip-post-attach`
- `--stop-for-personalization`
- `--terminal-columns`
- `--terminal-rows`
- `--user-data-folder`
- `--workspace-folder`

### `read-configuration`

Read configuration

Options:
- `--additional-features`
- `--config`
- `--container-id`
- `--docker-compose-path`
- `--docker-path`
- `--id-label`
- `--include-features-configuration`
- `--include-merged-configuration`
- `--log-format`
- `--log-level`
- `--mount-git-worktree-common-dir`
- `--mount-workspace-git-root`
- `--override-config`
- `--skip-feature-auto-mapping`
- `--terminal-columns`
- `--terminal-rows`
- `--user-data-folder`
- `--workspace-folder`

### `outdated`

Show current and available versions

Options:
- `--config`
- `--log-format`
- `--log-level`
- `--output-format`
- `--terminal-columns`
- `--terminal-rows`
- `--user-data-folder`
- `--workspace-folder`

### `upgrade`

Upgrade lockfile

Options:
- `--config`
- `--docker-compose-path`
- `--docker-path`
- `--dry-run`
- `--feature`
- `--log-level`
- `--target-version`
- `--workspace-folder`

### `features`

Features commands

Options:
- None

### `templates`

Templates commands

Options:
- None

### `exec`

Execute a command on a running dev container

Options:
- `--config`
- `--container-data-folder`
- `--container-id`
- `--container-system-data-folder`
- `--default-user-env-probe`
- `--docker-compose-path`
- `--docker-path`
- `--id-label`
- `--log-format`
- `--log-level`
- `--mount-git-worktree-common-dir`
- `--mount-workspace-git-root`
- `--override-config`
- `--remote-env`
- `--skip-feature-auto-mapping`
- `--terminal-columns`
- `--terminal-rows`
- `--user-data-folder`
- `--workspace-folder`

## `features` Subcommands

### `features test`

Test Features

Options:
- `--base-image`
- `--features`
- `--filter`
- `--global-scenarios-only`
- `--log-level`
- `--permit-randomization`
- `--preserve-test-containers`
- `--project-folder`
- `--quiet`
- `--remote-user`
- `--skip-autogenerated`
- `--skip-duplicated`
- `--skip-scenarios`

### `features package`

Package Features

Options:
- None

### `features publish`

Package and publish Features

Options:
- None

### `features info`

Fetch metadata for a published Feature

Options:
- `--log-level`
- `--output-format`

### `features resolve-dependencies`

Read and resolve dependency graph from a configuration

Options:
- `--log-level`
- `--workspace-folder`

### `features generate-docs`

Generate documentation

Options:
- `--github-owner`
- `--github-repo`
- `--log-level`
- `--namespace`
- `--project-folder`
- `--registry`

## `templates` Subcommands

### `templates apply`

Apply a template to the project

Options:
- `--features`
- `--log-level`
- `--omit-paths`
- `--template-args`
- `--template-id`
- `--tmp-dir`
- `--workspace-folder`

### `templates publish`

Package and publish templates

Options:
- None

### `templates metadata`

Fetch a published Template\'s metadata

Options:
- `--log-level`

### `templates generate-docs`

Generate documentation

Options:
- `--github-owner`
- `--github-repo`
- `--log-level`
- `--project-folder`

