# Native Template Publish Command

`devcontainer templates publish` currently packages a single local Template into a local OCI layout. It does not push to a remote registry or use registry credentials.

The target must be a Template root containing `devcontainer-template.json`:

```bash
devcontainer templates publish ./src/my-template \
  --registry ghcr.io \
  --namespace example/templates \
  --output-dir ./template-oci-layout
```

The command writes:

- a compressed Template archive next to the target
- `oci-layout`
- `index.json`
- `blobs/sha256/*`

The JSON result includes `mode: "local-oci-layout"`, the layout path, the archive path, the manifest digest, and the tags written into the layout.

## Tags

For semantic versions, the native publisher writes moving major/minor/latest tags when the new version is not older than the existing tags in the same layout. For example, publishing `1.2.3` to an empty layout writes:

- `1`
- `1.2`
- `1.2.3`
- `latest`

Non-semver versions are written as a single exact tag.

## Applying a Local Layout

`templates apply` can consume the layout when it is placed under the workspace path expected by the native registry helper:

```text
<workspace>/.devcontainer/oci-layouts/ghcr.io/example/templates/my-template/
```

Then apply it with the same registry-style reference:

```bash
devcontainer templates apply \
  --workspace-folder <workspace> \
  --template-id ghcr.io/example/templates/my-template:latest
```

## Supported Options

Use long option names for scripted workflows:

- `--registry <host>`: registry host recorded in metadata; defaults to `ghcr.io`.
- `--namespace <name>`: namespace recorded in metadata.
- `--output-dir <path>`: local OCI layout destination.
- `--log-level <level>`: accepted for compatibility.

Short aliases may appear in generated upstream help text, but the native parser for this flow currently handles the long option names above.
