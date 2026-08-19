//! Runtime container smoke tests for basic up and workspace-mount behavior.

use std::fs;
use std::process::Command;

use crate::support::runtime_harness::{write_devcontainer_config, RuntimeHarness};

#[test]
fn up_starts_a_container_and_exec_runs_inside_it() {
    let harness = RuntimeHarness::new();
    let workspace = harness.workspace();
    fs::create_dir_all(&workspace).expect("workspace dir");
    write_devcontainer_config(
        &workspace,
        "{\n  \"image\": \"alpine:3.20\",\n  \"workspaceFolder\": \"/workspace\",\n  \"postCreateCommand\": \"echo ready\"\n}\n",
    );

    let fake_podman = harness.fake_podman.to_string_lossy().to_string();
    let up_output = harness.run(
        &[
            "up",
            "--docker-path",
            fake_podman.as_str(),
            "--workspace-folder",
            workspace.to_string_lossy().as_ref(),
            "--include-configuration",
        ],
        &[("FAKE_PODMAN_PS_DISABLE_DEFAULT", "1")],
    );

    assert!(up_output.status.success(), "{up_output:?}");
    let up_payload = harness.parse_stdout_json(&up_output);
    assert_eq!(up_payload["containerId"], "fake-container-id");
    assert_eq!(up_payload["remoteWorkspaceFolder"], "/workspace");
    assert_eq!(up_payload["configuration"]["image"], "alpine:3.20");

    let exec_output = harness.run(
        &[
            "exec",
            "--docker-path",
            fake_podman.as_str(),
            "--workspace-folder",
            workspace.to_string_lossy().as_ref(),
            "/bin/echo",
            "hello-from-container",
        ],
        &[],
    );

    assert!(exec_output.status.success(), "{exec_output:?}");
    assert_eq!(
        String::from_utf8(exec_output.stdout).expect("utf8 stdout"),
        "hello-from-container\n"
    );

    let invocations = harness.read_invocations();
    assert!(invocations.contains("run "));
    assert!(invocations.contains("--mount type=bind,source="));
    assert!(invocations.contains(",target=/workspace"));
    assert!(invocations.contains("exec -i --workdir /workspace"));
    assert!(invocations.contains("-e HOME=/root"));
    assert!(invocations.contains("fake-container-id /bin/echo hello-from-container"));

    let exec_log = harness.read_exec_log();
    assert!(exec_log.contains("/bin/sh -lc echo ready"));
}

#[test]
fn up_pull_always_refreshes_the_source_without_pulling_the_final_tag() {
    let harness = RuntimeHarness::new();
    let workspace = harness.workspace();
    fs::create_dir_all(&workspace).expect("workspace dir");
    write_devcontainer_config(&workspace, "{\n  \"image\": \"alpine:3.20\"\n}\n");

    let fake_podman = harness.fake_podman.to_string_lossy().to_string();
    let output = harness.run(
        &[
            "up",
            "--docker-path",
            fake_podman.as_str(),
            "--workspace-folder",
            workspace.to_string_lossy().as_ref(),
            "--pull-always",
        ],
        &[("FAKE_PODMAN_PS_DISABLE_DEFAULT", "1")],
    );

    assert!(output.status.success(), "{output:?}");
    let invocations = harness.read_invocations();
    assert!(invocations.contains("pull alpine:3.20"), "{invocations}");
    assert!(
        !invocations.contains("run -d --pull always"),
        "{invocations}"
    );
}

#[test]
fn up_pull_always_honors_explicit_true_and_false_values() {
    for (arguments, should_pull) in [
        (vec!["--pull-always", "true"], true),
        (vec!["--pull-always=true"], true),
        (vec!["--pull-always", "false"], false),
        (vec!["--pull-always=false"], false),
    ] {
        let harness = RuntimeHarness::new();
        let workspace = harness.workspace();
        fs::create_dir_all(&workspace).expect("workspace dir");
        write_devcontainer_config(&workspace, "{\n  \"image\": \"alpine:3.20\"\n}\n");
        let fake_podman = harness.fake_podman.to_string_lossy().to_string();
        let workspace_arg = workspace.to_string_lossy().to_string();
        let mut args = vec![
            "up",
            "--docker-path",
            fake_podman.as_str(),
            "--workspace-folder",
            workspace_arg.as_str(),
        ];
        args.extend(arguments.iter().copied());

        let output = harness.run(&args, &[("FAKE_PODMAN_PS_DISABLE_DEFAULT", "1")]);

        assert!(output.status.success(), "{arguments:?}: {output:?}");
        let invocations = harness.read_invocations();
        assert_eq!(
            invocations.contains("pull alpine:3.20"),
            should_pull,
            "{arguments:?}: {invocations}"
        );
        assert!(
            !invocations.contains("run -d --pull always"),
            "{invocations}"
        );
    }
}

#[test]
fn up_succeeds_with_env_backed_runtime_defaults() {
    let harness = RuntimeHarness::new();
    let workspace = harness.workspace();
    fs::create_dir_all(&workspace).expect("workspace dir");
    write_devcontainer_config(
        &workspace,
        "{\n  \"image\": \"alpine:3.20\",\n  \"workspaceFolder\": \"/workspace\"\n}\n",
    );

    let fake_podman = harness.fake_podman.to_string_lossy().to_string();
    let user_data = harness.root.join("env-user-data");
    let output = harness.run(
        &[
            "up",
            "--workspace-folder",
            workspace.to_string_lossy().as_ref(),
        ],
        &[
            ("DEVCONTAINER_DOCKER_PATH", fake_podman.as_str()),
            ("DEVCONTAINER_BUILDKIT", "never"),
            (
                "DEVCONTAINER_USER_DATA_FOLDER",
                user_data.to_string_lossy().as_ref(),
            ),
            (
                "DEVCONTAINER_CONTAINER_DATA_FOLDER",
                "/tmp/env-container-data",
            ),
            ("DEVCONTAINER_GPU_AVAILABILITY", "none"),
            ("DEVCONTAINER_UPDATE_REMOTE_USER_UID_DEFAULT", "never"),
            ("DEVCONTAINER_MOUNT_WORKSPACE_GIT_ROOT", "false"),
            ("DEVCONTAINER_MOUNT_GIT_WORKTREE_COMMON_DIR", "false"),
            ("DEVCONTAINER_WORKSPACE_MOUNT_CONSISTENCY", "delegated"),
            ("FAKE_PODMAN_PS_DISABLE_DEFAULT", "1"),
        ],
    );

    assert!(output.status.success(), "{output:?}");
    let payload = harness.parse_stdout_json(&output);
    assert_eq!(payload["containerId"], "fake-container-id");
    let invocations = harness.read_invocations();
    assert!(invocations.contains("run "));
}

#[test]
fn explicit_cli_runtime_flags_override_conflicting_env_defaults() {
    let harness = RuntimeHarness::new();
    let workspace = harness.workspace();
    fs::create_dir_all(&workspace).expect("workspace dir");
    write_devcontainer_config(
        &workspace,
        "{\n  \"image\": \"alpine:3.20\",\n  \"workspaceFolder\": \"/workspace\",\n  \"hostRequirements\": { \"gpu\": \"optional\" }\n}\n",
    );

    let fake_podman = harness.fake_podman.to_string_lossy().to_string();
    let output = harness.run(
        &[
            "up",
            "--docker-path",
            fake_podman.as_str(),
            "--gpu-availability",
            "none",
            "--workspace-folder",
            workspace.to_string_lossy().as_ref(),
        ],
        &[
            ("DEVCONTAINER_DOCKER_PATH", "/path/that/does/not/exist"),
            ("DEVCONTAINER_GPU_AVAILABILITY", "all"),
            ("FAKE_PODMAN_PS_DISABLE_DEFAULT", "1"),
        ],
    );

    assert!(output.status.success(), "{output:?}");
    let invocations = harness.read_invocations();
    assert!(!invocations.contains("--gpus all"));
}

#[test]
fn up_reports_missing_default_engine_with_actionable_guidance() {
    let harness = RuntimeHarness::new();
    let workspace = harness.workspace();
    fs::create_dir_all(&workspace).expect("workspace dir");
    write_devcontainer_config(
        &workspace,
        "{\n  \"image\": \"alpine:3.20\",\n  \"workspaceFolder\": \"/workspace\"\n}\n",
    );

    let path = harness.root.to_string_lossy().to_string();
    let output = harness.run(
        &[
            "up",
            "--workspace-folder",
            workspace.to_string_lossy().as_ref(),
        ],
        &[
            ("PATH", path.as_str()),
            ("FAKE_PODMAN_PS_DISABLE_DEFAULT", "1"),
        ],
    );

    assert!(!output.status.success(), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("utf8 stderr");
    assert!(stderr.contains("Container engine executable not found: docker"));
    assert!(stderr.contains("--docker-path podman"));
    assert!(!stderr.contains("os error 2"));
    assert!(!stderr.contains("No such file or directory"));
}

#[test]
fn up_uses_workspace_mount_target_for_remote_workdir_when_workspace_folder_is_omitted() {
    let harness = RuntimeHarness::new();
    let workspace = harness.workspace();
    fs::create_dir_all(&workspace).expect("workspace dir");
    write_devcontainer_config(
        &workspace,
        "{\n  \"image\": \"alpine:3.20\",\n  \"workspaceMount\": \"type=bind,source=/host/project,target=/custom-target\",\n  \"postCreateCommand\": \"echo ready\"\n}\n",
    );

    let fake_podman = harness.fake_podman.to_string_lossy().to_string();
    let output = harness.run(
        &[
            "up",
            "--docker-path",
            fake_podman.as_str(),
            "--workspace-folder",
            workspace.to_string_lossy().as_ref(),
        ],
        &[("FAKE_PODMAN_PS_DISABLE_DEFAULT", "1")],
    );

    assert!(output.status.success(), "{output:?}");
    let payload = harness.parse_stdout_json(&output);
    assert_eq!(payload["remoteWorkspaceFolder"], "/custom-target");
    let invocations = harness.read_invocations();
    assert!(invocations.contains("exec --workdir /custom-target"));
    assert!(invocations.contains("-e HOME=/root"));
    assert!(invocations.contains("fake-container-id /bin/sh -lc echo ready"));
}

#[test]
fn up_applies_feature_runtime_metadata_to_container_creation() {
    let harness = RuntimeHarness::new();
    let workspace = harness.workspace();
    let feature_dir = workspace.join(".devcontainer").join("local-feature");
    fs::create_dir_all(&feature_dir).expect("feature dir");
    fs::write(
        feature_dir.join("devcontainer-feature.json"),
        "{\n  \"id\": \"local-feature\",\n  \"name\": \"Local Feature\",\n  \"version\": \"1.0.0\",\n  \"containerEnv\": {\n    \"FEATURE_FLAG\": \"enabled\"\n  },\n  \"init\": true,\n  \"privileged\": true,\n  \"capAdd\": [\"SYS_ADMIN\"],\n  \"securityOpt\": [\"seccomp=unconfined\"],\n  \"postCreateCommand\": \"echo feature-ready\"\n}\n",
    )
    .expect("feature manifest");
    fs::write(feature_dir.join("install.sh"), "#!/bin/sh\nset -eu\n").expect("install script");
    write_devcontainer_config(
        &workspace,
        "{\n  \"image\": \"alpine:3.20\",\n  \"workspaceFolder\": \"/workspace\",\n  \"features\": {\n    \"./local-feature\": {}\n  }\n}\n",
    );

    let fake_podman = harness.fake_podman.to_string_lossy().to_string();
    let output = harness.run(
        &[
            "up",
            "--docker-path",
            fake_podman.as_str(),
            "--workspace-folder",
            workspace.to_string_lossy().as_ref(),
            "--include-configuration",
        ],
        &[("FAKE_PODMAN_PS_DISABLE_DEFAULT", "1")],
    );

    assert!(output.status.success(), "{output:?}");
    let payload = harness.parse_stdout_json(&output);
    assert_eq!(
        payload["configuration"]["containerEnv"]["FEATURE_FLAG"],
        "enabled"
    );
    assert_eq!(payload["configuration"]["init"], true);
    assert_eq!(payload["configuration"]["privileged"], true);

    let invocations = harness.read_invocations();
    assert!(invocations.contains("--init"));
    assert!(invocations.contains("--privileged"));
    assert!(invocations.contains("--cap-add SYS_ADMIN"));
    assert!(invocations.contains("--security-opt seccomp=unconfined"));
    assert!(invocations.contains("-e FEATURE_FLAG=enabled"));

    let exec_log = harness.read_exec_log();
    assert!(exec_log.contains("/bin/sh -lc echo feature-ready"));
}

#[test]
fn up_emits_config_mounts_before_cli_mounts() {
    let harness = RuntimeHarness::new();
    let workspace = harness.workspace();
    fs::create_dir_all(&workspace).expect("workspace dir");
    write_devcontainer_config(
        &workspace,
        "{\n  \"image\": \"alpine:3.20\",\n  \"mounts\": [{\n    \"type\": \"bind\",\n    \"source\": \"/tmp/config-src\",\n    \"target\": \"/tmp/config-dst\"\n  }]\n}\n",
    );

    let fake_podman = harness.fake_podman.to_string_lossy().to_string();
    let output = harness.run(
        &[
            "up",
            "--docker-path",
            fake_podman.as_str(),
            "--workspace-folder",
            workspace.to_string_lossy().as_ref(),
            "--mount",
            "type=volume,source=cli-cache,target=/cli-cache",
        ],
        &[("FAKE_PODMAN_PS_DISABLE_DEFAULT", "1")],
    );

    assert!(output.status.success(), "{output:?}");

    let invocations = harness.read_invocations();
    let config_mount = "--mount type=bind,source=/tmp/config-src,target=/tmp/config-dst";
    let cli_mount = "--mount type=volume,source=cli-cache,target=/cli-cache";
    let config_position = invocations.find(config_mount).expect("config mount");
    let cli_position = invocations.find(cli_mount).expect("cli mount");

    assert!(
        config_position < cli_position,
        "expected config mounts before CLI mounts: {invocations}"
    );
}

#[test]
fn up_rejects_invalid_cli_mount_before_engine_invocation() {
    let harness = RuntimeHarness::new();
    let workspace = harness.workspace();
    fs::create_dir_all(&workspace).expect("workspace dir");
    write_devcontainer_config(&workspace, "{\n  \"image\": \"alpine:3.20\"\n}\n");

    let fake_podman = harness.fake_podman.to_string_lossy().to_string();
    let output = harness.run(
        &[
            "up",
            "--docker-path",
            fake_podman.as_str(),
            "--workspace-folder",
            workspace.to_string_lossy().as_ref(),
            "--mount",
            "invalid-mount",
        ],
        &[("FAKE_PODMAN_PS_DISABLE_DEFAULT", "1")],
    );

    assert!(!output.status.success(), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("utf8 stderr");
    assert!(
        stderr.contains("Invalid value for option --mount"),
        "{stderr}"
    );

    let invocation_log = harness.log_dir.join("invocations.log");
    assert!(
        !invocation_log.exists(),
        "unexpected engine invocation log: {}",
        fs::read_to_string(&invocation_log).unwrap_or_default()
    );
}

#[test]
fn up_adds_gpu_flags_when_required_and_gpu_availability_is_all() {
    let harness = RuntimeHarness::new();
    let workspace = harness.workspace();
    fs::create_dir_all(&workspace).expect("workspace dir");
    write_devcontainer_config(
        &workspace,
        "{\n  \"image\": \"alpine:3.20\",\n  \"hostRequirements\": {\n    \"gpu\": \"required\"\n  }\n}\n",
    );

    let fake_podman = harness.fake_podman.to_string_lossy().to_string();
    let output = harness.run(
        &[
            "up",
            "--docker-path",
            fake_podman.as_str(),
            "--workspace-folder",
            workspace.to_string_lossy().as_ref(),
            "--gpu-availability",
            "all",
        ],
        &[("FAKE_PODMAN_PS_DISABLE_DEFAULT", "1")],
    );

    assert!(output.status.success(), "{output:?}");
    let invocations = harness.read_invocations();
    assert!(invocations.contains("--gpus all"));
}

#[test]
fn up_mounts_git_root_by_default_and_uses_subfolder_workdir() {
    let harness = RuntimeHarness::new();
    let repo_root = harness.root.join("repo");
    let workspace = repo_root.join("packages").join("app");
    fs::create_dir_all(&workspace).expect("workspace dir");
    init_git_repo(&repo_root);
    let expected_repo_root = repo_root
        .canonicalize()
        .unwrap_or_else(|_| repo_root.clone());
    write_devcontainer_config(
        &workspace,
        "{\n  \"image\": \"alpine:3.20\",\n  \"postCreateCommand\": \"echo ready\"\n}\n",
    );

    let fake_podman = harness.fake_podman.to_string_lossy().to_string();
    let output = harness.run(
        &[
            "up",
            "--docker-path",
            fake_podman.as_str(),
            "--workspace-folder",
            workspace.to_string_lossy().as_ref(),
        ],
        &[("FAKE_PODMAN_PS_DISABLE_DEFAULT", "1")],
    );

    assert!(output.status.success(), "{output:?}");
    let payload = harness.parse_stdout_json(&output);
    assert_eq!(
        payload["remoteWorkspaceFolder"],
        "/workspaces/repo/packages/app"
    );

    let invocations = harness.read_invocations();
    assert!(invocations.contains(&format!(
        "--mount type=bind,source={},target=/workspaces/repo",
        expected_repo_root.display()
    )));
    assert!(invocations.contains("exec --workdir /workspaces/repo/packages/app"));
    assert!(invocations.contains("-e HOME=/root"));
    assert!(invocations.contains("fake-container-id /bin/sh -lc echo ready"));
}

#[test]
fn up_honors_workspace_mount_flags_for_nested_workspaces() {
    let harness = RuntimeHarness::new();
    let repo_root = harness.root.join("repo");
    let workspace = repo_root.join("packages").join("app");
    fs::create_dir_all(&workspace).expect("workspace dir");
    init_git_repo(&repo_root);
    let expected_workspace = workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.clone());
    write_devcontainer_config(
        &workspace,
        "{\n  \"image\": \"alpine:3.20\",\n  \"postCreateCommand\": \"echo ready\"\n}\n",
    );

    let fake_podman = harness.fake_podman.to_string_lossy().to_string();
    let output = harness.run(
        &[
            "up",
            "--docker-path",
            fake_podman.as_str(),
            "--workspace-folder",
            workspace.to_string_lossy().as_ref(),
            "--mount-workspace-git-root",
            "false",
            "--workspace-mount-consistency",
            "delegated",
        ],
        &[("FAKE_PODMAN_PS_DISABLE_DEFAULT", "1")],
    );

    assert!(output.status.success(), "{output:?}");
    let payload = harness.parse_stdout_json(&output);
    assert_eq!(payload["remoteWorkspaceFolder"], "/workspaces/app");

    let expected_mount = if std::env::consts::OS == "linux" {
        format!(
            "--mount type=bind,source={},target=/workspaces/app",
            expected_workspace.display()
        )
    } else {
        format!(
            "--mount type=bind,source={},target=/workspaces/app,consistency=delegated",
            expected_workspace.display()
        )
    };
    let invocations = harness.read_invocations();
    assert!(invocations.contains(&expected_mount));
}

#[test]
fn up_mounts_git_worktree_common_dir_when_requested() {
    let harness = RuntimeHarness::new();
    let repo_root = harness.root.join("repo");
    let worktree_root = harness.root.join("worktrees").join("feature");
    let workspace = worktree_root.join("packages").join("app");
    init_git_repo_with_commit(&repo_root);
    add_relative_git_worktree(&repo_root, &worktree_root);
    fs::create_dir_all(&workspace).expect("workspace dir");
    let expected_worktree_root = worktree_root
        .canonicalize()
        .unwrap_or_else(|_| worktree_root.clone());
    let expected_repo_git_dir = repo_root
        .join(".git")
        .canonicalize()
        .unwrap_or_else(|_| repo_root.join(".git"));
    write_devcontainer_config(
        &workspace,
        "{\n  \"image\": \"alpine:3.20\",\n  \"postCreateCommand\": \"echo ready\"\n}\n",
    );

    let fake_podman = harness.fake_podman.to_string_lossy().to_string();
    let output = harness.run(
        &[
            "up",
            "--docker-path",
            fake_podman.as_str(),
            "--workspace-folder",
            workspace.to_string_lossy().as_ref(),
            "--mount-git-worktree-common-dir",
        ],
        &[("FAKE_PODMAN_PS_DISABLE_DEFAULT", "1")],
    );

    assert!(output.status.success(), "{output:?}");
    let payload = harness.parse_stdout_json(&output);
    assert_eq!(
        payload["remoteWorkspaceFolder"],
        "/workspaces/worktrees/feature/packages/app"
    );

    let invocations = harness.read_invocations();
    assert!(invocations.contains(&format!(
        "--mount type=bind,source={},target=/workspaces/worktrees/feature",
        expected_worktree_root.display()
    )));
    assert!(invocations.contains(&format!(
        "--mount type=bind,source={},target=/workspaces/repo/.git",
        expected_repo_git_dir.display()
    )));
}

#[test]
fn up_rebases_git_worktree_common_dir_for_configured_workspace_folder() {
    let harness = RuntimeHarness::new();
    let repo_root = harness.root.join("repo");
    let worktree_root = harness.root.join("worktrees").join("feature");
    init_git_repo_with_commit(&repo_root);
    add_relative_git_worktree(&repo_root, &worktree_root);
    let expected_worktree_root = worktree_root
        .canonicalize()
        .unwrap_or_else(|_| worktree_root.clone());
    let expected_repo_git_dir = repo_root
        .join(".git")
        .canonicalize()
        .unwrap_or_else(|_| repo_root.join(".git"));
    write_devcontainer_config(
        &worktree_root,
        "{\n  \"image\": \"alpine:3.20\",\n  \"workspaceFolder\": \"/workspace\",\n  \"postCreateCommand\": \"echo ready\"\n}\n",
    );

    let fake_podman = harness.fake_podman.to_string_lossy().to_string();
    let output = harness.run(
        &[
            "up",
            "--docker-path",
            fake_podman.as_str(),
            "--workspace-folder",
            worktree_root.to_string_lossy().as_ref(),
            "--mount-git-worktree-common-dir",
        ],
        &[("FAKE_PODMAN_PS_DISABLE_DEFAULT", "1")],
    );

    assert!(output.status.success(), "{output:?}");
    let payload = harness.parse_stdout_json(&output);
    assert_eq!(payload["remoteWorkspaceFolder"], "/workspace");

    let invocations = harness.read_invocations();
    assert!(invocations.contains(&format!(
        "--mount type=bind,source={},target=/workspace",
        expected_worktree_root.display()
    )));
    assert!(invocations.contains(&format!(
        "--mount type=bind,source={},target=/repo/.git",
        expected_repo_git_dir.display()
    )));
}

fn init_git_repo(root: &std::path::Path) {
    let status = Command::new("git")
        .args(["init", "--quiet"])
        .current_dir(root)
        .status()
        .expect("git init");
    assert!(status.success(), "git init failed: {status:?}");
}

fn init_git_repo_with_commit(root: &std::path::Path) {
    fs::create_dir_all(root).expect("repo dir");
    init_git_repo(root);
    fs::write(root.join("README.md"), "hello\n").expect("readme");

    let add_status = Command::new("git")
        .args(["add", "README.md"])
        .current_dir(root)
        .status()
        .expect("git add");
    assert!(add_status.success(), "git add failed: {add_status:?}");

    let commit_status = Command::new("git")
        .args([
            "-c",
            "user.name=Devcontainer Tests",
            "-c",
            "user.email=devcontainer-tests@example.com",
            "commit",
            "--quiet",
            "-m",
            "init",
        ])
        .current_dir(root)
        .status()
        .expect("git commit");
    assert!(
        commit_status.success(),
        "git commit failed: {commit_status:?}"
    );
}

fn add_relative_git_worktree(repo_root: &std::path::Path, worktree_root: &std::path::Path) {
    if let Some(parent) = worktree_root.parent() {
        fs::create_dir_all(parent).expect("worktree parent");
    }

    let status = Command::new("git")
        .args([
            "worktree",
            "add",
            "--relative-paths",
            worktree_root.to_string_lossy().as_ref(),
            "-b",
            "feature",
        ])
        .current_dir(repo_root)
        .status()
        .expect("git worktree add");
    assert!(status.success(), "git worktree add failed: {status:?}");
}
