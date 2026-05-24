//! Runtime container smoke tests for compose project-name behavior.

use std::fs;
use std::path::Path;

use crate::support::runtime_harness::{write_devcontainer_config, RuntimeHarness};

fn write_executable_script(path: &Path, body: &str) {
    fs::write(path, body).expect("script");
    let mut permissions = fs::metadata(path).expect("script metadata").permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        permissions.set_mode(0o755);
    }
    fs::set_permissions(path, permissions).expect("script permissions");
}

#[test]
fn up_uses_custom_compose_project_name_from_compose_file() {
    let harness = RuntimeHarness::new();
    let workspace = harness.workspace();
    fs::create_dir_all(workspace.join(".devcontainer")).expect("workspace config dir");
    fs::write(
        workspace.join(".devcontainer").join("docker-compose.yml"),
        "name: Custom-Project-Name\nservices:\n  app:\n    image: alpine:3.20\n",
    )
    .expect("compose");
    write_devcontainer_config(
        &workspace,
        "{\n  \"dockerComposeFile\": \"docker-compose.yml\",\n  \"service\": \"app\",\n  \"workspaceFolder\": \"/workspace\"\n}\n",
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
        &[],
    );

    assert!(output.status.success(), "{output:?}");
    let payload = harness.parse_stdout_json(&output);
    assert_eq!(payload["composeProjectName"], "custom-project-name");

    let invocations = harness.read_invocations();
    assert!(invocations.contains("compose --project-name custom-project-name -f "));
}

#[test]
fn up_reads_compose_project_name_from_compose_directory_dotenv() {
    let harness = RuntimeHarness::new();
    let workspace = harness.workspace();
    let outside = harness.root.join("outside");
    fs::create_dir_all(&outside).expect("outside dir");
    fs::create_dir_all(workspace.join(".devcontainer")).expect("workspace config dir");
    fs::write(
        workspace.join(".devcontainer").join(".env"),
        "COMPOSE_PROJECT_NAME=dotenv-project\n",
    )
    .expect("dotenv");
    fs::write(
        workspace.join(".devcontainer").join("docker-compose.yml"),
        "services:\n  app:\n    image: alpine:3.20\n",
    )
    .expect("compose");
    write_devcontainer_config(
        &workspace,
        "{\n  \"dockerComposeFile\": \"docker-compose.yml\",\n  \"service\": \"app\",\n  \"workspaceFolder\": \"/workspace\"\n}\n",
    );

    let fake_podman = harness.fake_podman.to_string_lossy().to_string();
    let output = harness.run_in_dir(
        &[
            "up",
            "--docker-path",
            fake_podman.as_str(),
            "--workspace-folder",
            workspace.to_string_lossy().as_ref(),
        ],
        &[],
        Some(&outside),
    );

    assert!(output.status.success(), "{output:?}");
    let payload = harness.parse_stdout_json(&output);
    assert_eq!(payload["composeProjectName"], "dotenv-project");

    let invocations = harness.read_invocations();
    assert!(invocations.contains("compose --project-name dotenv-project -f "));
}

#[test]
fn up_prefers_compose_project_name_from_process_environment() {
    let harness = RuntimeHarness::new();
    let workspace = harness.workspace();
    fs::create_dir_all(workspace.join(".devcontainer")).expect("workspace config dir");
    fs::write(
        workspace.join(".devcontainer").join(".env"),
        "COMPOSE_PROJECT_NAME=dotenv-project\n",
    )
    .expect("dotenv");
    fs::write(
        workspace.join(".devcontainer").join("docker-compose.yml"),
        "name: file-project\nservices:\n  app:\n    image: alpine:3.20\n",
    )
    .expect("compose");
    write_devcontainer_config(
        &workspace,
        "{\n  \"dockerComposeFile\": \"docker-compose.yml\",\n  \"service\": \"app\",\n  \"workspaceFolder\": \"/workspace\"\n}\n",
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
        &[("COMPOSE_PROJECT_NAME", "Env Project!")],
    );

    assert!(output.status.success(), "{output:?}");
    let payload = harness.parse_stdout_json(&output);
    assert_eq!(payload["composeProjectName"], "envproject");

    let invocations = harness.read_invocations();
    assert!(invocations.contains("compose --project-name envproject -f "));
}

#[test]
fn up_expands_plain_dollar_compose_project_names() {
    let harness = RuntimeHarness::new();
    let workspace = harness.workspace();
    fs::create_dir_all(workspace.join(".devcontainer")).expect("workspace config dir");
    fs::write(
        workspace.join(".devcontainer").join("docker-compose.yml"),
        "name: $CUSTOM_PROJECT\nservices:\n  app:\n    image: alpine:3.20\n",
    )
    .expect("compose");
    write_devcontainer_config(
        &workspace,
        "{\n  \"dockerComposeFile\": \"docker-compose.yml\",\n  \"service\": \"app\",\n  \"workspaceFolder\": \"/workspace\"\n}\n",
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
        &[("CUSTOM_PROJECT", "FromEnv_Project")],
    );

    assert!(output.status.success(), "{output:?}");
    let payload = harness.parse_stdout_json(&output);
    assert_eq!(payload["composeProjectName"], "fromenv_project");

    let invocations = harness.read_invocations();
    assert!(invocations.contains("compose --project-name fromenv_project -f "));
}

#[test]
fn up_uses_default_compose_files_when_docker_compose_file_array_is_empty() {
    let harness = RuntimeHarness::new();
    let workspace = harness.workspace();
    fs::create_dir_all(workspace.join(".devcontainer")).expect("workspace config dir");
    fs::write(
        workspace.join("docker-compose.yml"),
        "services:\n  app:\n    image: alpine:3.20\n",
    )
    .expect("compose");
    fs::write(
        workspace.join("docker-compose.override.yml"),
        "services:\n  app:\n    environment:\n      EXTRA: override\n",
    )
    .expect("compose override");
    write_devcontainer_config(
        &workspace,
        "{\n  \"dockerComposeFile\": [],\n  \"service\": \"app\",\n  \"workspaceFolder\": \"/workspace\"\n}\n",
    );

    let fake_podman = harness.fake_podman.to_string_lossy().to_string();
    let output = harness.run_in_dir(
        &[
            "up",
            "--docker-path",
            fake_podman.as_str(),
            "--workspace-folder",
            workspace.to_string_lossy().as_ref(),
        ],
        &[],
        Some(&workspace),
    );

    assert!(output.status.success(), "{output:?}");
    let invocations = harness.read_invocations();
    let compose_file = workspace
        .join("docker-compose.yml")
        .canonicalize()
        .unwrap_or_else(|_| workspace.join("docker-compose.yml"));
    let override_file = workspace
        .join("docker-compose.override.yml")
        .canonicalize()
        .unwrap_or_else(|_| workspace.join("docker-compose.override.yml"));
    assert!(
        invocations.contains(&format!(
            " -f {} -f {} ",
            compose_file.display(),
            override_file.display()
        )),
        "{invocations}"
    );
}

#[test]
fn up_resolves_compose_file_env_paths_relative_to_workspace_when_array_is_empty() {
    let harness = RuntimeHarness::new();
    let workspace = harness.workspace();
    let compose_dir = workspace.join("relative-compose");
    fs::create_dir_all(&compose_dir).expect("compose dir");
    fs::create_dir_all(workspace.join(".devcontainer")).expect("workspace config dir");
    fs::write(
        compose_dir.join("docker-compose.yml"),
        "services:\n  app:\n    image: alpine:3.20\n",
    )
    .expect("compose");
    write_devcontainer_config(
        &workspace,
        "{\n  \"dockerComposeFile\": [],\n  \"service\": \"app\",\n  \"workspaceFolder\": \"/workspace\"\n}\n",
    );

    let fake_podman = harness.fake_podman.to_string_lossy().to_string();
    let output = harness.run_in_dir(
        &[
            "up",
            "--docker-path",
            fake_podman.as_str(),
            "--workspace-folder",
            workspace.to_string_lossy().as_ref(),
        ],
        &[("COMPOSE_FILE", "relative-compose/docker-compose.yml")],
        Some(&workspace),
    );

    assert!(output.status.success(), "{output:?}");
    let invocations = harness.read_invocations();
    let compose_file = compose_dir
        .join("docker-compose.yml")
        .canonicalize()
        .unwrap_or_else(|_| compose_dir.join("docker-compose.yml"));
    assert!(
        invocations.contains(&format!(" -f {} ", compose_file.display())),
        "{invocations}"
    );
}

#[test]
fn up_falls_back_to_docker_compose_when_docker_compose_subcommand_is_unavailable() {
    let harness = RuntimeHarness::new();
    let workspace = harness.workspace();
    let docker_wrapper = harness.root.join("docker");
    let docker_compose_wrapper = harness.root.join("docker-compose");
    fs::create_dir_all(workspace.join(".devcontainer")).expect("workspace config dir");
    fs::write(
        workspace.join(".devcontainer").join("docker-compose.yml"),
        "services:\n  app:\n    image: alpine:3.20\n",
    )
    .expect("compose");
    write_devcontainer_config(
        &workspace,
        "{\n  \"dockerComposeFile\": \"docker-compose.yml\",\n  \"service\": \"app\",\n  \"workspaceFolder\": \"/workspace\"\n}\n",
    );
    write_executable_script(
        &docker_wrapper,
        &format!(
            "#!/bin/sh\nif [ \"${{1:-}}\" = \"compose\" ]; then\n  exit 1\nfi\nexec \"{}\" \"$@\"\n",
            harness.fake_podman.display()
        ),
    );
    write_executable_script(
        &docker_compose_wrapper,
        &format!(
            "#!/bin/sh\nexec \"{}\" compose \"$@\"\n",
            harness.fake_podman.display()
        ),
    );
    let path = format!(
        "{}:{}",
        harness.root.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let output = harness.run(
        &[
            "up",
            "--workspace-folder",
            workspace.to_string_lossy().as_ref(),
        ],
        &[("PATH", path.as_str())],
    );

    assert!(output.status.success(), "{output:?}");
    let invocations = harness.read_invocations();
    assert!(invocations.contains("compose --project-name workspace_devcontainer -f "));
}
