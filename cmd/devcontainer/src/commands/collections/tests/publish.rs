//! Unit tests for feature publishing helpers.

use std::fs;

use super::support::unique_temp_dir;
use crate::commands::collections::publish::{
    package_collection_target, publish_collection_target_to_oci,
};
use crate::commands::common::{generate_manifest_docs, ManifestDocOptions};

#[test]
fn packaging_a_collection_target_creates_an_archive() {
    let root = unique_temp_dir();
    fs::create_dir_all(&root).expect("failed to create package root");
    fs::write(
        root.join("devcontainer-feature.json"),
        "{\n  \"id\": \"packaged-feature\",\n  \"name\": \"Packaged Feature\"\n}\n",
    )
    .expect("failed to write feature manifest");

    let archive =
        package_collection_target(&root, "devcontainer-feature.json", "feature").expect("archive");

    assert!(archive.is_file());
    let _ = fs::remove_file(archive);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn packaging_reports_manifest_parse_errors() {
    let root = unique_temp_dir();
    fs::create_dir_all(&root).expect("failed to create package root");
    fs::write(root.join("devcontainer-feature.json"), "{").expect("invalid manifest");

    let error = package_collection_target(&root, "devcontainer-feature.json", "feature")
        .expect_err("invalid manifest should fail");

    assert!(!error.is_empty());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn packaging_reports_archive_creation_errors() {
    let root = unique_temp_dir();
    let target = root.join("demo");
    fs::create_dir_all(&target).expect("failed to create package root");
    fs::write(
        target.join("devcontainer-feature.json"),
        "{\n  \"id\": \"packaged-feature\",\n  \"name\": \"Packaged Feature\"\n}\n",
    )
    .expect("failed to write feature manifest");
    fs::create_dir(root.join("feature-demo.tgz")).expect("blocked archive path");

    let error = package_collection_target(&target, "devcontainer-feature.json", "feature")
        .expect_err("archive path directory should fail");

    assert!(!error.is_empty());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn generate_feature_docs_writes_readme() {
    let root = unique_temp_dir();
    fs::create_dir_all(&root).expect("failed to create docs root");
    fs::write(
        root.join("devcontainer-feature.json"),
        "{\n  \"id\": \"docs-feature\",\n  \"name\": \"Docs Feature\",\n  \"description\": \"Generated docs\"\n}\n",
    )
    .expect("failed to write feature manifest");

    let readme = generate_manifest_docs(
        &root,
        "devcontainer-feature.json",
        "Feature",
        &ManifestDocOptions::default(),
    )
    .expect("readme");

    assert!(readme.is_file());
    let content = fs::read_to_string(readme).expect("readme content");
    assert!(content.contains("Docs Feature"));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn publish_writes_a_local_oci_layout() {
    let root = unique_temp_dir();
    let output_dir = unique_temp_dir();
    fs::create_dir_all(&root).expect("feature root");
    fs::write(
        root.join("devcontainer-feature.json"),
        "{\n  \"id\": \"published-feature\",\n  \"name\": \"Published Feature\",\n  \"version\": \"1.0.0\"\n}\n",
    )
    .expect("manifest");

    let payload = publish_collection_target_to_oci(
        &root,
        "devcontainer-feature.json",
        "feature",
        "features publish",
        &["--output-dir".to_string(), output_dir.display().to_string()],
    )
    .expect("publish payload");

    assert_eq!(payload["published"], true);
    assert_eq!(
        payload["publishedTags"],
        serde_json::json!(["1", "1.0", "1.0.0", "latest"])
    );
    assert!(output_dir.join("oci-layout").is_file());
    assert!(output_dir.join("index.json").is_file());
    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_dir_all(output_dir);
}

#[test]
fn publish_updates_moving_semantic_tags_for_new_patch_versions() {
    let root = unique_temp_dir();
    let output_dir = unique_temp_dir();
    fs::create_dir_all(&root).expect("feature root");
    let manifest_path = root.join("devcontainer-feature.json");
    fs::write(
        &manifest_path,
        "{\n  \"id\": \"published-feature\",\n  \"name\": \"Published Feature\",\n  \"version\": \"1.0.0\"\n}\n",
    )
    .expect("manifest");

    publish_collection_target_to_oci(
        &root,
        "devcontainer-feature.json",
        "feature",
        "features publish",
        &["--output-dir".to_string(), output_dir.display().to_string()],
    )
    .expect("first publish payload");

    fs::write(
        &manifest_path,
        "{\n  \"id\": \"published-feature\",\n  \"name\": \"Published Feature\",\n  \"version\": \"1.0.1\"\n}\n",
    )
    .expect("updated manifest");

    let payload = publish_collection_target_to_oci(
        &root,
        "devcontainer-feature.json",
        "feature",
        "features publish",
        &["--output-dir".to_string(), output_dir.display().to_string()],
    )
    .expect("second publish payload");

    assert_eq!(
        payload["publishedTags"],
        serde_json::json!(["1", "1.0", "1.0.1", "latest"])
    );

    let index: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(output_dir.join("index.json")).expect("index"))
            .expect("index json");
    let tags = index["manifests"]
        .as_array()
        .expect("manifests")
        .iter()
        .filter_map(|entry| entry["annotations"]["org.opencontainers.image.ref.name"].as_str())
        .collect::<Vec<_>>();

    assert!(tags.contains(&"1"));
    assert!(tags.contains(&"1.0"));
    assert!(tags.contains(&"1.0.0"));
    assert!(tags.contains(&"1.0.1"));
    assert!(tags.contains(&"latest"));
    assert_eq!(tags.len(), 5);

    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_dir_all(output_dir);
}

#[test]
fn publish_does_not_move_semantic_tags_back_to_older_versions() {
    let root = unique_temp_dir();
    let output_dir = unique_temp_dir();
    fs::create_dir_all(&root).expect("feature root");
    let manifest_path = root.join("devcontainer-feature.json");
    fs::write(
        &manifest_path,
        "{\n  \"id\": \"published-feature\",\n  \"name\": \"Published Feature\",\n  \"version\": \"1.0.1\"\n}\n",
    )
    .expect("manifest");

    publish_collection_target_to_oci(
        &root,
        "devcontainer-feature.json",
        "feature",
        "features publish",
        &["--output-dir".to_string(), output_dir.display().to_string()],
    )
    .expect("first publish payload");

    fs::write(
        &manifest_path,
        "{\n  \"id\": \"published-feature\",\n  \"name\": \"Published Feature\",\n  \"version\": \"1.0.0\"\n}\n",
    )
    .expect("updated manifest");

    let payload = publish_collection_target_to_oci(
        &root,
        "devcontainer-feature.json",
        "feature",
        "features publish",
        &["--output-dir".to_string(), output_dir.display().to_string()],
    )
    .expect("second publish payload");

    assert_eq!(payload["publishedTags"], serde_json::json!(["1.0.0"]));
    let index: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(output_dir.join("index.json")).expect("index"))
            .expect("index json");
    let moving_tags = index["manifests"]
        .as_array()
        .expect("manifests")
        .iter()
        .filter(|entry| {
            matches!(
                entry["annotations"]["org.opencontainers.image.ref.name"].as_str(),
                Some("1" | "1.0" | "latest")
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(moving_tags.len(), 3);

    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_dir_all(output_dir);
}

#[test]
fn publish_rewrites_existing_layout_for_same_version_republishes() {
    let root = unique_temp_dir();
    let output_dir = unique_temp_dir();
    fs::create_dir_all(&root).expect("feature root");
    let manifest_path = root.join("devcontainer-feature.json");
    fs::write(
        &manifest_path,
        "{\n  \"id\": \"published-feature\",\n  \"name\": \"Published Feature\",\n  \"version\": \"1.0.0\",\n  \"description\": \"first\"\n}\n",
    )
    .expect("manifest");

    let first_payload = publish_collection_target_to_oci(
        &root,
        "devcontainer-feature.json",
        "feature",
        "features publish",
        &["--output-dir".to_string(), output_dir.display().to_string()],
    )
    .expect("first publish payload");

    fs::write(
        &manifest_path,
        "{\n  \"id\": \"published-feature\",\n  \"name\": \"Published Feature\",\n  \"version\": \"1.0.0\",\n  \"description\": \"second\"\n}\n",
    )
    .expect("updated manifest");

    let second_payload = publish_collection_target_to_oci(
        &root,
        "devcontainer-feature.json",
        "feature",
        "features publish",
        &["--output-dir".to_string(), output_dir.display().to_string()],
    )
    .expect("second publish payload");

    assert_eq!(second_payload["published"], true);
    assert_ne!(first_payload["digest"], second_payload["digest"]);

    let index: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(output_dir.join("index.json")).expect("index"))
            .expect("index json");
    let published_manifest = index["manifests"]
        .as_array()
        .expect("manifests")
        .iter()
        .find(|entry| entry["annotations"]["org.opencontainers.image.ref.name"] == "1.0.0")
        .expect("1.0.0 tag");

    assert_eq!(published_manifest["digest"], second_payload["digest"]);

    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_dir_all(output_dir);
}

#[test]
fn publish_versionless_manifests_as_latest() {
    let root = unique_temp_dir();
    let output_dir = unique_temp_dir();
    fs::create_dir_all(&root).expect("feature root");
    fs::write(
        root.join("devcontainer-feature.json"),
        "{\n  \"id\": \"published-feature\",\n  \"name\": \"Published Feature\"\n}\n",
    )
    .expect("manifest");

    let payload = publish_collection_target_to_oci(
        &root,
        "devcontainer-feature.json",
        "feature",
        "features publish",
        &["--output-dir".to_string(), output_dir.display().to_string()],
    )
    .expect("publish payload");

    assert_eq!(payload["published"], true);
    assert_eq!(payload["version"], "latest");
    assert_eq!(payload["publishedTags"], serde_json::json!(["latest"]));
    assert!(output_dir.join("oci-layout").is_file());
    assert!(output_dir.join("index.json").is_file());

    let index: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(output_dir.join("index.json")).expect("index"))
            .expect("index json");
    let tags = index["manifests"]
        .as_array()
        .expect("manifests")
        .iter()
        .filter_map(|entry| entry["annotations"]["org.opencontainers.image.ref.name"].as_str())
        .collect::<Vec<_>>();

    assert_eq!(tags, vec!["latest"]);

    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_dir_all(output_dir);
}

#[test]
fn generate_feature_docs_include_collection_and_repository_metadata() {
    let root = unique_temp_dir();
    fs::create_dir_all(&root).expect("failed to create docs root");
    fs::write(
        root.join("devcontainer-feature.json"),
        "{\n  \"id\": \"docs-feature\",\n  \"name\": \"Docs Feature\",\n  \"description\": \"Generated docs\"\n}\n",
    )
    .expect("failed to write feature manifest");

    let readme = generate_manifest_docs(
        &root,
        "devcontainer-feature.json",
        "Feature",
        &ManifestDocOptions {
            registry: Some("ghcr.io".to_string()),
            namespace: Some("devcontainers/features".to_string()),
            github_owner: Some("devcontainers".to_string()),
            github_repo: Some("cli".to_string()),
        },
    )
    .expect("readme");

    let content = fs::read_to_string(readme).expect("readme content");
    assert!(content.contains("`ghcr.io/devcontainers/features/docs-feature`"));
    assert!(content.contains("https://github.com/devcontainers/cli"));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn publish_records_registry_namespace_and_resource_metadata() {
    let root = unique_temp_dir();
    let output_dir = unique_temp_dir();
    fs::create_dir_all(&root).expect("feature root");
    fs::write(
        root.join("devcontainer-feature.json"),
        "{\n  \"id\": \"published-feature\",\n  \"name\": \"Published Feature\",\n  \"version\": \"1.0.0\"\n}\n",
    )
    .expect("manifest");

    let payload = publish_collection_target_to_oci(
        &root,
        "devcontainer-feature.json",
        "feature",
        "features publish",
        &[
            "--output-dir".to_string(),
            output_dir.display().to_string(),
            "--registry".to_string(),
            "example.registry".to_string(),
            "--namespace".to_string(),
            "acme/features".to_string(),
        ],
    )
    .expect("publish payload");

    assert_eq!(payload["registry"], "example.registry");
    assert_eq!(payload["namespace"], "acme/features");
    assert_eq!(
        payload["resource"],
        "example.registry/acme/features/published-feature"
    );
    let manifest_digest = payload["digest"]
        .as_str()
        .expect("digest")
        .trim_start_matches("sha256:");
    let manifest = fs::read_to_string(
        output_dir
            .join("blobs")
            .join("sha256")
            .join(manifest_digest),
    )
    .expect("manifest blob");
    assert!(manifest.contains("example.registry/acme/features/published-feature"));
    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_dir_all(output_dir);
}

#[test]
fn publish_with_namespace_and_missing_id_leaves_resource_unset() {
    let root = unique_temp_dir();
    let output_dir = unique_temp_dir();
    fs::create_dir_all(&root).expect("feature root");
    fs::write(
        root.join("devcontainer-feature.json"),
        "{\n  \"name\": \"Published Feature\",\n  \"version\": \"1.0.0\"\n}\n",
    )
    .expect("manifest");

    let payload = publish_collection_target_to_oci(
        &root,
        "devcontainer-feature.json",
        "feature",
        "features publish",
        &[
            "--output-dir".to_string(),
            output_dir.display().to_string(),
            "--namespace".to_string(),
            "acme/features".to_string(),
        ],
    )
    .expect("publish payload");

    assert_eq!(payload["namespace"], "acme/features");
    assert!(payload["resource"].is_null());
    let manifest_digest = payload["digest"]
        .as_str()
        .expect("digest")
        .trim_start_matches("sha256:");
    let manifest: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(
            output_dir
                .join("blobs")
                .join("sha256")
                .join(manifest_digest),
        )
        .expect("manifest blob"),
    )
    .expect("manifest json");
    assert!(manifest["annotations"]
        .get("org.opencontainers.image.ref.name")
        .is_none());
    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_dir_all(output_dir);
}

#[test]
fn publish_defaults_layout_next_to_collection_target_parent() {
    let root = unique_temp_dir();
    let target = root.join("features").join("demo");
    fs::create_dir_all(&target).expect("feature root");
    fs::write(
        target.join("devcontainer-feature.json"),
        "{\n  \"id\": \"published-feature\",\n  \"name\": \"Published Feature\",\n  \"version\": \"1.0.0\"\n}\n",
    )
    .expect("manifest");

    let payload = publish_collection_target_to_oci(
        &target,
        "devcontainer-feature.json",
        "feature",
        "features publish",
        &[],
    )
    .expect("publish payload");

    let expected_layout = root.join("features").join("feature-oci-layout");
    assert_eq!(
        payload["layout"].as_str(),
        Some(expected_layout.to_string_lossy().as_ref())
    );
    assert!(expected_layout.join("index.json").is_file());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn publish_reports_layout_write_failures() {
    let root = unique_temp_dir();
    let output_path = root.join("blocked-layout");
    fs::create_dir_all(root.join("feature")).expect("feature root");
    fs::write(
        root.join("feature").join("devcontainer-feature.json"),
        "{\n  \"id\": \"published-feature\",\n  \"name\": \"Published Feature\",\n  \"version\": \"1.0.0\"\n}\n",
    )
    .expect("manifest");
    fs::write(&output_path, "not a directory").expect("blocked layout file");

    let error = publish_collection_target_to_oci(
        &root.join("feature"),
        "devcontainer-feature.json",
        "feature",
        "features publish",
        &[
            "--output-dir".to_string(),
            output_path.display().to_string(),
        ],
    )
    .expect_err("blocked layout should fail");

    assert!(
        error.to_ascii_lowercase().contains("not a directory"),
        "{error}"
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn publish_reports_manifest_parse_errors() {
    let root = unique_temp_dir();
    let output_dir = unique_temp_dir();
    fs::create_dir_all(&root).expect("feature root");
    fs::write(root.join("devcontainer-feature.json"), "{").expect("invalid manifest");

    let error = publish_collection_target_to_oci(
        &root,
        "devcontainer-feature.json",
        "feature",
        "features publish",
        &["--output-dir".to_string(), output_dir.display().to_string()],
    )
    .expect_err("invalid manifest should fail");

    assert!(!error.is_empty());
    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_dir_all(output_dir);
}

#[test]
fn publish_reports_invalid_existing_index_json() {
    let root = unique_temp_dir();
    let output_dir = unique_temp_dir();
    fs::create_dir_all(&root).expect("feature root");
    fs::write(
        root.join("devcontainer-feature.json"),
        "{\n  \"id\": \"published-feature\",\n  \"name\": \"Published Feature\",\n  \"version\": \"1.0.0\"\n}\n",
    )
    .expect("manifest");
    fs::create_dir_all(&output_dir).expect("output dir");
    fs::write(output_dir.join("index.json"), "{").expect("invalid index");

    let error = publish_collection_target_to_oci(
        &root,
        "devcontainer-feature.json",
        "feature",
        "features publish",
        &["--output-dir".to_string(), output_dir.display().to_string()],
    )
    .expect_err("invalid index should fail");

    assert!(!error.is_empty());
    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_dir_all(output_dir);
}

#[test]
fn publish_ignores_untagged_existing_index_entries() {
    let root = unique_temp_dir();
    let output_dir = unique_temp_dir();
    fs::create_dir_all(&root).expect("feature root");
    fs::write(
        root.join("devcontainer-feature.json"),
        "{\n  \"id\": \"published-feature\",\n  \"name\": \"Published Feature\",\n  \"version\": \"1.0.0\"\n}\n",
    )
    .expect("manifest");
    fs::create_dir_all(&output_dir).expect("output dir");
    fs::write(
        output_dir.join("index.json"),
        r#"{
  "schemaVersion": 2,
  "manifests": [{
    "digest": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    "annotations": {}
  }]
}
"#,
    )
    .expect("index");

    publish_collection_target_to_oci(
        &root,
        "devcontainer-feature.json",
        "feature",
        "features publish",
        &["--output-dir".to_string(), output_dir.display().to_string()],
    )
    .expect("publish payload");

    let index: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(output_dir.join("index.json")).expect("index"))
            .expect("index json");
    assert!(index["manifests"]
        .as_array()
        .expect("manifests")
        .iter()
        .any(
            |entry| entry["annotations"].as_object().is_some_and(|annotations| {
                !annotations.contains_key("org.opencontainers.image.ref.name")
            })
        ));
    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_dir_all(output_dir);
}

#[test]
fn publish_non_semver_versions_as_exact_tags() {
    let root = unique_temp_dir();
    let output_dir = unique_temp_dir();
    fs::create_dir_all(&root).expect("feature root");
    fs::write(
        root.join("devcontainer-feature.json"),
        "{\n  \"id\": \"published-feature\",\n  \"name\": \"Published Feature\",\n  \"version\": \"1.2.3.4\"\n}\n",
    )
    .expect("manifest");

    let payload = publish_collection_target_to_oci(
        &root,
        "devcontainer-feature.json",
        "feature",
        "features publish",
        &["--output-dir".to_string(), output_dir.display().to_string()],
    )
    .expect("publish payload");

    assert_eq!(payload["publishedTags"], serde_json::json!(["1.2.3.4"]));
    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_dir_all(output_dir);
}
