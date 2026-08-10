//! Smoke tests for dotfiles and personalization lifecycle behavior.

use std::fs;

use crate::support::runtime_harness::{write_devcontainer_config, RuntimeHarness};
use crate::support::workspace_fixture::WorkspaceFixture;

#[test]
fn up_installs_dotfiles_between_post_create_and_post_start() {
    let harness = RuntimeHarness::new();
    let workspace = WorkspaceFixture::new(harness.workspace());
    workspace.init_dotfiles_repo(
        "dotfiles-repo",
        "#!/bin/sh\nset -eu\nprintf 'dotfiles\\n' >> ../order.log\n",
    );
    let order_log = workspace.root().join("order.log");
    write_devcontainer_config(
        workspace.root(),
        "{\n  \"image\": \"alpine:3.20\",\n  \"postCreateCommand\": \"printf 'post-create\\\\n' > /workspaces/workspace/order.log\",\n  \"postStartCommand\": \"printf 'post-start\\\\n' >> /workspaces/workspace/order.log\"\n}\n",
    );

    let fake_podman = harness.fake_podman.to_string_lossy().to_string();
    let output = harness.run(
        &[
            "up",
            "--docker-path",
            fake_podman.as_str(),
            "--workspace-folder",
            workspace.root().to_string_lossy().as_ref(),
            "--dotfiles-repository",
            "./dotfiles-repo",
            "--dotfiles-target-path",
            "./applied-dotfiles",
            "--container-data-folder",
            "./.devcontainer-data",
        ],
        &[("FAKE_PODMAN_PS_DISABLE_DEFAULT", "1")],
    );

    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        fs::read_to_string(order_log).expect("order log"),
        "post-create\ndotfiles\npost-start\n"
    );
    let exec_log = harness.read_exec_log();
    assert!(
        exec_log.contains("/bin/sh -lc printf 'post-create\\n' > /workspaces/workspace/order.log")
    );
    assert!(exec_log.contains("git clone --depth 1"));
    assert!(exec_log.contains("./dotfiles-repo"));
    assert!(exec_log.contains("./applied-dotfiles"));
    assert!(exec_log.contains("printf 'post-start\\n' >> /workspaces/workspace/order.log"));
}

#[test]
fn up_installs_dotfiles_from_environment_defaults() {
    let harness = RuntimeHarness::new();
    let workspace = WorkspaceFixture::new(harness.workspace());
    workspace.init_dotfiles_repo(
        "dotfiles-env-repo",
        "#!/bin/sh\nset -eu\nprintf 'env-dotfiles\\n' >> ../env-order.log\n",
    );
    let order_log = workspace.root().join("env-order.log");
    write_devcontainer_config(
        workspace.root(),
        "{\n  \"image\": \"alpine:3.20\",\n  \"postCreateCommand\": \"printf 'post-create\\\\n' > /workspaces/workspace/env-order.log\",\n  \"postStartCommand\": \"printf 'post-start\\\\n' >> /workspaces/workspace/env-order.log\"\n}\n",
    );

    let fake_podman = harness.fake_podman.to_string_lossy().to_string();
    let output = harness.run(
        &[
            "up",
            "--docker-path",
            fake_podman.as_str(),
            "--workspace-folder",
            workspace.root().to_string_lossy().as_ref(),
            "--container-data-folder",
            "./.devcontainer-data",
        ],
        &[
            ("DEVCONTAINER_DOTFILES_REPOSITORY", "./dotfiles-env-repo"),
            ("DEVCONTAINER_DOTFILES_INSTALL_COMMAND", "install.sh"),
            (
                "DEVCONTAINER_DOTFILES_TARGET_PATH",
                "./env-applied-dotfiles",
            ),
            ("FAKE_PODMAN_PS_DISABLE_DEFAULT", "1"),
        ],
    );

    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        fs::read_to_string(order_log).expect("environment order log"),
        "post-create\nenv-dotfiles\npost-start\n"
    );
    let exec_log = harness.read_exec_log();
    assert!(exec_log.contains("./dotfiles-env-repo"), "{exec_log}");
    assert!(exec_log.contains("./env-applied-dotfiles"), "{exec_log}");
    assert!(exec_log.contains("if [ -f './install.sh' ]"), "{exec_log}");
    assert!(!exec_log.contains("for f in install.sh"), "{exec_log}");
}

#[test]
fn dotfiles_marker_skips_reinstall_on_followup_lifecycle_runs() {
    let harness = RuntimeHarness::new();
    let workspace = WorkspaceFixture::new(harness.workspace());
    workspace.init_dotfiles_repo(
        "dotfiles-repo",
        "#!/bin/sh\nset -eu\nprintf 'dotfiles\\n' >> ../order.log\n",
    );
    let marker_file = workspace
        .root()
        .join(".devcontainer-data")
        .join(".dotfilesMarker");
    let order_log = workspace.root().join("order.log");
    let config_path = write_devcontainer_config(
        workspace.root(),
        "{\n  \"image\": \"alpine:3.20\",\n  \"postCreateCommand\": \"printf 'post-create\\\\n' >> order.log\",\n  \"postAttachCommand\": \"printf 'post-attach\\\\n' >> order.log\"\n}\n",
    );

    let fake_podman = harness.fake_podman.to_string_lossy().to_string();
    let set_up_output = harness.run_in_dir(
        &[
            "set-up",
            "--docker-path",
            fake_podman.as_str(),
            "--container-id",
            "fake-container-id",
            "--config",
            config_path.to_string_lossy().as_ref(),
            "--dotfiles-repository",
            "./dotfiles-repo",
            "--dotfiles-target-path",
            "./applied-dotfiles",
            "--container-data-folder",
            "./.devcontainer-data",
        ],
        &[],
        Some(workspace.root()),
    );
    assert!(set_up_output.status.success(), "{set_up_output:?}");

    let run_user_commands_output = harness.run_in_dir(
        &[
            "run-user-commands",
            "--docker-path",
            fake_podman.as_str(),
            "--workspace-folder",
            workspace.root().to_string_lossy().as_ref(),
            "--dotfiles-repository",
            "./dotfiles-repo",
            "--dotfiles-target-path",
            "./applied-dotfiles",
            "--container-data-folder",
            "./.devcontainer-data",
        ],
        &[("FAKE_PODMAN_PS_OUTPUT", "fake-container-id")],
        Some(workspace.root()),
    );
    assert!(
        run_user_commands_output.status.success(),
        "{run_user_commands_output:?}"
    );

    let order = fs::read_to_string(order_log).expect("order log");
    assert_eq!(order.matches("dotfiles\n").count(), 1, "{order}");
    assert!(marker_file.is_file(), "expected {}", marker_file.display());
}

#[test]
fn run_user_commands_stops_for_personalization_before_post_start_and_post_attach() {
    let harness = RuntimeHarness::new();
    let workspace = WorkspaceFixture::new(harness.workspace());
    workspace.init_dotfiles_repo(
        "dotfiles-repo",
        "#!/bin/sh\nset -eu\nprintf 'dotfiles\\n' >> ../order.log\n",
    );
    let order_log = workspace.root().join("order.log");
    write_devcontainer_config(
        workspace.root(),
        "{\n  \"image\": \"alpine:3.20\",\n  \"postCreateCommand\": \"printf 'post-create\\\\n' > order.log\",\n  \"postStartCommand\": \"printf 'post-start\\\\n' >> order.log\",\n  \"postAttachCommand\": \"printf 'post-attach\\\\n' >> order.log\"\n}\n",
    );

    let fake_podman = harness.fake_podman.to_string_lossy().to_string();
    let output = harness.run_in_dir(
        &[
            "run-user-commands",
            "--docker-path",
            fake_podman.as_str(),
            "--workspace-folder",
            workspace.root().to_string_lossy().as_ref(),
            "--stop-for-personalization",
            "--dotfiles-repository",
            "./dotfiles-repo",
            "--dotfiles-target-path",
            "./applied-dotfiles",
            "--container-data-folder",
            "./.devcontainer-data",
        ],
        &[("FAKE_PODMAN_PS_OUTPUT", "fake-container-id")],
        Some(workspace.root()),
    );

    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        fs::read_to_string(order_log).expect("order log"),
        "post-create\ndotfiles\n"
    );
    let exec_log = harness.read_exec_log();
    assert!(exec_log.contains("/bin/sh -lc printf 'post-create\\n' > order.log"));
    assert!(exec_log.contains("git clone --depth 1"));
    assert!(!exec_log.contains("printf 'post-start\\n' >> order.log"));
    assert!(!exec_log.contains("printf 'post-attach\\n' >> order.log"));
}
