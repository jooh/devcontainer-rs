# Enhancements over upstream

These are additions to the pinned
[upstream Dev Containers CLI](https://github.com/devcontainers/cli).
Standard dev container behavior, including Features, Docker Compose, and
lifecycle commands, is described in the [usage guide](usage.md).

## Native distribution

The CLI ships as a Rust binary, so standalone, Homebrew, and Python package
installations do not require Node.js. Python wheels embed the executable;
the Python package does not expose a Python API. The npm packages are an
alternative wrapper around the same native executable.

## Persistent runtime defaults

Set environment variables once instead of repeating flags. Explicit CLI flags
take precedence; unset or blank variables keep the normal defaults.

For Podman:

```bash
export DEVCONTAINER_DOCKER_PATH=podman
export DEVCONTAINER_DOCKER_COMPOSE_PATH=podman-compose
devcontainer up --workspace-folder .
```

Engine paths are executable names or paths, not shell commands with arguments.
For example, use `podman-compose`, not `"podman compose"`.

| Environment variable | Equivalent flag |
| --- | --- |
| `DEVCONTAINER_CONFIG` | `--config` |
| `DEVCONTAINER_DOCKER_PATH` | `--docker-path` |
| `DEVCONTAINER_DOCKER_COMPOSE_PATH` | `--docker-compose-path` |
| `DEVCONTAINER_BUILDKIT` | `--buildkit` |
| `DEVCONTAINER_USER_DATA_FOLDER` | `--user-data-folder` |
| `DEVCONTAINER_CONTAINER_DATA_FOLDER` | `--container-data-folder` |
| `DEVCONTAINER_DOTFILES_REPOSITORY` | `--dotfiles-repository` |
| `DEVCONTAINER_DOTFILES_INSTALL_COMMAND` | `--dotfiles-install-command` |
| `DEVCONTAINER_DOTFILES_TARGET_PATH` | `--dotfiles-target-path` |
| `DEVCONTAINER_GPU_AVAILABILITY` | `--gpu-availability` |
| `DEVCONTAINER_UPDATE_REMOTE_USER_UID_DEFAULT` | `--update-remote-user-uid-default` |
| `DEVCONTAINER_MOUNT_WORKSPACE_GIT_ROOT` | `--mount-workspace-git-root` |
| `DEVCONTAINER_MOUNT_GIT_WORKTREE_COMMON_DIR` | `--mount-git-worktree-common-dir` |
| `DEVCONTAINER_WORKSPACE_MOUNT_CONSISTENCY` | `--workspace-mount-consistency` |

`DEVCONTAINER_CONFIG` resolves relative paths from `--workspace-folder`, or from
the current directory when no workspace is supplied. Use the same value for
`up` and `exec` to select the same configuration.

## Refresh images before starting

```bash
devcontainer up --workspace-folder . --pull-always
```

`--pull-always` refreshes remote source images and build bases. Compose projects
refresh the remote images needed by the selected services and their dependencies.
Existing containers can still be reused; add `--remove-existing-container` when
you also want to replace the container.

## Additional command options

The native CLI also adds these options to individual commands:

| Command | Addition |
| --- | --- |
| `build` | `--build-no-cache` as an alternative to `--no-cache` |
| `set-up` | `--workspace-folder` and `--docker-compose-path` |
| `exec` | `--secrets-file` for environment variables from a JSON file |
| `features test` | `--docker-path` to select the container engine |
| `features publish`, `templates publish` | `--output-dir` for a local OCI layout |
| `templates metadata` | `--workspace-folder` to resolve local OCI layouts |

Collection publishing currently writes local OCI layouts; it does not upload
them to a remote registry. See the [CLI reference](cli.md) for option syntax.
