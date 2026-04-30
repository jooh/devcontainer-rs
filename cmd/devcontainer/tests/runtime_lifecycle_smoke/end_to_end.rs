//! End-to-end lifecycle smoke coverage across up, exec, run-user-commands, and set-up.

use std::fs;

use crate::support::runtime_harness::{write_devcontainer_config, RuntimeHarness};

#[test]
fn lifecycle_flow_runs_across_up_exec_run_user_commands_and_set_up() {
    let harness = RuntimeHarness::new();
    let workspace = harness.workspace();
    fs::create_dir_all(&workspace).expect("workspace dir");
    let config_path = write_devcontainer_config(
        &workspace,
        "{\n  \"image\": \"alpine:3.20\",\n  \"workspaceFolder\": \"/workspace\",\n  \"onCreateCommand\": \"echo on-create\",\n  \"updateContentCommand\": \"echo update-content\",\n  \"postCreateCommand\": \"echo post-create\",\n  \"postStartCommand\": \"echo post-start\",\n  \"postAttachCommand\": \"echo post-attach\"\n}\n",
    );

    let fake_podman = harness.fake_podman.to_string_lossy().to_string();
    let up_output = harness.run(
        &[
            "up",
            "--docker-path",
            fake_podman.as_str(),
            "--workspace-folder",
            workspace.to_string_lossy().as_ref(),
        ],
        &[("FAKE_PODMAN_PS_DISABLE_DEFAULT", "1")],
    );

    assert!(up_output.status.success(), "{up_output:?}");
    let up_payload = harness.parse_stdout_json(&up_output);
    assert_eq!(up_payload["containerId"], "fake-container-id");

    let exec_output = harness.run(
        &[
            "exec",
            "--docker-path",
            fake_podman.as_str(),
            "--container-id",
            "fake-container-id",
            "/bin/echo",
            "hello-from-lifecycle-flow",
        ],
        &[],
    );

    assert!(exec_output.status.success(), "{exec_output:?}");
    assert_eq!(
        String::from_utf8(exec_output.stdout).expect("utf8 stdout"),
        "hello-from-lifecycle-flow\n"
    );

    let run_user_commands_output = harness.run(
        &[
            "run-user-commands",
            "--docker-path",
            fake_podman.as_str(),
            "--workspace-folder",
            workspace.to_string_lossy().as_ref(),
        ],
        &[("FAKE_PODMAN_PS_OUTPUT", "fake-container-id")],
    );

    assert!(
        run_user_commands_output.status.success(),
        "{run_user_commands_output:?}"
    );

    let set_up_output = harness.run(
        &[
            "set-up",
            "--docker-path",
            fake_podman.as_str(),
            "--container-id",
            "fake-container-id",
            "--config",
            config_path.to_string_lossy().as_ref(),
        ],
        &[],
    );

    assert!(set_up_output.status.success(), "{set_up_output:?}");

    let invocations = harness.read_invocations();
    assert!(invocations.contains("run "));
    assert!(invocations.contains("exec -i --workdir /workspace"));
    assert!(invocations.contains("-e HOME=/root"));
    assert!(invocations.contains("fake-container-id /bin/echo hello-from-lifecycle-flow"));
    assert!(invocations.contains("ps -q "));

    let exec_log = harness.read_exec_log();
    assert!(exec_log.contains("/bin/sh -lc echo on-create"));
    assert!(exec_log.contains("/bin/sh -lc echo update-content"));
    assert!(exec_log.contains("/bin/sh -lc echo post-create"));
    assert!(exec_log.contains("/bin/sh -lc echo post-start"));
    assert!(exec_log.contains("/bin/sh -lc echo post-attach"));
}
