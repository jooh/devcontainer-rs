//! Unit tests for template collection helpers.

use std::fs;
use std::path::Path;

use serde_json::json;
use sha2::{Digest, Sha256};

use super::support::unique_temp_dir;
use crate::commands::collections::publish::publish_collection_target_to_oci;
use crate::commands::collections::templates::{
    apply_catalog_template, apply_template_target, build_template_metadata_payload,
    run_template_apply,
};

#[test]
fn published_embedded_templates_copy_upstream_source_files() {
    let workspace = unique_temp_dir();
    fs::create_dir_all(&workspace).expect("workspace");

    let payload = apply_catalog_template(
        "ghcr.io/devcontainers/templates/node-mongo:latest",
        &workspace,
        &[],
    )
    .expect("template apply");

    assert_eq!(payload["id"], "node-mongo");
    assert!(workspace
        .join(".devcontainer")
        .join("docker-compose.yml")
        .is_file());
    assert!(workspace
        .join(".devcontainer")
        .join("devcontainer.json")
        .is_file());
    let _ = fs::remove_dir_all(workspace);
}

#[test]
fn published_embedded_templates_apply_template_args_and_extra_features() {
    let workspace = unique_temp_dir();
    fs::create_dir_all(&workspace).expect("workspace");

    apply_catalog_template(
        "ghcr.io/devcontainers/templates/alpine:latest",
        &workspace,
        &[
            "--template-args".to_string(),
            json!({ "imageVariant": "3.14" }).to_string(),
            "--features".to_string(),
            json!([{ "id": "ghcr.io/devcontainers/features/git:1", "options": {} }]).to_string(),
        ],
    )
    .expect("template apply");

    let config = fs::read_to_string(workspace.join(".devcontainer.json")).expect("config");
    assert!(config.contains("0-alpine-3.14"));
    assert!(config.contains("ghcr.io/devcontainers/features/git:1"));
    let _ = fs::remove_dir_all(workspace);
}

#[test]
fn template_apply_copies_template_src_into_workspace() {
    let template_root = unique_temp_dir();
    let template_src = template_root.join("src");
    let workspace_root = unique_temp_dir();
    fs::create_dir_all(&template_src).expect("failed to create template src");
    fs::write(
        template_root.join("devcontainer-template.json"),
        "{\n  \"id\": \"demo-template\",\n  \"name\": \"Demo Template\"\n}\n",
    )
    .expect("failed to write template manifest");
    fs::write(template_src.join("README.md"), "# template\n")
        .expect("failed to write template file");

    apply_template_target(&template_root, &workspace_root).expect("apply template");

    assert!(workspace_root.join("README.md").is_file());
    let _ = fs::remove_dir_all(template_root);
    let _ = fs::remove_dir_all(workspace_root);
}

#[test]
fn template_apply_supports_omit_paths_and_tmp_dir() {
    let template_root = unique_temp_dir();
    let template_src = template_root.join("src");
    let workspace_root = unique_temp_dir();
    let tmp_dir = unique_temp_dir();
    fs::create_dir_all(template_src.join(".github")).expect("failed to create template src");
    fs::write(
        template_root.join("devcontainer-template.json"),
        "{\n  \"id\": \"demo-template\",\n  \"name\": \"Demo Template\"\n}\n",
    )
    .expect("failed to write template manifest");
    fs::write(template_src.join("README.md"), "# template\n")
        .expect("failed to write template file");
    fs::write(
        template_src.join(".github").join("workflows.yml"),
        "name: ci\n",
    )
    .expect("failed to write omitted file");

    run_template_apply(&[
        template_root.display().to_string(),
        "--workspace-folder".to_string(),
        workspace_root.display().to_string(),
        "--omit-paths".to_string(),
        "[\".github/*\"]".to_string(),
        "--tmp-dir".to_string(),
        tmp_dir.display().to_string(),
    ])
    .expect("apply template");

    assert!(workspace_root.join("README.md").is_file());
    assert!(!workspace_root.join(".github").exists());
    assert!(tmp_dir.is_dir());
    let _ = fs::remove_dir_all(template_root);
    let _ = fs::remove_dir_all(workspace_root);
    let _ = fs::remove_dir_all(tmp_dir);
}

#[test]
fn template_metadata_reads_manifest_metadata() {
    let root = unique_temp_dir();
    fs::create_dir_all(&root).expect("failed to create template root");
    fs::write(
        root.join("devcontainer-template.json"),
        "{\n  \"id\": \"demo-template\",\n  \"name\": \"Demo Template\"\n}\n",
    )
    .expect("failed to write template manifest");

    let payload = build_template_metadata_payload(root.to_string_lossy().as_ref(), None)
        .expect("template metadata");

    assert_eq!(payload["id"], "demo-template");
    assert_eq!(payload["name"], "Demo Template");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn template_metadata_reads_published_catalog_metadata() {
    let payload = build_template_metadata_payload(
        "ghcr.io/devcontainers/templates/docker-from-docker:latest",
        None,
    )
    .expect("template metadata");

    assert_eq!(payload["id"], "docker-from-docker");
    assert_eq!(payload["name"], "Docker from Docker");
}

#[test]
fn template_metadata_supports_generic_published_templates() {
    let payload = build_template_metadata_payload(
        "ghcr.io/devcontainers/templates/anaconda-postgres:latest",
        None,
    )
    .expect("template metadata");

    assert_eq!(payload["id"], "anaconda-postgres");
    assert_eq!(payload["name"], "Anaconda Postgres");
}

#[test]
fn template_metadata_supports_digest_pinned_catalog_refs() {
    let payload = build_template_metadata_payload(
        "ghcr.io/devcontainers/templates/docker-from-docker@sha256:0123456789abcdef",
        None,
    )
    .expect("template metadata");

    assert_eq!(payload["id"], "docker-from-docker");
    assert_eq!(payload["name"], "Docker from Docker");
}

#[test]
fn template_metadata_reads_workspace_oci_layout_metadata() {
    let workspace = unique_temp_dir();
    fs::create_dir_all(&workspace).expect("workspace");
    let layout_root = workspace
        .join(".devcontainer")
        .join("oci-layouts")
        .join("ghcr.io")
        .join("acme")
        .join("templates")
        .join("published-template");
    write_template_layout(
        &layout_root,
        json!({
            "id": "published-template",
            "name": "Published Template",
            "description": "Template from workspace OCI mirror",
            "version": "1.2.3",
        }),
        None,
        "1.2.3",
    );

    let payload = build_template_metadata_payload(
        "ghcr.io/acme/templates/published-template:1.2.3",
        Some(workspace.as_path()),
    )
    .expect("template metadata");

    assert_eq!(payload["id"], "published-template");
    assert_eq!(payload["name"], "Published Template");
    assert_eq!(payload["description"], "Template from workspace OCI mirror");
    let _ = fs::remove_dir_all(workspace);
}

#[test]
fn published_templates_apply_workspace_oci_layout_archives() {
    let template_root = unique_temp_dir();
    let workspace = unique_temp_dir();
    let layout_root = workspace
        .join(".devcontainer")
        .join("oci-layouts")
        .join("ghcr.io")
        .join("acme")
        .join("templates")
        .join("published-template");
    fs::create_dir_all(template_root.join(".devcontainer")).expect("template files");
    fs::create_dir_all(&workspace).expect("workspace");
    fs::write(
        template_root.join("devcontainer-template.json"),
        "{\n  \"id\": \"published-template\",\n  \"name\": \"Published Template\",\n  \"description\": \"Workspace OCI template\",\n  \"version\": \"1.2.3\",\n  \"options\": {\n    \"channel\": { \"type\": \"string\", \"default\": \"stable\" }\n  }\n}\n",
    )
    .expect("manifest");
    fs::write(
        template_root
            .join(".devcontainer")
            .join("devcontainer.json"),
        "{\n  \"name\": \"${templateOption:channel} template\"\n}\n",
    )
    .expect("template config");

    publish_collection_target_to_oci(
        &template_root,
        "devcontainer-template.json",
        "template",
        "templates publish",
        &[
            "--output-dir".to_string(),
            layout_root.display().to_string(),
        ],
    )
    .expect("publish payload");

    apply_catalog_template(
        "ghcr.io/acme/templates/published-template:latest",
        &workspace,
        &[
            "--template-args".to_string(),
            json!({ "channel": "beta" }).to_string(),
            "--features".to_string(),
            json!([{ "id": "ghcr.io/devcontainers/features/git:1", "options": {} }]).to_string(),
        ],
    )
    .expect("template apply");

    let config = fs::read_to_string(workspace.join(".devcontainer").join("devcontainer.json"))
        .expect("config");
    assert!(config.contains("\"name\": \"beta template\""), "{config}");
    assert!(
        config.contains("ghcr.io/devcontainers/features/git:1"),
        "{config}"
    );
    let _ = fs::remove_dir_all(template_root);
    let _ = fs::remove_dir_all(workspace);
}

#[test]
fn published_template_local_oci_layout_errors_without_layer() {
    let workspace = unique_temp_dir();
    let layout_root = workspace
        .join(".devcontainer")
        .join("oci-layouts")
        .join("ghcr.io")
        .join("acme")
        .join("templates")
        .join("no-layer-template");
    fs::create_dir_all(layout_root.join("blobs").join("sha256")).expect("layout blobs");
    fs::write(layout_root.join("oci-layout"), "{}").expect("layout marker");
    let manifest = json!({
        "schemaVersion": 2,
        "layers": [],
        "annotations": {
            "dev.containers.metadata": json!({
                "id": "no-layer-template",
                "name": "No Layer Template",
                "version": "1.0.0"
            }).to_string()
        }
    });
    let manifest_bytes = serde_json::to_vec(&manifest).expect("manifest bytes");
    let manifest_digest = sha256_digest(&manifest_bytes);
    fs::write(
        layout_root
            .join("blobs")
            .join("sha256")
            .join(&manifest_digest),
        manifest_bytes,
    )
    .expect("manifest blob");
    fs::write(
        layout_root.join("index.json"),
        json!({
            "schemaVersion": 2,
            "manifests": [{
                "digest": format!("sha256:{manifest_digest}"),
                "annotations": {
                    "org.opencontainers.image.ref.name": "1.0.0"
                }
            }]
        })
        .to_string(),
    )
    .expect("index");

    let error = apply_catalog_template(
        "ghcr.io/acme/templates/no-layer-template:1.0.0",
        &workspace,
        &[],
    )
    .expect_err("missing layer");

    assert!(error.contains("Published template OCI manifest is missing a layer"));
    let _ = fs::remove_dir_all(workspace);
}

#[test]
fn generic_published_template_ignores_extra_features_without_id() {
    let workspace = unique_temp_dir();

    apply_catalog_template(
        "ghcr.io/devcontainers/templates/anaconda-postgres:latest",
        &workspace,
        &[
            "--features".to_string(),
            json!([{ "options": { "version": "latest" } }]).to_string(),
        ],
    )
    .expect("template apply");

    let config = fs::read_to_string(workspace.join(".devcontainer").join("devcontainer.json"))
        .expect("config");
    assert!(!config.contains("\"features\""), "{config}");
    let _ = fs::remove_dir_all(workspace);
}

fn write_template_layout(
    layout_root: &Path,
    metadata: serde_json::Value,
    layer_bytes: Option<&[u8]>,
    tag: &str,
) -> String {
    fs::create_dir_all(layout_root.join("blobs").join("sha256")).expect("layout blobs");
    fs::write(
        layout_root.join("oci-layout"),
        "{\n  \"imageLayoutVersion\": \"1.0.0\"\n}\n",
    )
    .expect("layout marker");

    let layer_bytes = layer_bytes.unwrap_or(b"");
    let layer_digest = sha256_digest(layer_bytes);
    fs::write(
        layout_root.join("blobs").join("sha256").join(&layer_digest),
        layer_bytes,
    )
    .expect("layer blob");

    let manifest = json!({
        "schemaVersion": 2,
        "mediaType": "application/vnd.oci.image.manifest.v1+json",
        "layers": [{
            "mediaType": "application/vnd.devcontainers.layer.v1+tar+gzip",
            "digest": format!("sha256:{layer_digest}"),
            "size": layer_bytes.len(),
        }],
        "annotations": {
            "dev.containers.metadata": metadata.to_string(),
        }
    });
    let manifest_bytes = serde_json::to_vec_pretty(&manifest).expect("manifest bytes");
    let manifest_digest = sha256_digest(&manifest_bytes);
    fs::write(
        layout_root
            .join("blobs")
            .join("sha256")
            .join(&manifest_digest),
        &manifest_bytes,
    )
    .expect("manifest blob");
    fs::write(
        layout_root.join("index.json"),
        serde_json::to_string_pretty(&json!({
            "schemaVersion": 2,
            "manifests": [{
                "mediaType": "application/vnd.oci.image.manifest.v1+json",
                "digest": format!("sha256:{manifest_digest}"),
                "size": manifest_bytes.len(),
                "annotations": {
                    "org.opencontainers.image.ref.name": tag,
                }
            }]
        }))
        .expect("index payload"),
    )
    .expect("index write");

    manifest_digest
}

fn sha256_digest(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}
