# Native Parity Inventory

Generated from the pinned upstream CLI command matrix and static source evidence in the Rust implementation.

- Upstream commit: `2d81ee3c9ed96a7312c18c7513a17933f8f66d41`
- Source: `upstream/src/spec-node/devContainersSpecCLI.ts`
- Declared upstream command paths present natively: `20/20`
- Upstream options with a native source reference in mapped files: `200/200`

This report is a static inventory, not a semantic parity proof. A referenced option can still be only partially implemented, and command-level known gaps are called out explicitly below.

## Summary

| Command | Declared | Option refs | Missing refs | Known gaps |
| --- | --- | --- | --- | --- |
| `up` | yes | 43/43 | 0 | 2 |
| `set-up` | yes | 20/20 | 0 | 1 |
| `build` | yes | 22/22 | 0 | 2 |
| `run-user-commands` | yes | 27/27 | 0 | 1 |
| `read-configuration` | yes | 18/18 | 0 | 2 |
| `outdated` | yes | 8/8 | 0 | 1 |
| `upgrade` | yes | 8/8 | 0 | 1 |
| `features` | yes | 0/0 | 0 | 1 |
| `features test` | yes | 13/13 | 0 | 1 |
| `features package` | yes | 0/0 | 0 | 1 |
| `features publish` | yes | 0/0 | 0 | 1 |
| `features info` | yes | 2/2 | 0 | 1 |
| `features resolve-dependencies` | yes | 2/2 | 0 | 1 |
| `features generate-docs` | yes | 6/6 | 0 | 1 |
| `templates` | yes | 0/0 | 0 | 1 |
| `templates apply` | yes | 7/7 | 0 | 1 |
| `templates publish` | yes | 0/0 | 0 | 1 |
| `templates metadata` | yes | 1/1 | 0 | 1 |
| `templates generate-docs` | yes | 4/4 | 0 | 1 |
| `exec` | yes | 19/19 | 0 | 1 |

## `up`

- Description: Create and run dev container
- Declared natively: yes
- Option source references: 43/43
- Missing option references: none
- Known gaps: Native runtime now layers Features for image, dockerfile, and Docker Compose configs. Several upstream flags remain unimplemented or are only partially honored.

## `set-up`

- Description: Set up an existing container as a dev container
- Declared natively: yes
- Option source references: 20/20
- Missing option references: none
- Known gaps: Lifecycle execution is native, but several upstream setup and dotfiles flags are still missing.

## `build`

- Description: Build a dev container image
- Declared natively: yes
- Option source references: 22/22
- Missing option references: none
- Known gaps: Native runtime now layers Features for image, dockerfile, and Docker Compose configs. Several upstream build flags are still unimplemented or are only partially honored.

## `run-user-commands`

- Description: Run user commands
- Declared natively: yes
- Option source references: 27/27
- Missing option references: none
- Known gaps: Lifecycle execution is native, but several upstream runtime and dotfiles flags are still missing.

## `read-configuration`

- Description: Read configuration
- Declared natively: yes
- Option source references: 18/18
- Missing option references: none
- Known gaps: `--include-features-configuration` resolves local/published Feature sets natively, but still relies on fixture/manual manifests rather than full OCI resolution. Variable substitution support is still narrower than upstream.

## `outdated`

- Description: Show current and available versions
- Declared natively: yes
- Option source references: 8/8
- Missing option references: none
- Known gaps: Backed by fixture/manual catalog data rather than real upstream registry resolution.

## `upgrade`

- Description: Upgrade lockfile
- Declared natively: yes
- Option source references: 8/8
- Missing option references: none
- Known gaps: Backed by fixture/manual catalog data rather than real upstream registry resolution.

## `features`

- Description: Features commands
- Declared natively: yes
- Option source references: 0/0
- Missing option references: none
- Known gaps: Top-level command exists, but several subcommands still use local/offline substitutes rather than real OCI flows.

## `features test`

- Description: Test Features
- Declared natively: yes
- Option source references: 13/13
- Missing option references: none
- Known gaps: Native test runner exists, but parity with upstream feature resolution and registry-backed dependencies is incomplete.

## `features package`

- Description: Package Features
- Declared natively: yes
- Option source references: 0/0
- Missing option references: none
- Known gaps: Packages local targets, but broader upstream collection behavior is still limited.

## `features publish`

- Description: Package and publish Features
- Declared natively: yes
- Option source references: 0/0
- Missing option references: none
- Known gaps: Publishes a local OCI layout rather than a real authenticated registry push flow.

## `features info`

- Description: Fetch metadata for a published Feature
- Declared natively: yes
- Option source references: 2/2
- Missing option references: none
- Known gaps: Info modes are native, but published metadata still comes from embedded/manual catalog data instead of real OCI fetches.

## `features resolve-dependencies`

- Description: Read and resolve dependency graph from a configuration
- Declared natively: yes
- Option source references: 2/2
- Missing option references: none
- Known gaps: Current implementation follows declared `dependsOn` edges, but still relies on local/manual manifests rather than full OCI graph resolution.

## `features generate-docs`

- Description: Generate documentation
- Declared natively: yes
- Option source references: 6/6
- Missing option references: none
- Known gaps: Documentation generation is minimal compared with upstream.

## `templates`

- Description: Templates commands
- Declared natively: yes
- Option source references: 0/0
- Missing option references: none
- Known gaps: Top-level command exists, but published-template flows still rely on embedded/local substitutes.

## `templates apply`

- Description: Apply a template to the project
- Declared natively: yes
- Option source references: 7/7
- Missing option references: none
- Known gaps: Published template application is still based on embedded/local substitutes instead of real OCI fetches.

## `templates publish`

- Description: Package and publish templates
- Declared natively: yes
- Option source references: 0/0
- Missing option references: none
- Known gaps: Publishes a local OCI layout rather than a real authenticated registry push flow.

## `templates metadata`

- Description: Fetch a published Template\'s metadata
- Declared natively: yes
- Option source references: 1/1
- Missing option references: none
- Known gaps: Published template metadata is still based on embedded/local substitutes instead of real OCI fetches.

## `templates generate-docs`

- Description: Generate documentation
- Declared natively: yes
- Option source references: 4/4
- Missing option references: none
- Known gaps: Documentation generation is minimal compared with upstream.

## `exec`

- Description: Execute a command on a running dev container
- Declared natively: yes
- Option source references: 19/19
- Missing option references: none
- Known gaps: Core exec path is native, but upstream option coverage is still narrower.
