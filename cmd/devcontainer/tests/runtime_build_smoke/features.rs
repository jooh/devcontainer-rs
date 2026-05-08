//! Smoke tests for feature-layered native runtime builds.

use std::fs;
use std::path::Path;
use std::process::Command;

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
fn build_writes_feature_lockfile_when_requested() {
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
            "--experimental-frozen-lockfile",
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
