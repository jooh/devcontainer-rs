# CLI reference

Generated from the native binary's `--help` output.

This describes the current development version of the CLI. An installed
release may differ; use its `--help` for the options available there.

## devcontainer

```text
devcontainer <command>

Commands:
  devcontainer up                   Create and run dev container
  devcontainer set-up               Set up an existing container as a dev container
  devcontainer build [path]         Build a dev container image
  devcontainer run-user-commands    Run user commands
  devcontainer read-configuration   Read configuration
  devcontainer outdated             Show current and available versions
  devcontainer upgrade              Upgrade lockfile
  devcontainer features             Features commands
  devcontainer templates            Templates commands
  devcontainer exec <cmd> [args..]  Execute a command on a running dev container

Options:
  --help                          Show help  [boolean]
  --version                       Show version number  [boolean]
  --oci-auth-hardening            Restrict OCI bearer authentication realms, registry credential forwarding, and token redirects.  [boolean] [default: false]
  --allow-cross-origin-auth-host  Allow an OCI registry to use a cross-origin HTTPS authentication host. Format: <registry-host>=<auth-host>. May be repeated.  [array]
```

## devcontainer up

```text
devcontainer up

Create and run dev container

Options:
  --help                            Show help  [boolean]
  --version                         Show version number  [boolean]
  --oci-auth-hardening              Restrict OCI bearer authentication realms, registry credential forwarding, and token redirects.  [boolean] [default: false]
  --allow-cross-origin-auth-host    Allow an OCI registry to use a cross-origin HTTPS authentication host. Format: <registry-host>=<auth-host>. May be repeated.  [array]
  --docker-path                     Docker CLI path.  [string]
  --docker-arg                      Global argument for the Docker CLI, inserted before the subcommand of every Docker CLI invocation (including the Docker Compose plugin, but not a separate --docker-compose-path executable). Repeat --docker-arg=VALUE for each argument.  [string]
  --docker-compose-path             Docker Compose CLI path.  [string]
  --container-data-folder           Container data folder where user data inside the container will be stored.  [string]
  --container-system-data-folder    Container system data folder where system data inside the container will be stored.  [string]
  --workspace-folder                Workspace folder path. The devcontainer.json will be looked up relative to this path. If --id-label, --override-config, and --workspace-folder are not provided, this defaults to the current directory.  [string]
  --workspace-mount-consistency     Workspace mount consistency.  [choices: "consistent", "cached", "delegated"] [default: "cached"]
  --gpu-availability                Availability of GPUs in case the dev container requires any. `all` expects a GPU to be available.  [choices: "all", "detect", "none"] [default: "detect"]
  --mount-workspace-git-root        Mount the workspace using its Git root.  [boolean] [default: true]
  --mount-git-worktree-common-dir   Mount the Git worktree common dir for Git operations to work in the container. This requires the worktree to be created with relative paths (`git worktree add --relative-paths`).  [boolean] [default: false]
  --id-label                        Id label(s) of the format name=value. These will be set on the container and used to query for an existing container. If no --id-label is given, one will be inferred from the --workspace-folder path.  [string]
  --config                          devcontainer.json path. The default is to use .devcontainer/devcontainer.json or, if that does not exist, .devcontainer.json in the workspace folder.  [string]
  --override-config                 devcontainer.json path to override any devcontainer.json in the workspace folder (or built-in configuration). This is required when there is no devcontainer.json otherwise.  [string]
  --log-level                       Log level for the --terminal-log-file. When set to trace, the log level for --log-file will also be set to trace.  [choices: "info", "debug", "trace"] [default: "info"]
  --log-format                      Log format.  [choices: "text", "json"] [default: "text"]
  --terminal-columns                Number of columns to render the output for. This is required for some of the subprocesses to correctly render their output.  [number]
  --terminal-rows                   Number of rows to render the output for. This is required for some of the subprocesses to correctly render their output.  [number]
  --default-user-env-probe          Default value for the devcontainer.json's "userEnvProbe".  [choices: "none", "loginInteractiveShell", "interactiveShell", "loginShell"] [default: "loginInteractiveShell"]
  --update-remote-user-uid-default  Default for updating the remote user's UID and GID to the local user's one.  [choices: "never", "on", "off"] [default: "on"]
  --remove-existing-container       Removes the dev container if it already exists.  [boolean] [default: false]
  --build-no-cache                  Builds the image with `--no-cache` if the container does not exist.  [boolean] [default: false]
  --expect-existing-container       Fail if the container does not exist.  [boolean] [default: false]
  --skip-post-create                Do not run onCreateCommand, updateContentCommand, postCreateCommand, postStartCommand or postAttachCommand and do not install dotfiles.  [boolean] [default: false]
  --skip-non-blocking-commands      Stop running user commands after running the command configured with waitFor or the updateContentCommand by default.  [boolean] [default: false]
  --prebuild                        Stop after onCreateCommand and updateContentCommand, rerunning updateContentCommand if it has run before.  [boolean] [default: false]
  --user-data-folder                Host path to a directory that is intended to be persisted and share state between sessions.  [string]
  --mount                           Additional mount point(s). Format: type=<bind|volume>,source=<source>,target=<target>[,external=<true|false>]  [string]
  --remote-env                      Remote environment variables of the format name=value. These will be added when executing the user commands.  [string]
  --cache-from                      Additional image to use as potential layer cache during image building  [string]
  --cache-to                        Additional image to use as potential layer cache during image building  [string]
  --buildkit                        Control whether BuildKit should be used  [choices: "auto", "never"] [default: "auto"]
  --additional-features             Additional features to apply to the dev container (JSON as per "features" section in devcontainer.json)  [string]
  --skip-post-attach                Do not run postAttachCommand.  [boolean] [default: false]
  --dotfiles-repository             URL of a dotfiles Git repository (e.g., https://github.com/owner/repository.git)  [string]
  --dotfiles-install-command        The command to run after cloning the dotfiles repository. Defaults to run the first file of `install.sh`, `install`, `bootstrap.sh`, `bootstrap`, `setup.sh` and `setup` found in the dotfiles repository`s root folder.  [string]
  --dotfiles-target-path            The path to clone the dotfiles repository to. Defaults to `~/dotfiles`.  [string] [default: "~/dotfiles"]
  --container-session-data-folder   Folder to cache CLI data, for example userEnvProbe results  [string]
  --secrets-file                    Path to a json file containing secret environment variables as key-value pairs.  [string]
  --no-lockfile                     Disable lockfile generation and verification.  [boolean] [default: false]
  --frozen-lockfile                 Ensure lockfile exists and remains unchanged; fail otherwise.  [boolean] [default: false]
  --include-configuration           Include configuration in result.  [boolean] [default: false]
  --include-merged-configuration    Include merged configuration in result.  [boolean] [default: false]
  --pull-always                     Always pull images before creating the dev container. Native extension.  [boolean]
```

## devcontainer set-up

```text
devcontainer set-up

Set up an existing container as a dev container

Options:
  --help                           Show help  [boolean]
  --version                        Show version number  [boolean]
  --oci-auth-hardening             Restrict OCI bearer authentication realms, registry credential forwarding, and token redirects.  [boolean] [default: false]
  --allow-cross-origin-auth-host   Allow an OCI registry to use a cross-origin HTTPS authentication host. Format: <registry-host>=<auth-host>. May be repeated.  [array]
  --docker-path                    Docker CLI path.  [string]
  --docker-arg                     Global argument for the Docker CLI, inserted before the subcommand of every Docker CLI invocation (including the Docker Compose plugin, but not a separate --docker-compose-path executable). Repeat --docker-arg=VALUE for each argument.  [string]
  --container-data-folder          Container data folder where user data inside the container will be stored.  [string]
  --container-system-data-folder   Container system data folder where system data inside the container will be stored.  [string]
  --container-id                   Id of the container.  [string] [required]
  --config                         devcontainer.json path.  [string]
  --log-level                      Log level for the --terminal-log-file. When set to trace, the log level for --log-file will also be set to trace.  [choices: "info", "debug", "trace"] [default: "info"]
  --log-format                     Log format.  [choices: "text", "json"] [default: "text"]
  --terminal-columns               Number of columns to render the output for. This is required for some of the subprocesses to correctly render their output.  [number]
  --terminal-rows                  Number of rows to render the output for. This is required for some of the subprocesses to correctly render their output.  [number]
  --default-user-env-probe         Default value for the devcontainer.json's "userEnvProbe".  [choices: "none", "loginInteractiveShell", "interactiveShell", "loginShell"] [default: "loginInteractiveShell"]
  --skip-post-create               Do not run onCreateCommand, updateContentCommand, postCreateCommand, postStartCommand or postAttachCommand and do not install dotfiles.  [boolean] [default: false]
  --skip-non-blocking-commands     Stop running user commands after running the command configured with waitFor or the updateContentCommand by default.  [boolean] [default: false]
  --user-data-folder               Host path to a directory that is intended to be persisted and share state between sessions.  [string]
  --remote-env                     Remote environment variables of the format name=value. These will be added when executing the user commands.  [string]
  --dotfiles-repository            URL of a dotfiles Git repository (e.g., https://github.com/owner/repository.git)  [string]
  --dotfiles-install-command       The command to run after cloning the dotfiles repository. Defaults to run the first file of `install.sh`, `install`, `bootstrap.sh`, `bootstrap`, `setup.sh` and `setup` found in the dotfiles repository`s root folder.  [string]
  --dotfiles-target-path           The path to clone the dotfiles repository to. Defaults to `~/dotfiles`.  [string] [default: "~/dotfiles"]
  --container-session-data-folder  Folder to cache CLI data, for example userEnvProbe results  [string]
  --include-configuration          Include configuration in result.  [boolean] [default: false]
  --include-merged-configuration   Include merged configuration in result.  [boolean] [default: false]
  --workspace-folder                Workspace folder used to resolve the devcontainer configuration. Native extension.  [string]
  --docker-compose-path             Docker Compose CLI path. Native extension.  [string]
```

## devcontainer build

```text
devcontainer build [path]

Build a dev container image

Options:
  --help                          Show help  [boolean]
  --version                       Show version number  [boolean]
  --oci-auth-hardening            Restrict OCI bearer authentication realms, registry credential forwarding, and token redirects.  [boolean] [default: false]
  --allow-cross-origin-auth-host  Allow an OCI registry to use a cross-origin HTTPS authentication host. Format: <registry-host>=<auth-host>. May be repeated.  [array]
  --user-data-folder              Host path to a directory that is intended to be persisted and share state between sessions.  [string]
  --docker-path                   Docker CLI path.  [string]
  --docker-arg                    Global argument for the Docker CLI, inserted before the subcommand of every Docker CLI invocation (including the Docker Compose plugin, but not a separate --docker-compose-path executable). Repeat --docker-arg=VALUE for each argument.  [string]
  --docker-compose-path           Docker Compose CLI path.  [string]
  --workspace-folder              Workspace folder path. The devcontainer.json will be looked up relative to this path. If not provided, defaults to the current directory.  [string]
  --config                        devcontainer.json path. The default is to use .devcontainer/devcontainer.json or, if that does not exist, .devcontainer.json in the workspace folder.  [string]
  --log-level                     Log level.  [choices: "info", "debug", "trace"] [default: "info"]
  --log-format                    Log format.  [choices: "text", "json"] [default: "text"]
  --no-cache                      Builds the image with `--no-cache`.  [boolean] [default: false]
  --image-name                    Image name.  [string]
  --cache-from                    Additional image to use as potential layer cache  [string]
  --cache-to                      A destination of buildx cache  [string]
  --buildkit                      Control whether BuildKit should be used  [choices: "auto", "never"] [default: "auto"]
  --platform                      Set target platforms.  [string]
  --push                          Push to a container registry.  [boolean] [default: false]
  --label                         Provide key and value configuration that adds metadata to an image  [string]
  --output                        Overrides the default behavior to load built images into the local docker registry. Valid options are the same ones provided to the --output option of docker buildx build.  [string]
  --additional-features           Additional features to apply to the dev container (JSON as per "features" section in devcontainer.json)  [string]
  --no-lockfile                   Disable lockfile generation and verification.  [boolean] [default: false]
  --frozen-lockfile               Ensure lockfile exists and remains unchanged; fail otherwise.  [boolean] [default: false]
  --build-no-cache                  Build without using cached layers. Native extension.  [boolean]
```

## devcontainer run-user-commands

```text
devcontainer run-user-commands

Run user commands

Options:
  --help                           Show help  [boolean]
  --version                        Show version number  [boolean]
  --oci-auth-hardening             Restrict OCI bearer authentication realms, registry credential forwarding, and token redirects.  [boolean] [default: false]
  --allow-cross-origin-auth-host   Allow an OCI registry to use a cross-origin HTTPS authentication host. Format: <registry-host>=<auth-host>. May be repeated.  [array]
  --user-data-folder               Host path to a directory that is intended to be persisted and share state between sessions.  [string]
  --docker-path                    Docker CLI path.  [string]
  --docker-arg                     Global argument for the Docker CLI, inserted before the subcommand of every Docker CLI invocation (including the Docker Compose plugin, but not a separate --docker-compose-path executable). Repeat --docker-arg=VALUE for each argument.  [string]
  --docker-compose-path            Docker Compose CLI path.  [string]
  --container-data-folder          Container data folder where user data inside the container will be stored.  [string]
  --container-system-data-folder   Container system data folder where system data inside the container will be stored.  [string]
  --workspace-folder               Workspace folder path.The devcontainer.json will be looked up relative to this path. If --container-id, --id-label, and --workspace-folder are not provided, this defaults to the current directory.  [string]
  --mount-workspace-git-root       Mount the workspace using its Git root.  [boolean] [default: true]
  --mount-git-worktree-common-dir  Mount the Git worktree common dir for Git operations to work in the container. This requires the worktree to be created with relative paths (`git worktree add --relative-paths`).  [boolean] [default: false]
  --container-id                   Id of the container to run the user commands for.  [string]
  --id-label                       Id label(s) of the format name=value. If no --container-id is given the id labels will be used to look up the container. If no --id-label is given, one will be inferred from the --workspace-folder path.  [string]
  --config                         devcontainer.json path. The default is to use .devcontainer/devcontainer.json or, if that does not exist, .devcontainer.json in the workspace folder.  [string]
  --override-config                devcontainer.json path to override any devcontainer.json in the workspace folder (or built-in configuration). This is required when there is no devcontainer.json otherwise.  [string]
  --log-level                      Log level for the --terminal-log-file. When set to trace, the log level for --log-file will also be set to trace.  [choices: "info", "debug", "trace"] [default: "info"]
  --log-format                     Log format.  [choices: "text", "json"] [default: "text"]
  --terminal-columns               Number of columns to render the output for. This is required for some of the subprocesses to correctly render their output.  [number]
  --terminal-rows                  Number of rows to render the output for. This is required for some of the subprocesses to correctly render their output.  [number]
  --default-user-env-probe         Default value for the devcontainer.json's "userEnvProbe".  [choices: "none", "loginInteractiveShell", "interactiveShell", "loginShell"] [default: "loginInteractiveShell"]
  --skip-non-blocking-commands     Stop running user commands after running the command configured with waitFor or the updateContentCommand by default.  [boolean] [default: false]
  --prebuild                       Stop after onCreateCommand and updateContentCommand, rerunning updateContentCommand if it has run before.  [boolean] [default: false]
  --stop-for-personalization       Stop for personalization.  [boolean] [default: false]
  --remote-env                     Remote environment variables of the format name=value. These will be added when executing the user commands.  [string]
  --skip-post-attach               Do not run postAttachCommand.  [boolean] [default: false]
  --dotfiles-repository            URL of a dotfiles Git repository (e.g., https://github.com/owner/repository.git)  [string]
  --dotfiles-install-command       The command to run after cloning the dotfiles repository. Defaults to run the first file of `install.sh`, `install`, `bootstrap.sh`, `bootstrap`, `setup.sh` and `setup` found in the dotfiles repository`s root folder.  [string]
  --dotfiles-target-path           The path to clone the dotfiles repository to. Defaults to `~/dotfiles`.  [string] [default: "~/dotfiles"]
  --container-session-data-folder  Folder to cache CLI data, for example userEnvProbe results  [string]
  --secrets-file                   Path to a json file containing secret environment variables as key-value pairs.  [string]
```

## devcontainer read-configuration

```text
devcontainer read-configuration

Read configuration

Options:
  --help                            Show help  [boolean]
  --version                         Show version number  [boolean]
  --oci-auth-hardening              Restrict OCI bearer authentication realms, registry credential forwarding, and token redirects.  [boolean] [default: false]
  --allow-cross-origin-auth-host    Allow an OCI registry to use a cross-origin HTTPS authentication host. Format: <registry-host>=<auth-host>. May be repeated.  [array]
  --user-data-folder                Host path to a directory that is intended to be persisted and share state between sessions.  [string]
  --docker-path                     Docker CLI path.  [string]
  --docker-arg                      Global argument for the Docker CLI, inserted before the subcommand of every Docker CLI invocation (including the Docker Compose plugin, but not a separate --docker-compose-path executable). Repeat --docker-arg=VALUE for each argument.  [string]
  --docker-compose-path             Docker Compose CLI path.  [string]
  --workspace-folder                Workspace folder path. The devcontainer.json will be looked up relative to this path. If --container-id, --id-label, and --workspace-folder are not provided, this defaults to the current directory.  [string]
  --mount-workspace-git-root        Mount the workspace using its Git root.  [boolean] [default: true]
  --mount-git-worktree-common-dir   Mount the Git worktree common dir for Git operations to work in the container. This requires the worktree to be created with relative paths (`git worktree add --relative-paths`).  [boolean] [default: false]
  --container-id                    Id of the container to run the user commands for.  [string]
  --id-label                        Id label(s) of the format name=value. If no --container-id is given the id labels will be used to look up the container. If no --id-label is given, one will be inferred from the --workspace-folder path.  [string]
  --config                          devcontainer.json path. The default is to use .devcontainer/devcontainer.json or, if that does not exist, .devcontainer.json in the workspace folder.  [string]
  --override-config                 devcontainer.json path to override any devcontainer.json in the workspace folder (or built-in configuration). This is required when there is no devcontainer.json otherwise.  [string]
  --log-level                       Log level for the --terminal-log-file. When set to trace, the log level for --log-file will also be set to trace.  [choices: "info", "debug", "trace"] [default: "info"]
  --log-format                      Log format.  [choices: "text", "json"] [default: "text"]
  --terminal-columns                Number of columns to render the output for. This is required for some of the subprocesses to correctly render their output.  [number]
  --terminal-rows                   Number of rows to render the output for. This is required for some of the subprocesses to correctly render their output.  [number]
  --include-features-configuration  Include features configuration.  [boolean] [default: false]
  --include-merged-configuration    Include merged configuration.  [boolean] [default: false]
  --additional-features             Additional features to apply to the dev container (JSON as per "features" section in devcontainer.json)  [string]
```

## devcontainer outdated

```text
devcontainer outdated

Show current and available versions

Options:
  --help                          Show help  [boolean]
  --version                       Show version number  [boolean]
  --oci-auth-hardening            Restrict OCI bearer authentication realms, registry credential forwarding, and token redirects.  [boolean] [default: false]
  --allow-cross-origin-auth-host  Allow an OCI registry to use a cross-origin HTTPS authentication host. Format: <registry-host>=<auth-host>. May be repeated.  [array]
  --user-data-folder              Host path to a directory that is intended to be persisted and share state between sessions.  [string]
  --workspace-folder              Workspace folder path. The devcontainer.json will be looked up relative to this path. If --workspace-folder is not provided, defaults to the current directory.  [string]
  --config                        devcontainer.json path. The default is to use .devcontainer/devcontainer.json or, if that does not exist, .devcontainer.json in the workspace folder.  [string]
  --output-format                 Output format.  [choices: "text", "json"] [default: "text"]
  --log-level                     Log level for the --terminal-log-file. When set to trace, the log level for --log-file will also be set to trace.  [choices: "info", "debug", "trace"] [default: "info"]
  --log-format                    Log format.  [choices: "text", "json"] [default: "text"]
  --terminal-columns              Number of columns to render the output for. This is required for some of the subprocesses to correctly render their output.  [number]
  --terminal-rows                 Number of rows to render the output for. This is required for some of the subprocesses to correctly render their output.  [number]
```

## devcontainer upgrade

```text
devcontainer upgrade

Upgrade lockfile

Options:
  --help                          Show help  [boolean]
  --version                       Show version number  [boolean]
  --oci-auth-hardening            Restrict OCI bearer authentication realms, registry credential forwarding, and token redirects.  [boolean] [default: false]
  --allow-cross-origin-auth-host  Allow an OCI registry to use a cross-origin HTTPS authentication host. Format: <registry-host>=<auth-host>. May be repeated.  [array]
  --workspace-folder              Workspace folder. If --workspace-folder is not provided defaults to the current directory.  [string]
  --docker-path                   Path to docker executable.  [string] [default: "docker"]
  --docker-arg                    Global argument for the Docker CLI, inserted before the subcommand of every Docker CLI invocation (including the Docker Compose plugin, but not a separate --docker-compose-path executable). Repeat --docker-arg=VALUE for each argument.  [string]
  --docker-compose-path           Path to docker-compose executable.  [string] [default: "docker-compose"]
  --config                        devcontainer.json path. The default is to use .devcontainer/devcontainer.json or, if that does not exist, .devcontainer.json in the workspace folder.  [string]
  --log-level                     Log level.  [choices: "error", "info", "debug", "trace"] [default: "info"]
  --dry-run                       Write generated lockfile to standard out instead of to disk.  [boolean]
```

## devcontainer features

```text
devcontainer features

Features commands

Commands:
  devcontainer features test [target]          Test Features
  devcontainer features package <target>       Package Features
  devcontainer features publish <target>       Package and publish Features
  devcontainer features info <mode> <feature>  Fetch metadata for a published Feature
  devcontainer features resolve-dependencies   Read and resolve dependency graph from a configuration
  devcontainer features generate-docs          Generate documentation

Options:
  --help                          Show help  [boolean]
  --version                       Show version number  [boolean]
  --oci-auth-hardening            Restrict OCI bearer authentication realms, registry credential forwarding, and token redirects.  [boolean] [default: false]
  --allow-cross-origin-auth-host  Allow an OCI registry to use a cross-origin HTTPS authentication host. Format: <registry-host>=<auth-host>. May be repeated.  [array]
```

## devcontainer features test

```text
devcontainer features test [target]

Test Features

Positionals:
  target  Path to folder containing 'src' and 'test' sub-folders.  [deprecated: Use --project-folder instead] [string] [default: "."]

Options:
      --help                          Show help  [boolean]
      --version                       Show version number  [boolean]
      --oci-auth-hardening            Restrict OCI bearer authentication realms, registry credential forwarding, and token redirects.  [boolean] [default: false]
      --allow-cross-origin-auth-host  Allow an OCI registry to use a cross-origin HTTPS authentication host. Format: <registry-host>=<auth-host>. May be repeated.  [array]
  -p, --project-folder                Path to folder containing 'src' and 'test' sub-folders. This is likely the git root of the project.  [string] [default: "."]
  -f, --features                      Feature(s) to test as space-separated parameters. Omit to run all tests.  Cannot be combined with '--global-scenarios-only'.  [array]
      --filter                        Filter current tests to only run scenarios containing this string.  Cannot be combined with '--skip-scenarios'.  [string]
      --global-scenarios-only         Run only scenario tests under 'tests/_global' .  Cannot be combined with '-f'.  [boolean] [default: false]
      --skip-scenarios                Skip all 'scenario' style tests.  Cannot be combined with '--global--scenarios-only'.  [boolean] [default: false]
      --skip-autogenerated            Skip all 'autogenerated' style tests (test.sh).  [boolean] [default: false]
      --skip-duplicated               Skip all 'duplicate' style tests (duplicate.sh).  [boolean] [default: false]
      --permit-randomization          Allow an element of randomness in test cases.  [boolean] [default: false]
  -i, --base-image                    Base Image. Not used for scenarios.  [string] [default: "ubuntu:focal"]
  -u, --remote-user                   Remote user.  Not used for scenarios.  [string]
      --log-level                     Log level.  [choices: "info", "debug", "trace"] [default: "info"]
      --preserve-test-containers      Do not remove test containers after running tests.  [boolean] [default: false]
  -q, --quiet                         Quiets output  [boolean] [default: false]
  --docker-path                     Container engine CLI path. Native extension.  [string]
```

## devcontainer features package

```text
devcontainer features package <target>

Package Features

Positionals:
  target
           Package features at provided [target] (default is cwd), where [target] is either:
              1. A path to the src folder of the collection with [1..n] features.
              2. A path to a single feature that contains a devcontainer-feature.json.

              Additionally, a 'devcontainer-collection.json' will be generated in the output directory.
  [string] [required] [default: "."]

Options:
      --help                          Show help  [boolean]
      --version                       Show version number  [boolean]
      --oci-auth-hardening            Restrict OCI bearer authentication realms, registry credential forwarding, and token redirects.  [boolean] [default: false]
      --allow-cross-origin-auth-host  Allow an OCI registry to use a cross-origin HTTPS authentication host. Format: <registry-host>=<auth-host>. May be repeated.  [array]
  -o, --output-folder                 Path to output directory. Will create directories as needed.  [string] [default: "./output"]
  -f, --force-clean-output-folder     Automatically delete previous output directory before packaging  [boolean] [default: false]
      --log-level                     Log level.  [choices: "info", "debug", "trace"] [default: "info"]
```

## devcontainer features publish

```text
devcontainer features publish <target>

Package and publish Features

Positionals:
  target
           Package and publish features at provided [target] (default is cwd), where [target] is either:
              1. A path to the src folder of the collection with [1..n] features.
              2. A path to a single feature that contains a devcontainer-feature.json.
  [string] [required] [default: "."]

Options:
      --help                          Show help  [boolean]
      --version                       Show version number  [boolean]
      --oci-auth-hardening            Restrict OCI bearer authentication realms, registry credential forwarding, and token redirects.  [boolean] [default: false]
      --allow-cross-origin-auth-host  Allow an OCI registry to use a cross-origin HTTPS authentication host. Format: <registry-host>=<auth-host>. May be repeated.  [array]
  -r, --registry                      Name of the OCI registry.  [string] [default: "ghcr.io"]
  -n, --namespace                     Unique indentifier for the collection of features. Example: <owner>/<repo>  [string] [required]
      --log-level                     Log level.  [choices: "info", "debug", "trace"] [default: "info"]
  --output-dir                      Directory for the generated local OCI layout. Native extension.  [string]
```

## devcontainer features info

```text
devcontainer features info <mode> <feature>

Fetch metadata for a published Feature

Positionals:
  mode     Data to query. Select 'verbose' to return everything.  [required] [choices: "manifest", "tags", "dependencies", "verbose"]
  feature  Feature Identifier  [string] [required]

Options:
  --help                          Show help  [boolean]
  --version                       Show version number  [boolean]
  --oci-auth-hardening            Restrict OCI bearer authentication realms, registry credential forwarding, and token redirects.  [boolean] [default: false]
  --allow-cross-origin-auth-host  Allow an OCI registry to use a cross-origin HTTPS authentication host. Format: <registry-host>=<auth-host>. May be repeated.  [array]
  --log-level                     Log level.  [choices: "info", "debug", "trace"] [default: "info"]
  --output-format                 Output format.  [choices: "text", "json"] [default: "text"]
```

## devcontainer features resolve-dependencies

```text
devcontainer features resolve-dependencies

Read and resolve dependency graph from a configuration

Options:
  --help                          Show help  [boolean]
  --version                       Show version number  [boolean]
  --oci-auth-hardening            Restrict OCI bearer authentication realms, registry credential forwarding, and token redirects.  [boolean] [default: false]
  --allow-cross-origin-auth-host  Allow an OCI registry to use a cross-origin HTTPS authentication host. Format: <registry-host>=<auth-host>. May be repeated.  [array]
  --log-level                     Log level.  [choices: "error", "info", "debug", "trace"] [default: "error"]
  --workspace-folder              Workspace folder to use for the configuration. If --workspace-folder is not provided, this defaults to the current directory  [string]
```

## devcontainer features generate-docs

```text
devcontainer features generate-docs

Generate documentation

Options:
      --help                          Show help  [boolean]
      --version                       Show version number  [boolean]
      --oci-auth-hardening            Restrict OCI bearer authentication realms, registry credential forwarding, and token redirects.  [boolean] [default: false]
      --allow-cross-origin-auth-host  Allow an OCI registry to use a cross-origin HTTPS authentication host. Format: <registry-host>=<auth-host>. May be repeated.  [array]
  -p, --project-folder                Path to folder containing 'src' and 'test' sub-folders. This is likely the git root of the project.  [string] [default: "."]
  -r, --registry                      Name of the OCI registry.  [string] [default: "ghcr.io"]
  -n, --namespace                     Unique indentifier for the collection of features. Example: <owner>/<repo>  [string] [required]
      --github-owner                  GitHub owner for docs.  [string] [default: ""]
      --github-repo                   GitHub repo for docs.  [string] [default: ""]
      --log-level                     Log level.  [choices: "info", "debug", "trace"] [default: "info"]
```

## devcontainer templates

```text
devcontainer templates

Templates commands

Commands:
  devcontainer templates apply                  Apply a template to the project
  devcontainer templates publish <target>       Package and publish templates
  devcontainer templates metadata <templateId>  Fetch a published Template's metadata
  devcontainer templates generate-docs          Generate documentation

Options:
  --help                          Show help  [boolean]
  --version                       Show version number  [boolean]
  --oci-auth-hardening            Restrict OCI bearer authentication realms, registry credential forwarding, and token redirects.  [boolean] [default: false]
  --allow-cross-origin-auth-host  Allow an OCI registry to use a cross-origin HTTPS authentication host. Format: <registry-host>=<auth-host>. May be repeated.  [array]
```

## devcontainer templates apply

```text
devcontainer templates apply

Apply a template to the project

Options:
      --help                          Show help  [boolean]
      --version                       Show version number  [boolean]
      --oci-auth-hardening            Restrict OCI bearer authentication realms, registry credential forwarding, and token redirects.  [boolean] [default: false]
      --allow-cross-origin-auth-host  Allow an OCI registry to use a cross-origin HTTPS authentication host. Format: <registry-host>=<auth-host>. May be repeated.  [array]
  -w, --workspace-folder              Target workspace folder to apply Template. If --workspace-folder is not provided, this defaults to the current directory  [string]
  -t, --template-id                   Reference to a Template in a supported OCI registry  [string] [required]
  -a, --template-args                 Arguments to replace within the provided Template, provided as JSON  [string] [default: "{}"]
  -f, --features                      Features to add to the provided Template, provided as JSON.  [string] [default: "[]"]
      --log-level                     Log level.  [choices: "info", "debug", "trace"] [default: "info"]
      --tmp-dir                       Directory to use for temporary files. If not provided, the system default will be inferred.  [string]
      --omit-paths                    List of paths within the Template to omit applying, provided as JSON.  To ignore a directory append '/*'. Eg: '[".github/*", "dir/a/*", "file.ts"]'  [string] [default: "[]"]
```

## devcontainer templates publish

```text
devcontainer templates publish <target>

Package and publish templates

Positionals:
  target
           Package and publish templates at provided [target] (default is cwd), where [target] is either:
              1. A path to the src folder of the collection with [1..n] templates.
              2. A path to a single template that contains a devcontainer-template.json.
  [string] [required] [default: "."]

Options:
      --help                          Show help  [boolean]
      --version                       Show version number  [boolean]
      --oci-auth-hardening            Restrict OCI bearer authentication realms, registry credential forwarding, and token redirects.  [boolean] [default: false]
      --allow-cross-origin-auth-host  Allow an OCI registry to use a cross-origin HTTPS authentication host. Format: <registry-host>=<auth-host>. May be repeated.  [array]
  -r, --registry                      Name of the OCI registry.  [string] [default: "ghcr.io"]
  -n, --namespace                     Unique indentifier for the collection of templates. Example: <owner>/<repo>  [string] [required]
      --log-level                     Log level.  [choices: "info", "debug", "trace"] [default: "info"]
  --output-dir                      Directory for the generated local OCI layout. Native extension.  [string]
```

## devcontainer templates metadata

```text
devcontainer templates metadata <templateId>

Fetch a published Template's metadata

Positionals:
  templateId  Template Identifier  [string] [required]

Options:
  --help                          Show help  [boolean]
  --version                       Show version number  [boolean]
  --oci-auth-hardening            Restrict OCI bearer authentication realms, registry credential forwarding, and token redirects.  [boolean] [default: false]
  --allow-cross-origin-auth-host  Allow an OCI registry to use a cross-origin HTTPS authentication host. Format: <registry-host>=<auth-host>. May be repeated.  [array]
  --log-level                     Log level.  [choices: "info", "debug", "trace"] [default: "info"]
  --workspace-folder                Workspace folder used to resolve local OCI layouts. Native extension.  [string]
```

## devcontainer templates generate-docs

```text
devcontainer templates generate-docs

Generate documentation

Options:
      --help                          Show help  [boolean]
      --version                       Show version number  [boolean]
      --oci-auth-hardening            Restrict OCI bearer authentication realms, registry credential forwarding, and token redirects.  [boolean] [default: false]
      --allow-cross-origin-auth-host  Allow an OCI registry to use a cross-origin HTTPS authentication host. Format: <registry-host>=<auth-host>. May be repeated.  [array]
  -p, --project-folder                Path to folder containing 'src' and 'test' sub-folders. This is likely the git root of the project.  [string] [default: "."]
      --github-owner                  GitHub owner for docs.  [string] [default: ""]
      --github-repo                   GitHub repo for docs.  [string] [default: ""]
      --log-level                     Log level.  [choices: "info", "debug", "trace"] [default: "info"]
```

## devcontainer exec

```text
devcontainer exec <cmd> [args..]

Execute a command on a running dev container

Positionals:
  cmd   Command to execute.  [string] [required]
  args  Arguments to the command.  [array] [default: []]

Options:
  --help                           Show help  [boolean]
  --version                        Show version number  [boolean]
  --oci-auth-hardening             Restrict OCI bearer authentication realms, registry credential forwarding, and token redirects.  [boolean] [default: false]
  --allow-cross-origin-auth-host   Allow an OCI registry to use a cross-origin HTTPS authentication host. Format: <registry-host>=<auth-host>. May be repeated.  [array]
  --user-data-folder               Host path to a directory that is intended to be persisted and share state between sessions.  [string]
  --docker-path                    Docker CLI path.  [string]
  --docker-arg                     Global argument for the Docker CLI, inserted before the subcommand of every Docker CLI invocation (including the Docker Compose plugin, but not a separate --docker-compose-path executable). Repeat --docker-arg=VALUE for each argument.  [string]
  --docker-compose-path            Docker Compose CLI path.  [string]
  --container-data-folder          Container data folder where user data inside the container will be stored.  [string]
  --container-system-data-folder   Container system data folder where system data inside the container will be stored.  [string]
  --workspace-folder               Workspace folder path. The devcontainer.json will be looked up relative to this path. If --container-id, --id-label, and --workspace-folder are not provided, this defaults to the current directory.  [string]
  --mount-workspace-git-root       Mount the workspace using its Git root.  [boolean] [default: true]
  --mount-git-worktree-common-dir  Mount the Git worktree common dir for Git operations to work in the container. This requires the worktree to be created with relative paths (`git worktree add --relative-paths`).  [boolean] [default: false]
  --container-id                   Id of the container to run the user commands for.  [string]
  --id-label                       Id label(s) of the format name=value. If no --container-id is given the id labels will be used to look up the container. If no --id-label is given, one will be inferred from the --workspace-folder path.  [string]
  --config                         devcontainer.json path. The default is to use .devcontainer/devcontainer.json or, if that does not exist, .devcontainer.json in the workspace folder.  [string]
  --override-config                devcontainer.json path to override any devcontainer.json in the workspace folder (or built-in configuration). This is required when there is no devcontainer.json otherwise.  [string]
  --log-level                      Log level for the --terminal-log-file. When set to trace, the log level for --log-file will also be set to trace.  [choices: "info", "debug", "trace"] [default: "info"]
  --log-format                     Log format.  [choices: "text", "json"] [default: "text"]
  --terminal-columns               Number of columns to render the output for. This is required for some of the subprocesses to correctly render their output.  [number]
  --terminal-rows                  Number of rows to render the output for. This is required for some of the subprocesses to correctly render their output.  [number]
  --default-user-env-probe         Default value for the devcontainer.json's "userEnvProbe".  [choices: "none", "loginInteractiveShell", "interactiveShell", "loginShell"] [default: "loginInteractiveShell"]
  --remote-env                     Remote environment variables of the format name=value. These will be added when executing the user commands.  [string]
  --secrets-file                    Path to a JSON file containing secret environment variables. Native extension.  [string]
```
