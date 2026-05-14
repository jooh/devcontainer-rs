//! Smoke tests for feature-layered native runtime builds.

use std::fs;
use std::path::Path;
use std::process::Command;

use serde_json::Value;

use crate::support::runtime_harness::{write_devcontainer_config, RuntimeHarness};

#[test]
fn build_wraps_image_configs_with_feature_layers() {
    let harness = RuntimeHarness::new();
    let workspace = harness.workspace();
    let feature_dir = workspace.join(".devcontainer").join("local-feature");
    fs::create_dir_all(&feature_dir).expect("feature dir");
    fs::write(
        feature_dir.join("devcontainer-feature.json"),
        "{\n  \"id\": \"local-feature\",\n  \"name\": \"Local Feature\",\n  \"version\": \"1.0.0\",\n  \"options\": {\n    \"favorite\": {\n      \"type\": \"string\",\n      \"default\": \"blue\"\n    }\n  }\n}\n",
    )
    .expect("feature manifest");
    fs::write(feature_dir.join("install.sh"), "#!/bin/sh\nset -eu\n").expect("install script");
    write_devcontainer_config(
        &workspace,
        "{\n  \"image\": \"debian:bookworm\",\n  \"features\": {\n    \"./local-feature\": {\n      \"favorite\": \"red\"\n    }\n  }\n}\n",
    );

    let fake_podman = harness.fake_podman.to_string_lossy().to_string();
    let output = harness.run(
        &[
            "build",
            "--docker-path",
            fake_podman.as_str(),
            "--workspace-folder",
            workspace.to_string_lossy().as_ref(),
            "--image-name",
            "example/native-build:features",
        ],
        &[],
    );

    assert!(output.status.success(), "{output:?}");
    let invocations = harness.read_invocations();
    assert!(invocations.contains("--tag example/native-build:features"));
    assert_eq!(
        invocations
            .lines()
            .filter(|line| line.starts_with("build "))
            .count(),
        1
    );
}

#[test]
fn build_materializes_workspace_oci_feature_layout() {
    let harness = RuntimeHarness::new();
    let workspace = harness.workspace();
    let feature_root = harness.root.join("published-feature-src");
    let layout_root = workspace
        .join(".devcontainer")
        .join("oci-layouts")
        .join("ghcr.io")
        .join("acme")
        .join("features")
        .join("offline-feature");
    fs::create_dir_all(feature_root.join("repo")).expect("feature repo dir");
    fs::write(
        feature_root.join("devcontainer-feature.json"),
        "{\n  \"id\": \"offline-feature\",\n  \"name\": \"Offline Feature\",\n  \"version\": \"1.0.0\",\n  \"options\": {\n    \"packages\": { \"type\": \"string\", \"default\": \"\" }\n  }\n}\n",
    )
    .expect("feature manifest");
    fs::write(feature_root.join("install.sh"), "#!/bin/sh\nset -eu\n").expect("install script");
    fs::write(feature_root.join("repo").join("data.txt"), "offline data\n").expect("repo data");
    publish_feature_layout(&feature_root, &layout_root);
    write_devcontainer_config(
        &workspace,
        "{\n  \"image\": \"debian:bookworm\",\n  \"features\": {\n    \"ghcr.io/acme/features/offline-feature:1.0.0\": {\n      \"packages\": \"jq\"\n    }\n  }\n}\n",
    );

    let fake_podman = harness.fake_podman.to_string_lossy().to_string();
    let output = harness.run(
        &[
            "build",
            "--docker-path",
            fake_podman.as_str(),
            "--workspace-folder",
            workspace.to_string_lossy().as_ref(),
            "--image-name",
            "example/native-build:workspace-oci-feature",
        ],
        &[(
            "FAKE_PODMAN_REQUIRE_BUILD_CONTEXT_FILE",
            "feature-0-offline-feature/repo/data.txt",
        )],
    );

    assert!(output.status.success(), "{output:?}");
    let dockerfiles = fs::read_to_string(harness.log_dir.join("build-dockerfiles.log"))
        .expect("build dockerfiles log");
    assert!(dockerfiles.contains("COPY feature-0-offline-feature"));
    assert!(dockerfiles.contains("PACKAGES="));
    assert!(dockerfiles.contains("jq"));
}

#[test]
fn build_writes_lockfile_for_non_ghcr_workspace_oci_feature() {
    let harness = RuntimeHarness::new();
    let workspace = harness.workspace();
    let feature_root = harness.root.join("published-feature-src");
    let layout_root = workspace
        .join(".devcontainer")
        .join("oci-layouts")
        .join("example.com")
        .join("acme")
        .join("features")
        .join("offline-feature");
    fs::create_dir_all(feature_root.join("repo")).expect("feature repo dir");
    fs::write(
        feature_root.join("devcontainer-feature.json"),
        "{\n  \"id\": \"offline-feature\",\n  \"name\": \"Offline Feature\",\n  \"version\": \"1.0.0\"\n}\n",
    )
    .expect("feature manifest");
    fs::write(feature_root.join("install.sh"), "#!/bin/sh\nset -eu\n").expect("install script");
    publish_feature_layout(&feature_root, &layout_root);
    write_devcontainer_config(
        &workspace,
        "{\n  \"image\": \"debian:bookworm\",\n  \"features\": {\n    \"example.com/acme/features/offline-feature:1.0.0\": {}\n  }\n}\n",
    );

    let fake_podman = harness.fake_podman.to_string_lossy().to_string();
    let output = harness.run(
        &[
            "build",
            "--docker-path",
            fake_podman.as_str(),
            "--workspace-folder",
            workspace.to_string_lossy().as_ref(),
            "--image-name",
            "example/native-build:non-ghcr-lockfile",
        ],
        &[],
    );

    assert!(output.status.success(), "{output:?}");
    let lockfile: Value = serde_json::from_str(
        &fs::read_to_string(
            workspace
                .join(".devcontainer")
                .join("devcontainer-lock.json"),
        )
        .expect("lockfile"),
    )
    .expect("lockfile json");
    let entry = &lockfile["features"]["example.com/acme/features/offline-feature:1.0.0"];
    assert_eq!(entry["version"], "1.0.0");
    assert!(
        entry["resolved"].as_str().is_some_and(
            |value| value.starts_with("example.com/acme/features/offline-feature@sha256:")
        ),
        "{entry:?}"
    );
}

#[test]
fn build_uses_existing_lockfile_for_broad_oci_selector() {
    let harness = RuntimeHarness::new();
    let workspace = harness.workspace();
    let feature_root = harness.root.join("published-feature-src");
    let layout_root = workspace
        .join(".devcontainer")
        .join("oci-layouts")
        .join("ghcr.io")
        .join("acme")
        .join("features")
        .join("pinned-feature");
    fs::create_dir_all(&feature_root).expect("feature dir");
    fs::write(
        feature_root.join("devcontainer-feature.json"),
        "{\n  \"id\": \"pinned-feature\",\n  \"name\": \"Pinned Feature\",\n  \"version\": \"1.0.0\"\n}\n",
    )
    .expect("feature manifest");
    fs::write(feature_root.join("install.sh"), "#!/bin/sh\nset -eu\n").expect("install script");
    publish_feature_layout(&feature_root, &layout_root);
    let pinned_digest = layout_digest_for_tag(&layout_root, "1.0.0");
    fs::write(
        feature_root.join("devcontainer-feature.json"),
        "{\n  \"id\": \"pinned-feature\",\n  \"name\": \"Pinned Feature\",\n  \"version\": \"1.1.0\"\n}\n",
    )
    .expect("feature manifest");
    publish_feature_layout(&feature_root, &layout_root);
    let latest_digest = layout_digest_for_tag(&layout_root, "1.1.0");
    assert_ne!(pinned_digest, latest_digest);
    write_devcontainer_config(
        &workspace,
        "{\n  \"image\": \"debian:bookworm\",\n  \"features\": {\n    \"ghcr.io/acme/features/pinned-feature:1\": {}\n  }\n}\n",
    );
    fs::write(
        workspace.join(".devcontainer").join("devcontainer-lock.json"),
        format!(
            "{{\n  \"features\": {{\n    \"ghcr.io/acme/features/pinned-feature:1\": {{\n      \"version\": \"1.0.0\",\n      \"resolved\": \"ghcr.io/acme/features/pinned-feature@{pinned_digest}\",\n      \"integrity\": \"{pinned_digest}\"\n    }}\n  }}\n}}\n"
        ),
    )
    .expect("lockfile");

    let fake_podman = harness.fake_podman.to_string_lossy().to_string();
    let output = harness.run(
        &[
            "build",
            "--docker-path",
            fake_podman.as_str(),
            "--workspace-folder",
            workspace.to_string_lossy().as_ref(),
            "--image-name",
            "example/native-build:pinned-lockfile",
        ],
        &[],
    );

    assert!(output.status.success(), "{output:?}");
    let lockfile: Value = serde_json::from_str(
        &fs::read_to_string(
            workspace
                .join(".devcontainer")
                .join("devcontainer-lock.json"),
        )
        .expect("lockfile"),
    )
    .expect("lockfile json");
    let entry = &lockfile["features"]["ghcr.io/acme/features/pinned-feature:1"];
    assert_eq!(entry["version"], "1.0.0");
    assert_eq!(entry["integrity"], pinned_digest);
}

#[test]
fn build_writes_lockfile_for_direct_tarball_feature() {
    let harness = RuntimeHarness::new();
    let workspace = harness.workspace();
    fs::create_dir_all(workspace.join(".devcontainer")).expect("workspace config dir");
    let feature_uri = "https://github.com/codspace/tgz-features-with-dependson/releases/download/0.0.2/devcontainer-feature-B.tgz";
    write_devcontainer_config(
        &workspace,
        &format!(
            "{{\n  \"image\": \"debian:bookworm\",\n  \"features\": {{\n    \"{feature_uri}\": {{}}\n  }}\n}}\n"
        ),
    );

    let fake_podman = harness.fake_podman.to_string_lossy().to_string();
    let output = harness.run(
        &[
            "build",
            "--docker-path",
            fake_podman.as_str(),
            "--workspace-folder",
            workspace.to_string_lossy().as_ref(),
            "--image-name",
            "example/native-build:tarball-lockfile",
        ],
        &[],
    );

    assert!(output.status.success(), "{output:?}");
    let lockfile: Value = serde_json::from_str(
        &fs::read_to_string(
            workspace
                .join(".devcontainer")
                .join("devcontainer-lock.json"),
        )
        .expect("lockfile"),
    )
    .expect("lockfile json");
    let entry = &lockfile["features"][feature_uri];
    assert_eq!(entry["version"], "0.0.2");
    assert_eq!(entry["resolved"], feature_uri);
    assert!(
        entry["integrity"]
            == "sha256:d130123ba54335a026ab6cd51c8bcbd52d58a0aeaacd8a593512ba61c5117ea0",
        "{entry:?}"
    );
}

#[test]
fn feature_build_includes_syntax_directive_by_default() {
    let harness = RuntimeHarness::new();
    let workspace = harness.workspace();
    let feature_dir = workspace.join(".devcontainer").join("local-feature");
    fs::create_dir_all(&feature_dir).expect("feature dir");
    fs::write(
        feature_dir.join("devcontainer-feature.json"),
        "{\n  \"id\": \"local-feature\",\n  \"name\": \"Local Feature\",\n  \"version\": \"1.0.0\"\n}\n",
    )
    .expect("feature manifest");
    fs::write(feature_dir.join("install.sh"), "#!/bin/sh\nset -eu\n").expect("install script");
    write_devcontainer_config(
        &workspace,
        "{\n  \"image\": \"debian:bookworm\",\n  \"features\": {\n    \"./local-feature\": {}\n  }\n}\n",
    );

    let fake_podman = harness.fake_podman.to_string_lossy().to_string();
    let output = harness.run(
        &[
            "build",
            "--docker-path",
            fake_podman.as_str(),
            "--workspace-folder",
            workspace.to_string_lossy().as_ref(),
            "--image-name",
            "example/native-build:syntax",
        ],
        &[],
    );

    assert!(output.status.success(), "{output:?}");
    let dockerfiles = fs::read_to_string(harness.log_dir.join("build-dockerfiles.log"))
        .expect("build dockerfiles log");
    assert!(dockerfiles.contains("# syntax=docker/dockerfile:1.4"));
}

#[test]
fn feature_build_omits_syntax_directive_when_requested() {
    let harness = RuntimeHarness::new();
    let workspace = harness.workspace();
    let feature_dir = workspace.join(".devcontainer").join("local-feature");
    fs::create_dir_all(&feature_dir).expect("feature dir");
    fs::write(
        feature_dir.join("devcontainer-feature.json"),
        "{\n  \"id\": \"local-feature\",\n  \"name\": \"Local Feature\",\n  \"version\": \"1.0.0\"\n}\n",
    )
    .expect("feature manifest");
    fs::write(feature_dir.join("install.sh"), "#!/bin/sh\nset -eu\n").expect("install script");
    write_devcontainer_config(
        &workspace,
        "{\n  \"image\": \"debian:bookworm\",\n  \"features\": {\n    \"./local-feature\": {}\n  }\n}\n",
    );

    let fake_podman = harness.fake_podman.to_string_lossy().to_string();
    let output = harness.run(
        &[
            "build",
            "--docker-path",
            fake_podman.as_str(),
            "--workspace-folder",
            workspace.to_string_lossy().as_ref(),
            "--image-name",
            "example/native-build:no-syntax",
            "--omit-syntax-directive",
        ],
        &[],
    );

    assert!(output.status.success(), "{output:?}");
    let dockerfiles = fs::read_to_string(harness.log_dir.join("build-dockerfiles.log"))
        .expect("build dockerfiles log");
    assert!(!dockerfiles.contains("# syntax=docker/dockerfile:1.4"));
}

#[test]
fn build_pushes_final_feature_image_for_image_configs() {
    let harness = RuntimeHarness::new();
    let workspace = harness.workspace();
    let feature_dir = workspace.join(".devcontainer").join("local-feature");
    fs::create_dir_all(&feature_dir).expect("feature dir");
    fs::write(
        feature_dir.join("devcontainer-feature.json"),
        "{\n  \"id\": \"local-feature\",\n  \"name\": \"Local Feature\",\n  \"version\": \"1.0.0\"\n}\n",
    )
    .expect("feature manifest");
    fs::write(feature_dir.join("install.sh"), "#!/bin/sh\nset -eu\n").expect("install script");
    write_devcontainer_config(
        &workspace,
        "{\n  \"image\": \"debian:bookworm\",\n  \"features\": {\n    \"./local-feature\": {}\n  }\n}\n",
    );

    let fake_podman = harness.fake_podman.to_string_lossy().to_string();
    let output = harness.run(
        &[
            "build",
            "--docker-path",
            fake_podman.as_str(),
            "--workspace-folder",
            workspace.to_string_lossy().as_ref(),
            "--image-name",
            "example/native-build:features-push",
            "--push",
        ],
        &[],
    );

    assert!(output.status.success(), "{output:?}");
    let invocations = harness.read_invocations();
    assert!(invocations.contains("build --tag example/native-build:features-push"));
    assert!(invocations.contains("push example/native-build:features-push"));
}

#[test]
fn build_skips_feature_customizations_in_output_configuration_when_requested() {
    let harness = RuntimeHarness::new();
    let workspace = harness.workspace();
    let feature_dir = workspace.join(".devcontainer").join("local-feature");
    fs::create_dir_all(&feature_dir).expect("feature dir");
    fs::write(
        feature_dir.join("devcontainer-feature.json"),
        "{\n  \"id\": \"local-feature\",\n  \"name\": \"Local Feature\",\n  \"version\": \"1.0.0\",\n  \"customizations\": {\n    \"vscode\": {\n      \"extensions\": [\"ms-vscode.makefile-tools\"]\n    }\n  }\n}\n",
    )
    .expect("feature manifest");
    fs::write(feature_dir.join("install.sh"), "#!/bin/sh\nset -eu\n").expect("install script");
    write_devcontainer_config(
        &workspace,
        "{\n  \"image\": \"debian:bookworm\",\n  \"features\": {\n    \"./local-feature\": {}\n  },\n  \"customizations\": {\n    \"vscode\": {\n      \"extensions\": [\"user.extension\"]\n    }\n  }\n}\n",
    );

    let fake_podman = harness.fake_podman.to_string_lossy().to_string();
    let output = harness.run(
        &[
            "build",
            "--docker-path",
            fake_podman.as_str(),
            "--workspace-folder",
            workspace.to_string_lossy().as_ref(),
            "--skip-persisting-customizations-from-features",
        ],
        &[],
    );

    assert!(output.status.success(), "{output:?}");
    let payload = harness.parse_stdout_json(&output);
    assert_eq!(
        payload["configuration"]["customizations"],
        serde_json::json!({
            "vscode": {
                "extensions": ["user.extension"]
            }
        })
    );
}

#[test]
fn build_layers_features_on_top_of_dockerfile_builds() {
    let harness = RuntimeHarness::new();
    let workspace = harness.workspace();
    let feature_dir = workspace.join(".devcontainer").join("local-feature");
    fs::create_dir_all(&feature_dir).expect("feature dir");
    fs::write(
        feature_dir.join("devcontainer-feature.json"),
        "{\n  \"id\": \"local-feature\",\n  \"name\": \"Local Feature\",\n  \"version\": \"1.0.0\"\n}\n",
    )
    .expect("feature manifest");
    fs::write(feature_dir.join("install.sh"), "#!/bin/sh\nset -eu\n").expect("install script");
    fs::write(
        workspace.join(".devcontainer").join("Dockerfile"),
        "FROM scratch\n",
    )
    .expect("dockerfile");
    write_devcontainer_config(
        &workspace,
        "{\n  \"build\": {\n    \"dockerfile\": \"Dockerfile\",\n    \"context\": \".devcontainer\"\n  },\n  \"features\": {\n    \"./local-feature\": {}\n  }\n}\n",
    );

    let fake_podman = harness.fake_podman.to_string_lossy().to_string();
    let output = harness.run(
        &[
            "build",
            "--docker-path",
            fake_podman.as_str(),
            "--workspace-folder",
            workspace.to_string_lossy().as_ref(),
            "--image-name",
            "example/native-build:feature-stack",
        ],
        &[],
    );

    assert!(output.status.success(), "{output:?}");
    let invocations = harness.read_invocations();
    assert!(invocations.contains("--tag example/native-build:feature-stack-base"));
    assert!(invocations.contains("--tag example/native-build:feature-stack"));
    assert_eq!(
        invocations
            .lines()
            .filter(|line| line.starts_with("build "))
            .count(),
        2
    );
}

#[test]
fn build_pushes_final_feature_image_instead_of_intermediate_base_image() {
    let harness = RuntimeHarness::new();
    let workspace = harness.workspace();
    let feature_dir = workspace.join(".devcontainer").join("local-feature");
    fs::create_dir_all(&feature_dir).expect("feature dir");
    fs::write(
        feature_dir.join("devcontainer-feature.json"),
        "{\n  \"id\": \"local-feature\",\n  \"name\": \"Local Feature\",\n  \"version\": \"1.0.0\"\n}\n",
    )
    .expect("feature manifest");
    fs::write(feature_dir.join("install.sh"), "#!/bin/sh\nset -eu\n").expect("install script");
    fs::write(
        workspace.join(".devcontainer").join("Dockerfile"),
        "FROM scratch\n",
    )
    .expect("dockerfile");
    write_devcontainer_config(
        &workspace,
        "{\n  \"build\": {\n    \"dockerfile\": \"Dockerfile\",\n    \"context\": \".devcontainer\"\n  },\n  \"features\": {\n    \"./local-feature\": {}\n  }\n}\n",
    );

    let fake_podman = harness.fake_podman.to_string_lossy().to_string();
    let output = harness.run(
        &[
            "build",
            "--docker-path",
            fake_podman.as_str(),
            "--workspace-folder",
            workspace.to_string_lossy().as_ref(),
            "--image-name",
            "example/native-build:feature-stack-push",
            "--push",
        ],
        &[],
    );

    assert!(output.status.success(), "{output:?}");
    let invocations = harness.read_invocations();
    assert!(invocations.contains("push example/native-build:feature-stack-push"));
    assert!(!invocations.contains("push example/native-build:feature-stack-push-base"));
}

#[test]
fn build_writes_feature_lockfile_by_default() {
    let harness = RuntimeHarness::new();
    let workspace = harness.workspace();
    fs::create_dir_all(workspace.join(".devcontainer")).expect("workspace config dir");
    write_devcontainer_config(
        &workspace,
        "{\n  \"image\": \"debian:bookworm\",\n  \"features\": {\n    \"ghcr.io/devcontainers/features/git:1.0\": {}\n  }\n}\n",
    );

    let fake_podman = harness.fake_podman.to_string_lossy().to_string();
    let output = harness.run(
        &[
            "build",
            "--docker-path",
            fake_podman.as_str(),
            "--workspace-folder",
            workspace.to_string_lossy().as_ref(),
        ],
        &[],
    );

    assert!(output.status.success(), "{output:?}");
    let lockfile = fs::read_to_string(
        workspace
            .join(".devcontainer")
            .join("devcontainer-lock.json"),
    )
    .expect("lockfile");
    assert!(lockfile.contains("ghcr.io/devcontainers/features/git:1.0"));
    assert!(lockfile.contains("\"resolved\":"));
}

#[test]
fn build_no_lockfile_skips_feature_lockfile_write() {
    let harness = RuntimeHarness::new();
    let workspace = harness.workspace();
    fs::create_dir_all(workspace.join(".devcontainer")).expect("workspace config dir");
    write_devcontainer_config(
        &workspace,
        "{\n  \"image\": \"debian:bookworm\",\n  \"features\": {\n    \"ghcr.io/devcontainers/features/git:1.0\": {}\n  }\n}\n",
    );

    let fake_podman = harness.fake_podman.to_string_lossy().to_string();
    let output = harness.run(
        &[
            "build",
            "--docker-path",
            fake_podman.as_str(),
            "--workspace-folder",
            workspace.to_string_lossy().as_ref(),
            "--no-lockfile",
        ],
        &[],
    );

    assert!(output.status.success(), "{output:?}");
    assert!(!workspace
        .join(".devcontainer")
        .join("devcontainer-lock.json")
        .exists());
}

#[test]
fn build_rejects_mutually_exclusive_lockfile_flags() {
    let harness = RuntimeHarness::new();
    let workspace = harness.workspace();
    fs::create_dir_all(workspace.join(".devcontainer")).expect("workspace config dir");
    write_devcontainer_config(
        &workspace,
        "{\n  \"image\": \"debian:bookworm\",\n  \"features\": {\n    \"ghcr.io/devcontainers/features/git:1.0\": {}\n  }\n}\n",
    );

    let fake_podman = harness.fake_podman.to_string_lossy().to_string();
    let output = harness.run(
        &[
            "build",
            "--docker-path",
            fake_podman.as_str(),
            "--workspace-folder",
            workspace.to_string_lossy().as_ref(),
            "--no-lockfile",
            "--frozen-lockfile",
        ],
        &[],
    );

    assert!(!output.status.success(), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("utf8 stderr");
    assert!(stderr.contains("mutually exclusive"), "{stderr}");
    let invocations =
        fs::read_to_string(harness.log_dir.join("invocations.log")).unwrap_or_default();
    assert!(!invocations.contains("build "), "{invocations}");
}

#[test]
fn build_experimental_lockfile_flag_emits_deprecation_warning() {
    let harness = RuntimeHarness::new();
    let workspace = harness.workspace();
    fs::create_dir_all(workspace.join(".devcontainer")).expect("workspace config dir");
    write_devcontainer_config(
        &workspace,
        "{\n  \"image\": \"debian:bookworm\",\n  \"features\": {\n    \"ghcr.io/devcontainers/features/git:1.0\": {}\n  }\n}\n",
    );

    let fake_podman = harness.fake_podman.to_string_lossy().to_string();
    let output = harness.run(
        &[
            "build",
            "--docker-path",
            fake_podman.as_str(),
            "--workspace-folder",
            workspace.to_string_lossy().as_ref(),
            "--experimental-lockfile",
        ],
        &[],
    );

    assert!(output.status.success(), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("utf8 stderr");
    assert!(
        stderr.contains("--experimental-lockfile is deprecated"),
        "{stderr}"
    );
}

#[test]
fn build_rejects_corrupt_existing_feature_lockfile_before_build_or_push() {
    let harness = RuntimeHarness::new();
    let workspace = harness.workspace();
    let config_dir = workspace.join(".devcontainer");
    fs::create_dir_all(&config_dir).expect("workspace config dir");
    write_devcontainer_config(
        &workspace,
        "{\n  \"image\": \"debian:bookworm\",\n  \"features\": {\n    \"ghcr.io/devcontainers/features/git:1.0\": {}\n  }\n}\n",
    );
    fs::write(
        config_dir.join("devcontainer-lock.json"),
        "this is not json",
    )
    .expect("corrupt lockfile");

    let fake_podman = harness.fake_podman.to_string_lossy().to_string();
    let output = harness.run(
        &[
            "build",
            "--docker-path",
            fake_podman.as_str(),
            "--workspace-folder",
            workspace.to_string_lossy().as_ref(),
            "--image-name",
            "example/native-build:corrupt-lockfile",
            "--push",
        ],
        &[],
    );

    assert!(!output.status.success(), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("utf8 stderr");
    assert!(stderr.contains("line 1 column"), "{stderr}");
    let invocations =
        fs::read_to_string(harness.log_dir.join("invocations.log")).unwrap_or_default();
    assert!(!invocations.contains("build "), "{invocations}");
    assert!(!invocations.contains("push "), "{invocations}");
}

#[test]
fn build_omits_additional_only_features_from_generated_lockfile() {
    let harness = RuntimeHarness::new();
    let workspace = harness.workspace();
    fs::create_dir_all(workspace.join(".devcontainer")).expect("workspace config dir");
    write_devcontainer_config(
        &workspace,
        "{\n  \"image\": \"debian:bookworm\",\n  \"features\": {\n    \"ghcr.io/devcontainers/features/git:1.0\": {}\n  }\n}\n",
    );

    let fake_podman = harness.fake_podman.to_string_lossy().to_string();
    let output = harness.run(
        &[
            "build",
            "--docker-path",
            fake_podman.as_str(),
            "--workspace-folder",
            workspace.to_string_lossy().as_ref(),
            "--additional-features",
            "{\"ghcr.io/devcontainers/features/github-cli\":{}}",
        ],
        &[],
    );

    assert!(output.status.success(), "{output:?}");
    let lockfile: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(
            workspace
                .join(".devcontainer")
                .join("devcontainer-lock.json"),
        )
        .expect("lockfile"),
    )
    .expect("lockfile json");
    let features = lockfile["features"].as_object().expect("features object");
    assert!(features.contains_key("ghcr.io/devcontainers/features/git:1.0"));
    assert!(!features.contains_key("ghcr.io/devcontainers/features/github-cli"));
}

#[test]
fn build_rejects_outdated_frozen_feature_lockfile() {
    let harness = RuntimeHarness::new();
    let workspace = harness.workspace();
    let config_dir = workspace.join(".devcontainer");
    fs::create_dir_all(&config_dir).expect("workspace config dir");
    write_devcontainer_config(
        &workspace,
        "{\n  \"image\": \"debian:bookworm\",\n  \"features\": {\n    \"ghcr.io/devcontainers/features/git:1.0\": {}\n  }\n}\n",
    );
    fs::write(
        config_dir.join("devcontainer-lock.json"),
        "{\n  \"features\": {}\n}\n",
    )
    .expect("lockfile");

    let fake_podman = harness.fake_podman.to_string_lossy().to_string();
    let output = harness.run(
        &[
            "build",
            "--docker-path",
            fake_podman.as_str(),
            "--workspace-folder",
            workspace.to_string_lossy().as_ref(),
            "--frozen-lockfile",
        ],
        &[],
    );

    assert!(!output.status.success(), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("utf8 stderr");
    assert!(stderr.contains("Lockfile"));
    assert!(stderr.contains("out of date"));
}

fn publish_feature_layout(feature_root: &Path, layout_root: &Path) {
    let output = Command::new(env!("CARGO_BIN_EXE_devcontainer"))
        .args([
            "features",
            "publish",
            feature_root.to_string_lossy().as_ref(),
            "--registry",
            "ghcr.io",
            "--namespace",
            "acme/features",
            "--output-dir",
            layout_root.to_string_lossy().as_ref(),
        ])
        .output()
        .expect("features publish");

    assert!(output.status.success(), "{output:?}");
}

fn layout_digest_for_tag(layout_root: &Path, tag: &str) -> String {
    let index: Value = serde_json::from_str(
        &fs::read_to_string(layout_root.join("index.json")).expect("index json"),
    )
    .expect("index");
    index["manifests"]
        .as_array()
        .expect("manifests")
        .iter()
        .find_map(|entry| {
            (entry["annotations"]["org.opencontainers.image.ref.name"].as_str() == Some(tag))
                .then(|| entry["digest"].as_str().map(str::to_string))
                .flatten()
        })
        .expect("tag digest")
}
