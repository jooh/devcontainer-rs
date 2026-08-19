//! CLI smoke tests for collection-oriented commands.

use std::fs;
use std::path::Path;

use serde_json::Value;
use sha2::Digest;

use crate::support::runtime_harness::RuntimeHarness;
use crate::support::test_support::{devcontainer_command, unique_temp_dir};

const DEFAULT_PUBLISHED_TEMPLATE_BASE_IMAGE: &str = "docker.io/library/debian:bookworm-slim";

#[test]
fn features_test_array_filters_use_the_default_project_folder() {
    let harness = RuntimeHarness::new();
    let workspace = harness.root.join("feature-project");
    for feature in ["demo", "other"] {
        let src = workspace.join("src").join(feature);
        let test = workspace.join("test").join(feature);
        fs::create_dir_all(&src).expect("feature src");
        fs::create_dir_all(&test).expect("feature test");
        fs::write(
            src.join("devcontainer-feature.json"),
            format!(
                "{{\n  \"id\": \"{feature}\",\n  \"name\": \"{feature}\",\n  \"version\": \"1.0.0\"\n}}\n"
            ),
        )
        .expect("manifest");
        fs::write(test.join("test.sh"), "#!/bin/sh\nexit 0\n").expect("test script");
    }

    let fake_podman = harness.fake_podman.to_string_lossy().to_string();
    let output = harness.run_in_dir(
        &[
            "features",
            "test",
            "--docker-path",
            fake_podman.as_str(),
            "--features",
            "demo",
            "other",
        ],
        &[],
        Some(&workspace),
    );

    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
    assert!(stdout.contains("'demo'"), "{stdout}");
    assert!(stdout.contains("'other'"), "{stdout}");

    let _ = fs::remove_dir_all(workspace);
}

#[test]
fn collection_positionals_work_after_options() {
    let root = unique_temp_dir("devcontainer-cli-smoke");
    let feature = root.join("feature");
    let package_output = root.join("package-output");
    fs::create_dir_all(&feature).expect("feature root");
    fs::write(
        feature.join("devcontainer-feature.json"),
        "{\n  \"id\": \"demo\",\n  \"name\": \"Demo\",\n  \"version\": \"1.0.0\"\n}\n",
    )
    .expect("feature manifest");

    let package = devcontainer_command(None)
        .args([
            "features",
            "package",
            "--output-folder",
            package_output.to_string_lossy().as_ref(),
            feature.to_string_lossy().as_ref(),
        ])
        .output()
        .expect("features package should run");
    assert!(package.status.success(), "{package:?}");

    let template = root.join("template");
    let template_src = template.join("src");
    let workspace = root.join("workspace");
    fs::create_dir_all(&template_src).expect("template src");
    fs::write(
        template.join("devcontainer-template.json"),
        "{\n  \"id\": \"demo\",\n  \"name\": \"Demo\"\n}\n",
    )
    .expect("template manifest");
    fs::write(template_src.join("README.md"), "# applied\n").expect("template file");

    let apply = devcontainer_command(None)
        .args([
            "templates",
            "apply",
            "--workspace-folder",
            workspace.to_string_lossy().as_ref(),
            template.to_string_lossy().as_ref(),
        ])
        .output()
        .expect("templates apply should run");
    assert!(apply.status.success(), "{apply:?}");
    assert!(workspace.join("README.md").is_file());

    let _ = fs::remove_dir_all(root);
}

#[test]
fn features_test_emits_a_local_report() {
    let harness = RuntimeHarness::new();
    let workspace = harness.root.join("feature-project");
    let src = workspace.join("src").join("demo");
    let test = workspace.join("test").join("demo");
    fs::create_dir_all(&src).expect("feature src");
    fs::create_dir_all(&test).expect("feature test");
    fs::write(
        src.join("devcontainer-feature.json"),
        "{\n  \"id\": \"demo\",\n  \"name\": \"Demo Feature\",\n  \"version\": \"1.0.0\"\n}\n",
    )
    .expect("manifest");
    fs::write(test.join("test.sh"), "#!/bin/sh\nexit 0\n").expect("test script");
    fs::write(test.join("custom.sh"), "#!/bin/sh\nexit 0\n").expect("scenario script");
    fs::write(
        test.join("scenarios.json"),
        "{\n  \"custom\": {\n    \"image\": \"ubuntu:latest\"\n  }\n}\n",
    )
    .expect("scenarios");

    let fake_podman = harness.fake_podman.to_string_lossy().to_string();
    let output = harness.run(
        &[
            "features",
            "test",
            "--docker-path",
            fake_podman.as_str(),
            "--project-folder",
            workspace.to_string_lossy().as_ref(),
        ],
        &[],
    );

    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
    assert!(stdout.contains("TEST REPORT"));
    assert!(stdout.contains("'custom'"));
    assert!(stdout.contains("'demo'"));
    assert!(stdout.contains("Cleaning up 2 test containers"));

    let _ = fs::remove_dir_all(workspace);
}

#[test]
fn features_test_fails_when_a_test_script_fails() {
    let harness = RuntimeHarness::new();
    let workspace = harness.root.join("feature-project");
    let src = workspace.join("src").join("demo");
    let test = workspace.join("test").join("demo");
    fs::create_dir_all(&src).expect("feature src");
    fs::create_dir_all(&test).expect("feature test");
    fs::write(
        src.join("devcontainer-feature.json"),
        "{\n  \"id\": \"demo\",\n  \"name\": \"Demo Feature\",\n  \"version\": \"1.0.0\"\n}\n",
    )
    .expect("manifest");
    fs::write(test.join("test.sh"), "#!/bin/sh\nexit 1\n").expect("test script");

    let fake_podman = harness.fake_podman.to_string_lossy().to_string();
    let output = harness.run(
        &[
            "features",
            "test",
            "--docker-path",
            fake_podman.as_str(),
            "--project-folder",
            workspace.to_string_lossy().as_ref(),
        ],
        &[("FAKE_PODMAN_EXEC_EXIT_CODE", "1")],
    );

    assert!(!output.status.success(), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
    assert!(stdout.contains("TEST REPORT"));
    assert!(stdout.contains("❌ Failed:      'demo'"));

    let _ = fs::remove_dir_all(workspace);
}

#[test]
fn features_test_quiet_suppresses_local_report_output() {
    let harness = RuntimeHarness::new();
    let workspace = harness.root.join("feature-project");
    let src = workspace.join("src").join("demo");
    let test = workspace.join("test").join("demo");
    fs::create_dir_all(&src).expect("feature src");
    fs::create_dir_all(&test).expect("feature test");
    fs::write(
        src.join("devcontainer-feature.json"),
        "{\n  \"id\": \"demo\",\n  \"name\": \"Demo Feature\",\n  \"version\": \"1.0.0\"\n}\n",
    )
    .expect("manifest");
    fs::write(test.join("test.sh"), "#!/bin/sh\nexit 0\n").expect("test script");

    let fake_podman = harness.fake_podman.to_string_lossy().to_string();
    let output = harness.run(
        &[
            "features",
            "test",
            "--docker-path",
            fake_podman.as_str(),
            "--project-folder",
            workspace.to_string_lossy().as_ref(),
            "--quiet",
        ],
        &[],
    );

    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
    assert!(!stdout.contains("TEST REPORT"));
    assert!(!stdout.contains("Cleaning up"));

    let _ = fs::remove_dir_all(workspace);
}

#[test]
fn features_test_supports_build_scenarios_remote_user_and_duplicate_env() {
    let harness = RuntimeHarness::new();
    let workspace = harness.root.join("feature-project");
    let src = workspace.join("src").join("demo");
    let test = workspace.join("test").join("demo");
    let scenario_dir = test.join("custom");
    fs::create_dir_all(&src).expect("feature src");
    fs::create_dir_all(&scenario_dir).expect("scenario dir");
    fs::write(
        src.join("devcontainer-feature.json"),
        "{\n  \"id\": \"demo\",\n  \"name\": \"Demo Feature\",\n  \"version\": \"1.0.0\",\n  \"options\": {\n    \"color\": {\n      \"type\": \"string\",\n      \"enum\": [\"blue\", \"green\"],\n      \"default\": \"blue\"\n    }\n  }\n}\n",
    )
    .expect("manifest");
    fs::write(src.join("install.sh"), "#!/bin/sh\nexit 0\n").expect("install script");
    fs::write(
        test.join("duplicate.sh"),
        "#!/bin/sh\n[ \"$COLOR\" != \"blue\" ] && [ \"$COLOR__DEFAULT\" = \"blue\" ]\n",
    )
    .expect("duplicate script");
    fs::write(test.join("custom.sh"), "#!/bin/sh\nexit 0\n").expect("scenario script");
    fs::write(scenario_dir.join("Dockerfile.base"), "FROM scratch\n").expect("base dockerfile");
    fs::write(
        test.join("scenarios.json"),
        "{\n  \"custom\": {\n    \"build\": {\n      \"dockerfile\": \"Dockerfile.base\",\n      \"context\": \".\"\n    }\n  }\n}\n",
    )
    .expect("scenarios");

    let fake_podman = harness.fake_podman.to_string_lossy().to_string();
    let output = harness.run(
        &[
            "features",
            "test",
            "--docker-path",
            fake_podman.as_str(),
            "--project-folder",
            workspace.to_string_lossy().as_ref(),
            "--remote-user",
            "vscode",
            "--permit-randomization",
        ],
        &[],
    );

    assert!(output.status.success(), "{output:?}");
    let invocations = harness.read_invocations();
    assert!(invocations.contains("build "), "{invocations}");
    assert!(
        invocations.contains("exec --workdir /workspace --user vscode -e COLOR=green"),
        "{invocations}"
    );
    let dockerfiles =
        fs::read_to_string(harness.log_dir.join("build-dockerfiles.log")).expect("dockerfiles");
    assert!(dockerfiles.contains("FROM scratch"), "{dockerfiles}");

    let _ = fs::remove_dir_all(workspace);
}

#[test]
fn templates_apply_supports_published_template_ids() {
    let workspace = unique_temp_dir("devcontainer-cli-smoke");
    fs::create_dir_all(&workspace).expect("workspace");

    let output = devcontainer_command(None)
        .args([
            "templates",
            "apply",
            "--workspace-folder",
            workspace.to_string_lossy().as_ref(),
            "--template-id",
            "ghcr.io/devcontainers/templates/docker-from-docker:latest",
            "--template-args",
            "{ \"installZsh\": \"false\", \"upgradePackages\": \"true\", \"dockerVersion\": \"20.10\", \"moby\": \"true\", \"enableNonRootDocker\": \"true\" }",
            "--features",
            "[{ \"id\": \"ghcr.io/devcontainers/features/azure-cli:1\", \"options\": { \"version\": \"1\" } }]",
        ])
        .output()
        .expect("templates apply should run");

    assert!(output.status.success(), "{output:?}");
    let payload: Value = serde_json::from_slice(&output.stdout).expect("apply payload");
    assert_eq!(payload["files"][0], "./.devcontainer/devcontainer.json");
    let file = fs::read_to_string(workspace.join(".devcontainer").join("devcontainer.json"))
        .expect("devcontainer file");
    assert!(file.contains("\"name\": \"Docker from Docker\""));
    assert!(file.contains("\"installZsh\": \"false\""));
    assert!(file.contains("\"upgradePackages\": \"true\""));
    assert!(file.contains("\"version\": \"20.10\""));
    assert!(file.contains("\"moby\": \"true\""));
    assert!(file.contains("\"enableNonRootDocker\": \"true\""));
    assert!(file.contains(&format!(
        "\"image\": \"{DEFAULT_PUBLISHED_TEMPLATE_BASE_IMAGE}\""
    )));
    assert!(file.contains("ghcr.io/devcontainers/features/common-utils:1"));
    assert!(file.contains("ghcr.io/devcontainers/features/docker-from-docker:1"));
    assert!(file.contains("ghcr.io/devcontainers/features/azure-cli:1"));

    let _ = fs::remove_dir_all(workspace);
}

#[test]
fn templates_apply_supports_short_option_aliases() {
    let workspace = unique_temp_dir("devcontainer-cli-smoke");
    fs::create_dir_all(&workspace).expect("workspace");

    let output = devcontainer_command(None)
        .args([
            "templates",
            "apply",
            "-w",
            workspace.to_string_lossy().as_ref(),
            "-t",
            "ghcr.io/devcontainers/templates/docker-from-docker:latest",
            "-a",
            "{ \"installZsh\": \"false\", \"upgradePackages\": \"true\" }",
            "-f",
            "[{ \"id\": \"ghcr.io/devcontainers/features/azure-cli:1\", \"options\": {} }]",
        ])
        .output()
        .expect("templates apply should run");

    assert!(output.status.success(), "{output:?}");
    let file = fs::read_to_string(workspace.join(".devcontainer").join("devcontainer.json"))
        .expect("devcontainer file");
    assert!(file.contains("\"installZsh\": \"false\""));
    assert!(file.contains("\"upgradePackages\": \"true\""));
    assert!(file.contains("ghcr.io/devcontainers/features/azure-cli:1"));

    let _ = fs::remove_dir_all(workspace);
}

#[test]
fn templates_apply_uses_upstream_defaults_for_published_template_ids() {
    let workspace = unique_temp_dir("devcontainer-cli-smoke");
    fs::create_dir_all(&workspace).expect("workspace");

    let output = devcontainer_command(None)
        .args([
            "templates",
            "apply",
            "--workspace-folder",
            workspace.to_string_lossy().as_ref(),
            "--template-id",
            "ghcr.io/devcontainers/templates/docker-from-docker:latest",
        ])
        .output()
        .expect("templates apply should run");

    assert!(output.status.success(), "{output:?}");
    let file = fs::read_to_string(workspace.join(".devcontainer").join("devcontainer.json"))
        .expect("devcontainer file");
    assert!(file.contains("\"installZsh\": \"true\""));
    assert!(file.contains("\"upgradePackages\": \"false\""));

    let _ = fs::remove_dir_all(workspace);
}

#[test]
fn templates_metadata_supports_published_template_ids() {
    let output = devcontainer_command(None)
        .args([
            "templates",
            "metadata",
            "ghcr.io/devcontainers/templates/docker-from-docker:latest",
        ])
        .output()
        .expect("templates metadata should run");

    assert!(output.status.success(), "{output:?}");
    let payload: Value = serde_json::from_slice(&output.stdout).expect("metadata payload");
    assert_eq!(payload["id"], "docker-from-docker");
    assert_eq!(payload["name"], "Docker from Docker");
}

#[test]
fn features_info_supports_additional_published_feature_ids() {
    let output = devcontainer_command(None)
        .args([
            "features",
            "info",
            "manifest",
            "ghcr.io/devcontainers/features/node",
        ])
        .output()
        .expect("features info should run");

    assert!(output.status.success(), "{output:?}");
    let payload: Value = serde_json::from_slice(&output.stdout).expect("feature info payload");
    assert!(payload["canonicalId"]
        .as_str()
        .expect("canonical id")
        .starts_with("ghcr.io/devcontainers/features/node@sha256:"));
    assert_eq!(
        payload["manifest"]["layers"][0]["annotations"]["org.opencontainers.image.title"],
        "devcontainer-feature-node.tgz"
    );
    let metadata = payload["manifest"]["annotations"]["dev.containers.metadata"]
        .as_str()
        .expect("metadata string");
    assert!(metadata.contains("\"id\":\"node\""), "{metadata}");
    assert!(metadata.contains("\"name\":\"Node\""), "{metadata}");
}

#[test]
fn features_info_supports_text_output_for_verbose_mode() {
    let output = devcontainer_command(None)
        .args([
            "features",
            "info",
            "verbose",
            "ghcr.io/devcontainers/features/git:1",
            "--output-format",
            "text",
        ])
        .output()
        .expect("features info should run");

    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
    assert!(stdout.contains("\"feature\": \"ghcr.io/devcontainers/features/git\""));
    assert!(stdout.contains("\"publishedTags\""));
}

#[test]
fn templates_metadata_supports_additional_published_template_ids() {
    let output = devcontainer_command(None)
        .args([
            "templates",
            "metadata",
            "ghcr.io/devcontainers/templates/anaconda-postgres:latest",
        ])
        .output()
        .expect("templates metadata should run");

    assert!(output.status.success(), "{output:?}");
    let payload: Value = serde_json::from_slice(&output.stdout).expect("metadata payload");
    assert_eq!(payload["id"], "anaconda-postgres");
}

#[test]
fn templates_apply_supports_additional_published_template_ids() {
    let workspace = unique_temp_dir("devcontainer-cli-smoke");
    fs::create_dir_all(&workspace).expect("workspace");

    let output = devcontainer_command(None)
        .args([
            "templates",
            "apply",
            "--workspace-folder",
            workspace.to_string_lossy().as_ref(),
            "--template-id",
            "ghcr.io/devcontainers/templates/anaconda-postgres:latest",
            "--template-args",
            "{ \"nodeVersion\": \"lts/*\" }",
            "--features",
            "[{ \"id\": \"ghcr.io/devcontainers/features/azure-cli:1\", \"options\": {} }, { \"id\": \"ghcr.io/devcontainers/features/git:1\", \"options\": { \"version\": \"latest\", \"ppa\": true } }]",
        ])
        .output()
        .expect("templates apply should run");

    assert!(output.status.success(), "{output:?}");
    let payload: Value = serde_json::from_slice(&output.stdout).expect("apply payload");
    assert_eq!(payload["files"][0], "./.devcontainer/devcontainer.json");
    let file = fs::read_to_string(workspace.join(".devcontainer").join("devcontainer.json"))
        .expect("devcontainer file");
    assert!(file.contains("\"name\": \"Anaconda Postgres\""));
    assert!(file.contains(&format!(
        "\"image\": \"{DEFAULT_PUBLISHED_TEMPLATE_BASE_IMAGE}\""
    )));
    assert!(file.contains("ghcr.io/devcontainers/features/azure-cli:1"));
    assert!(file.contains("ghcr.io/devcontainers/features/git:1"));

    let _ = fs::remove_dir_all(workspace);
}

#[test]
fn templates_apply_reads_workspace_oci_layout_mirror() {
    let template_root = unique_temp_dir("devcontainer-cli-smoke");
    let workspace = unique_temp_dir("devcontainer-cli-smoke");
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
    publish_template_layout(&template_root, &layout_root);

    let output = devcontainer_command(None)
        .args([
            "templates",
            "apply",
            "--workspace-folder",
            workspace.to_string_lossy().as_ref(),
            "--template-id",
            "ghcr.io/acme/templates/published-template:latest",
            "--template-args",
            "{ \"channel\": \"beta\" }",
            "--features",
            "[{ \"id\": \"ghcr.io/devcontainers/features/git:1\", \"options\": {} }]",
        ])
        .output()
        .expect("templates apply should run");

    assert!(output.status.success(), "{output:?}");
    let file = fs::read_to_string(workspace.join(".devcontainer").join("devcontainer.json"))
        .expect("devcontainer file");
    assert!(file.contains("\"name\": \"beta template\""));
    assert!(file.contains("ghcr.io/devcontainers/features/git:1"));

    let _ = fs::remove_dir_all(template_root);
    let _ = fs::remove_dir_all(workspace);
}

#[test]
fn templates_metadata_reads_workspace_oci_layout_mirror() {
    let workspace = unique_temp_dir("devcontainer-cli-smoke");
    let layout_root = workspace
        .join(".devcontainer")
        .join("oci-layouts")
        .join("ghcr.io")
        .join("acme")
        .join("templates")
        .join("published-template");
    fs::create_dir_all(&workspace).expect("workspace");
    write_template_layout(
        &layout_root,
        serde_json::json!({
            "id": "published-template",
            "name": "Published Template",
            "description": "Workspace OCI template",
            "version": "1.2.3",
        }),
        None,
        "1.2.3",
    );

    let output = devcontainer_command(None)
        .args([
            "templates",
            "metadata",
            "ghcr.io/acme/templates/published-template:1.2.3",
            "--workspace-folder",
            workspace.to_string_lossy().as_ref(),
        ])
        .output()
        .expect("templates metadata should run");

    assert!(output.status.success(), "{output:?}");
    let payload: Value = serde_json::from_slice(&output.stdout).expect("metadata payload");
    assert_eq!(payload["id"], "published-template");
    assert_eq!(payload["name"], "Published Template");
    assert_eq!(payload["description"], "Workspace OCI template");

    let _ = fs::remove_dir_all(workspace);
}

fn publish_template_layout(template_root: &Path, layout_root: &Path) {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_devcontainer"))
        .args([
            "templates",
            "publish",
            template_root.to_string_lossy().as_ref(),
            "--output-dir",
            layout_root.to_string_lossy().as_ref(),
        ])
        .output()
        .expect("templates publish");

    assert!(output.status.success(), "{output:?}");
}

fn write_template_layout(
    layout_root: &Path,
    metadata: Value,
    layer_bytes: Option<&[u8]>,
    tag: &str,
) {
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

    let manifest = serde_json::json!({
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
        serde_json::to_string_pretty(&serde_json::json!({
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
}

fn sha256_digest(bytes: &[u8]) -> String {
    let mut hasher = sha2::Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}
