# Native Template Apply Command

`devcontainer templates apply` is implemented in Rust for local and repo-known template flows. It does not perform a general live OCI registry download today.

## Local Template Root

Pass a local template root as the positional target. The root must contain `devcontainer-template.json` and a `src/` directory.

```bash
devcontainer templates apply ./path/to/template \
  --workspace-folder ./workspace
```

The native command copies the template `src/` contents into the workspace. `--omit-paths` can exclude files or directories from the copy:

```bash
devcontainer templates apply ./path/to/template \
  --workspace-folder ./workspace \
  --omit-paths '[".github/*", "docs/example.md"]'
```

## Published-Style Template ID

Use `--template-id` for a registry-style identifier:

```bash
devcontainer templates apply \
  --workspace-folder ./workspace \
  --template-id ghcr.io/devcontainers/templates/docker-from-docker:latest \
  --template-args '{ "installZsh": "false", "upgradePackages": "true" }' \
  --features '[{ "id": "ghcr.io/devcontainers/features/azure-cli:1", "options": {} }]'
```

The native implementation resolves these IDs through, in order:

- a local OCI layout under `.devcontainer/oci-layouts/<normalized-template-id>/`
- embedded upstream test template fixtures for the small set wired into this crate
- a generated fallback `devcontainer.json` for generic `ghcr.io/.../templates/...` references

`docker-from-docker` has a native special case that writes a `devcontainer.json` with the repo-owned default base image and the expected Feature entries.

## Supported Options

Use long option names for scripted workflows:

- `--workspace-folder <path>`: target workspace; defaults to the current directory.
- `--template-id <id>`: registry-style template reference.
- `--template-args <json>`: template option values for published-style template IDs.
- `--features <json>`: extra Features to merge into the applied template.
- `--omit-paths <json-array>`: copied paths to omit.
- `--tmp-dir <path>`: extraction/staging directory for temporary files.

Short aliases may appear in generated upstream help text, but the native parser for this flow currently handles the long option names above.
