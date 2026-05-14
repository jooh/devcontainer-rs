//! Unit tests for feature collection commands.

use std::fs;
use std::path::{Path, PathBuf};

use super::support::unique_temp_dir;
use crate::commands::collections::features::{
    build_feature_info_payload, build_feature_info_payload_with_workspace,
    build_features_resolve_dependencies_payload,
};
use crate::commands::common::copy_directory_recursive;
use crate::test_support::write_test_control_manifest;

fn upstream_fixture_path(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../upstream/src/test/container-features/configs")
        .join(relative)
}

fn copy_upstream_fixture(relative: &str) -> PathBuf {
    let root = unique_temp_dir();
    copy_directory_recursive(&upstream_fixture_path(relative), &root)
        .expect("failed to copy upstream fixture");
    root
}

fn install_order_id_options(payload: &serde_json::Value) -> Vec<(String, serde_json::Value)> {
    payload["installOrder"]
        .as_array()
        .expect("installOrder array")
        .iter()
        .map(|entry| {
            (
                entry["id"].as_str().expect("install order id").to_string(),
                entry["options"].clone(),
            )
        })
        .collect()
}

#[test]
fn feature_dependency_resolution_respects_override_order() {
    let root = unique_temp_dir();
    let config_dir = root.join(".devcontainer");
    fs::create_dir_all(&config_dir).expect("failed to create config directory");
    fs::write(
        config_dir.join("devcontainer.json"),
        "{\n  \"image\": \"debian:bookworm\",\n  \"features\": {\n    \"feature-a\": {},\n    \"feature-b\": {}\n  },\n  \"overrideFeatureInstallOrder\": [\"feature-b\", \"feature-a\"]\n}\n",
    )
    .expect("failed to write config");

    let payload = build_features_resolve_dependencies_payload(&[
        "--workspace-folder".to_string(),
        root.display().to_string(),
    ])
    .expect("payload");

    let features = payload["resolvedFeatures"]
        .as_array()
        .expect("resolved features");
    assert_eq!(features[0], "feature-b");
    assert_eq!(features[1], "feature-a");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn feature_dependency_resolution_ignores_existing_lockfile() {
    let root = unique_temp_dir();
    let config_dir = root.join(".devcontainer");
    let feature_dir = config_dir.join("local-feature");
    fs::create_dir_all(&feature_dir).expect("failed to create feature directory");
    fs::write(
        feature_dir.join("devcontainer-feature.json"),
        "{\n  \"id\": \"local-feature\",\n  \"name\": \"Local Feature\",\n  \"version\": \"1.0.0\"\n}\n",
    )
    .expect("failed to write feature manifest");
    fs::write(feature_dir.join("install.sh"), "#!/bin/sh\nset -eu\n")
        .expect("failed to write feature install script");
    fs::write(
        config_dir.join("devcontainer.json"),
        "{\n  \"image\": \"debian:bookworm\",\n  \"features\": {\n    \"./local-feature\": {}\n  }\n}\n",
    )
    .expect("failed to write config");
    fs::write(
        config_dir.join("devcontainer-lock.json"),
        "this is not json",
    )
    .expect("failed to write corrupt lockfile");

    let payload = build_features_resolve_dependencies_payload(&[
        "--workspace-folder".to_string(),
        root.display().to_string(),
    ])
    .expect("payload should not read lockfile");

    let features = payload["resolvedFeatures"]
        .as_array()
        .expect("resolved features");
    assert_eq!(features, &[serde_json::json!("./local-feature")]);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn feature_dependency_resolution_matches_upstream_local_option_round_order() {
    let root = copy_upstream_fixture("feature-dependencies/dependsOn/local-with-options");

    let payload = build_features_resolve_dependencies_payload(&[
        "--workspace-folder".to_string(),
        root.display().to_string(),
    ])
    .expect("payload");

    let actual = install_order_id_options(&payload);
    assert_eq!(
        actual,
        vec![
            ("./b".to_string(), serde_json::json!({})),
            (
                "./b".to_string(),
                serde_json::json!({ "optA": "a", "optB": "a" })
            ),
            (
                "./b".to_string(),
                serde_json::json!({ "optA": "a", "optB": "b" })
            ),
            (
                "./b".to_string(),
                serde_json::json!({ "optA": "b", "optB": "a" })
            ),
            (
                "./b".to_string(),
                serde_json::json!({ "optA": "b", "optB": "b" })
            ),
            ("./d".to_string(), serde_json::json!({})),
            ("./e".to_string(), serde_json::json!({})),
            ("./c".to_string(), serde_json::json!({})),
            (
                "./a".to_string(),
                serde_json::json!({ "optA": "a", "optB": "b" })
            ),
        ]
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn feature_dependency_resolution_matches_upstream_override_round_priority() {
    let root =
        copy_upstream_fixture("feature-dependencies/overrideFeatureInstallOrder/local-simple");

    let payload = build_features_resolve_dependencies_payload(&[
        "--workspace-folder".to_string(),
        root.display().to_string(),
    ])
    .expect("payload");

    let ids = install_order_id_options(&payload)
        .into_iter()
        .map(|(id, _)| id)
        .collect::<Vec<_>>();
    assert_eq!(ids, vec!["./c", "./b", "./d", "./a"]);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn feature_dependency_resolution_matches_upstream_published_and_tarball_order() {
    let root = copy_upstream_fixture("feature-dependencies/dependsOn/tgz-ab");

    let payload = build_features_resolve_dependencies_payload(&[
        "--workspace-folder".to_string(),
        root.display().to_string(),
    ])
    .expect("payload");

    let actual = install_order_id_options(&payload);
    assert_eq!(
        actual,
        vec![
            (
                "ghcr.io/codspace/dependson/d@sha256:3795caa1e32ba6b30a08260039804eed6f3cf40811f0c65c118437743fa15ce8".to_string(),
                serde_json::json!({ "magicNumber": "30" })
            ),
            (
                "ghcr.io/codspace/dependson/e@sha256:9f36f159c70f8bebff57f341904b030733adb17ef12a5d58d4b3d89b2a6c7d5a".to_string(),
                serde_json::json!({ "magicNumber": "50" })
            ),
            (
                "ghcr.io/codspace/dependson/a@sha256:932027ef71da186210e6ceb3294c3459caaf6b548d2b547d5d26be3fc4b2264a".to_string(),
                serde_json::json!({ "magicNumber": "40" })
            ),
            (
                "https://github.com/codspace/tgz-features-with-dependson/releases/download/0.0.2/devcontainer-feature-A.tgz".to_string(),
                serde_json::json!({ "magicNumber": "10" })
            ),
            (
                "ghcr.io/codspace/dependson/c@sha256:db651708398b6d7af48f184c358728eaaf959606637133413cb4107b8454a868".to_string(),
                serde_json::json!({ "magicNumber": "20" })
            ),
            (
                "https://github.com/codspace/tgz-features-with-dependson/releases/download/0.0.2/devcontainer-feature-B.tgz".to_string(),
                serde_json::json!({ "magicNumber": "400" })
            ),
        ]
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn feature_dependency_resolution_rejects_upstream_circular_dependencies() {
    let root = copy_upstream_fixture("feature-dependencies/dependsOn/invalid-circular");

    let error = build_features_resolve_dependencies_payload(&[
        "--workspace-folder".to_string(),
        root.display().to_string(),
    ])
    .expect_err("circular dependencies should fail");

    assert!(error.contains("Circular feature dependency"), "{error}");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn feature_dependency_resolution_rejects_disallowed_features() {
    let root = unique_temp_dir();
    let config_dir = root.join(".devcontainer");
    let user_data = root.join("user-data");
    fs::create_dir_all(&config_dir).expect("failed to create config directory");
    write_test_control_manifest(&user_data);
    fs::write(
        config_dir.join("devcontainer.json"),
        "{\n  \"image\": \"debian:bookworm\",\n  \"features\": {\n    \"ghcr.io/devcontainers/features/problematic-feature:1\": {}\n  }\n}\n",
    )
    .expect("failed to write config");

    let error = build_features_resolve_dependencies_payload(&[
        "--workspace-folder".to_string(),
        root.display().to_string(),
        "--user-data-folder".to_string(),
        user_data.display().to_string(),
    ])
    .expect_err("disallowed feature should fail");

    assert!(error.contains("problematic-feature:1"), "{error}");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn feature_dependency_resolution_preserves_digest_pinned_oci_install_order() {
    let root = unique_temp_dir();
    let config_dir = root.join(".devcontainer");
    fs::create_dir_all(&config_dir).expect("failed to create config directory");
    fs::write(
        config_dir.join("devcontainer.json"),
        "{\n  \"image\": \"debian:bookworm\",\n  \"features\": {\n    \"ghcr.io/devcontainers/features/git-lfs@sha256:24d5802c837b2519b666a8403a9514c7296d769c9607048e9f1e040e7d7e331c\": {}\n  }\n}\n",
    )
    .expect("failed to write config");

    let payload = build_features_resolve_dependencies_payload(&[
        "--workspace-folder".to_string(),
        root.display().to_string(),
    ])
    .expect("payload");

    let actual = install_order_id_options(&payload);
    assert_eq!(
        actual,
        vec![(
            "ghcr.io/devcontainers/features/git-lfs@sha256:24d5802c837b2519b666a8403a9514c7296d769c9607048e9f1e040e7d7e331c"
                .to_string(),
            serde_json::json!({})
        )]
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn feature_info_reads_manifest_metadata() {
    let root = unique_temp_dir();
    fs::create_dir_all(&root).expect("failed to create feature root");
    fs::write(
        root.join("devcontainer-feature.json"),
        "{\n  \"id\": \"demo-feature\",\n  \"name\": \"Demo Feature\",\n  \"version\": \"1.0.0\"\n}\n",
    )
    .expect("failed to write feature manifest");

    let payload = build_feature_info_payload("manifest", root.to_string_lossy().as_ref())
        .expect("feature info");

    assert_eq!(payload["id"], "demo-feature");
    assert_eq!(payload["name"], "Demo Feature");
    assert_eq!(payload["version"], "1.0.0");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn feature_info_reads_published_catalog_oci_manifest() {
    let payload =
        build_feature_info_payload("manifest", "ghcr.io/devcontainers/features/azure-cli:1")
            .expect("feature info");

    assert_eq!(
        payload["canonicalId"],
        "ghcr.io/devcontainers/features/azure-cli@sha256:a00aa292592a8df58a940d6f6dfcf2bfd3efab145f62a17ccb12656528793134"
    );
    let manifest = &payload["manifest"];
    assert_eq!(manifest["schemaVersion"], 2);
    assert_eq!(
        manifest["mediaType"],
        "application/vnd.oci.image.manifest.v1+json"
    );
    assert_eq!(
        manifest["layers"][0]["mediaType"],
        "application/vnd.devcontainers.layer.v1+tar"
    );
    let metadata = manifest["annotations"]["dev.containers.metadata"]
        .as_str()
        .expect("metadata string");
    assert!(metadata.contains("\"id\":\"azure-cli\""), "{metadata}");
    assert!(metadata.contains("\"name\":\"Azure CLI\""), "{metadata}");
}

#[test]
fn feature_info_supports_generic_published_features() {
    let payload = build_feature_info_payload("manifest", "ghcr.io/devcontainers/features/node")
        .expect("feature info");

    assert_eq!(
        payload["manifest"]["layers"][0]["annotations"]["org.opencontainers.image.title"],
        "devcontainer-feature-node.tgz"
    );
    let metadata = payload["manifest"]["annotations"]["dev.containers.metadata"]
        .as_str()
        .expect("metadata string");
    assert!(metadata.contains("\"id\":\"node\""), "{metadata}");
    assert!(metadata.contains("\"version\":\"latest\""), "{metadata}");
}

#[test]
fn feature_info_supports_digest_pinned_catalog_refs() {
    let payload = build_feature_info_payload(
        "manifest",
        "ghcr.io/devcontainers/features/git-lfs@sha256:24d5802c837b2519b666a8403a9514c7296d769c9607048e9f1e040e7d7e331c",
    )
    .expect("feature info");

    assert_eq!(
        payload["canonicalId"],
        "ghcr.io/devcontainers/features/git-lfs@sha256:24d5802c837b2519b666a8403a9514c7296d769c9607048e9f1e040e7d7e331c"
    );
    assert_eq!(
        payload["manifest"]["layers"][0]["annotations"]["org.opencontainers.image.title"],
        "devcontainer-feature-git-lfs.tgz"
    );
    let metadata = payload["manifest"]["annotations"]["dev.containers.metadata"]
        .as_str()
        .expect("metadata string");
    assert!(metadata.contains("\"id\":\"git-lfs\""), "{metadata}");
    assert!(metadata.contains("\"name\":\"Git Lfs\""), "{metadata}");
}

#[test]
fn feature_info_reports_tags_dependencies_and_verbose_payloads() {
    let root = unique_temp_dir();
    fs::create_dir_all(&root).expect("failed to create feature root");
    fs::write(
        root.join("devcontainer-feature.json"),
        "{\n  \"id\": \"demo-feature\",\n  \"name\": \"Demo Feature\",\n  \"version\": \"1.0.0\",\n  \"dependsOn\": {\n    \"ghcr.io/devcontainers/features/common-utils:2\": {}\n  }\n}\n",
    )
    .expect("failed to write feature manifest");

    let tags =
        build_feature_info_payload("tags", root.to_string_lossy().as_ref()).expect("tags payload");
    let dependencies = build_feature_info_payload("dependencies", root.to_string_lossy().as_ref())
        .expect("dependencies payload");
    let verbose = build_feature_info_payload("verbose", root.to_string_lossy().as_ref())
        .expect("verbose payload");

    assert_eq!(tags["tags"][0], "1.0.0");
    assert!(dependencies["dependsOn"]
        .as_object()
        .expect("dependsOn object")
        .contains_key("ghcr.io/devcontainers/features/common-utils:2"));
    assert_eq!(verbose["manifest"]["id"], "demo-feature");
    assert_eq!(verbose["tags"][0], "1.0.0");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn feature_info_reads_catalog_tags_for_published_features() {
    let payload = build_feature_info_payload("tags", "ghcr.io/devcontainers/features/git:1")
        .expect("tags payload");

    let tags = payload["publishedTags"]
        .as_array()
        .expect("published tags array");
    assert_eq!(tags[0], "1.2.0");
    assert_eq!(tags[1], "1.1.5");
}

#[test]
fn feature_info_registry_tags_do_not_require_resolved_manifest() {
    let workspace = unique_temp_dir();
    let layout_dir = workspace
        .join(".devcontainer")
        .join("oci-layouts")
        .join("ghcr.io/acme/features/fake");
    fs::create_dir_all(&layout_dir).expect("layout dir");
    fs::write(
        layout_dir.join("oci-layout"),
        "{\"imageLayoutVersion\":\"1.0.0\"}\n",
    )
    .expect("oci layout");
    fs::write(
        layout_dir.join("index.json"),
        r#"{
  "schemaVersion": 2,
  "manifests": [{
    "mediaType": "application/vnd.oci.image.manifest.v1+json",
    "digest": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    "annotations": {
      "org.opencontainers.image.ref.name": "dev"
    }
  }]
}
"#,
    )
    .expect("index");

    let payload = build_feature_info_payload_with_workspace(
        "tags",
        "ghcr.io/acme/features/fake",
        Some(&workspace),
    )
    .expect("tags payload");

    assert_eq!(payload["publishedTags"], serde_json::json!(["dev"]));
    let _ = fs::remove_dir_all(workspace);
}
