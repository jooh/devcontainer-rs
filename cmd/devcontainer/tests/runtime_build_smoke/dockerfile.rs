//! Smoke tests for Dockerfile-based native runtime builds.

use std::fs;

use crate::support::runtime_harness::{write_devcontainer_config, RuntimeHarness};

#[test]
fn build_invokes_podman_for_dockerfile_configs() {
    let harness = RuntimeHarness::new();
    let workspace = harness.workspace();
    fs::create_dir_all(workspace.join(".devcontainer")).expect("workspace config dir");
    fs::write(
        workspace.join(".devcontainer").join("Dockerfile"),
        "FROM scratch\n",
    )
    .expect("dockerfile");
    write_devcontainer_config(
        &workspace,
        "{\n  \"build\": {\n    \"dockerfile\": \"Dockerfile\",\n    \"context\": \".devcontainer\"\n  }\n}\n",
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
            "example/native-build:test",
        ],
        &[],
    );

    assert!(output.status.success(), "{output:?}");
    let payload = harness.parse_stdout_json(&output);
    assert_eq!(payload["outcome"], "success");
    assert_eq!(payload["imageName"], "example/native-build:test");

    let invocations = harness.read_invocations();
    assert!(invocations.contains("build "));
    assert!(invocations.contains("--tag example/native-build:test"));
    assert!(invocations.contains("--file"));
}

#[test]
fn build_forwards_output_and_false_push_to_the_terminal_engine_build() {
    let harness = RuntimeHarness::new();
    let workspace = harness.workspace();
    let config_dir = workspace.join(".devcontainer");
    fs::create_dir_all(&config_dir).expect("workspace config dir");
    let dockerfile = config_dir.join("Dockerfile");
    fs::write(&dockerfile, "FROM scratch\n").expect("dockerfile");
    write_devcontainer_config(
        &workspace,
        "{\n  \"build\": {\n    \"dockerfile\": \"Dockerfile\",\n    \"context\": \".\"\n  }\n}\n",
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
            "example/native-build:oci-output",
            "--output",
            "type=oci,dest=/tmp/native-output.tar",
            "--push",
            "false",
        ],
        &[],
    );

    assert!(output.status.success(), "{output:?}");
    let payload = harness.parse_stdout_json(&output);
    assert_eq!(payload["outcome"], "success");
    assert_eq!(payload["imageName"], "example/native-build:oci-output");
    assert_eq!(
        harness.read_engine_argv(),
        vec![vec![
            "build".to_string(),
            "--tag".to_string(),
            "example/native-build:oci-output".to_string(),
            "--file".to_string(),
            dockerfile.display().to_string(),
            "--build-arg".to_string(),
            "BUILDKIT_INLINE_CACHE=1".to_string(),
            "--output".to_string(),
            "type=oci,dest=/tmp/native-output.tar".to_string(),
            config_dir.join(".").display().to_string(),
        ]]
    );
}

#[test]
fn build_rejects_output_with_effective_push_before_engine_work() {
    let harness = RuntimeHarness::new();
    let workspace = harness.workspace();
    let config_dir = workspace.join(".devcontainer");
    fs::create_dir_all(&config_dir).expect("workspace config dir");
    fs::write(config_dir.join("Dockerfile"), "FROM scratch\n").expect("dockerfile");
    write_devcontainer_config(
        &workspace,
        "{\n  \"build\": {\n    \"dockerfile\": \"Dockerfile\"\n  }\n}\n",
    );

    let fake_podman = harness.fake_podman.to_string_lossy().to_string();
    let output = harness.run(
        &[
            "build",
            "--docker-path",
            fake_podman.as_str(),
            "--workspace-folder",
            workspace.to_string_lossy().as_ref(),
            "--output",
            "type=oci,dest=/tmp/native-output.tar",
            "--push",
            "true",
        ],
        &[],
    );

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert_eq!(
        String::from_utf8(output.stderr)
            .expect("utf8 stderr")
            .trim(),
        "--push true cannot be used with --output."
    );
    assert!(!harness.log_dir.join("engine-argv.log").exists());
}

#[test]
fn build_passes_configured_build_args_to_the_engine() {
    let harness = RuntimeHarness::new();
    let workspace = harness.workspace();
    fs::create_dir_all(workspace.join(".devcontainer")).expect("workspace config dir");
    fs::write(
        workspace.join(".devcontainer").join("Dockerfile"),
        "FROM scratch\nARG VARIANT\nARG TOOLCHAIN\n",
    )
    .expect("dockerfile");
    write_devcontainer_config(
        &workspace,
        "{\n  \"build\": {\n    \"dockerfile\": \"Dockerfile\",\n    \"context\": \".devcontainer\",\n    \"args\": {\n      \"VARIANT\": \"bookworm\",\n      \"TOOLCHAIN\": \"stable\"\n    }\n  }\n}\n",
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
            "example/native-build:args",
        ],
        &[],
    );

    assert!(output.status.success(), "{output:?}");
    let invocations = harness.read_invocations();
    assert!(invocations.contains("--build-arg VARIANT=bookworm"));
    assert!(invocations.contains("--build-arg TOOLCHAIN=stable"));
}

#[test]
fn build_never_buildkit_sets_engine_env() {
    let harness = RuntimeHarness::new();
    let workspace = harness.workspace();
    fs::create_dir_all(workspace.join(".devcontainer")).expect("workspace config dir");
    fs::write(
        workspace.join(".devcontainer").join("Dockerfile"),
        "FROM scratch\n",
    )
    .expect("dockerfile");
    write_devcontainer_config(
        &workspace,
        "{\n  \"build\": {\n    \"dockerfile\": \"Dockerfile\",\n    \"context\": \".devcontainer\"\n  }\n}\n",
    );

    let fake_podman = harness.fake_podman.to_string_lossy().to_string();
    let output = harness.run(
        &[
            "build",
            "--docker-path",
            fake_podman.as_str(),
            "--workspace-folder",
            workspace.to_string_lossy().as_ref(),
            "--buildkit",
            "never",
        ],
        &[],
    );

    assert!(output.status.success(), "{output:?}");
    let build_env =
        fs::read_to_string(harness.log_dir.join("build-env.log")).expect("build env log");
    assert!(build_env.contains("DOCKER_BUILDKIT=0"));
}

#[test]
fn up_honors_build_no_cache() {
    let harness = RuntimeHarness::new();
    let workspace = harness.workspace();
    fs::create_dir_all(workspace.join(".devcontainer")).expect("workspace config dir");
    fs::write(
        workspace.join(".devcontainer").join("Dockerfile"),
        "FROM scratch\n",
    )
    .expect("dockerfile");
    write_devcontainer_config(
        &workspace,
        "{\n  \"build\": {\n    \"dockerfile\": \"Dockerfile\",\n    \"context\": \".devcontainer\"\n  }\n}\n",
    );

    let fake_podman = harness.fake_podman.to_string_lossy().to_string();
    let output = harness.run(
        &[
            "up",
            "--docker-path",
            fake_podman.as_str(),
            "--workspace-folder",
            workspace.to_string_lossy().as_ref(),
            "--build-no-cache",
        ],
        &[("FAKE_PODMAN_PS_DISABLE_DEFAULT", "1")],
    );

    assert!(output.status.success(), "{output:?}");
    let invocations = harness.read_invocations();
    assert!(invocations.contains("build "));
    assert!(invocations.contains("--no-cache"));
    if cfg!(target_os = "linux") {
        assert!(invocations.contains("image inspect --format"));
    }
}
