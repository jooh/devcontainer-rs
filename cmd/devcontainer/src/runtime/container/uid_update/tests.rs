//! Unit tests for UID-update image preparation helpers.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::json;

use crate::commands::common::{
    test_env_defaults, DEVCONTAINER_DOCKER_PATH, DEVCONTAINER_UPDATE_REMOTE_USER_UID_DEFAULT,
};
use crate::runtime::context::ResolvedConfig;

use super::{
    command_stdout, inspect_image_details_for_uid_update_once,
    inspect_image_details_without_variant, parse_image_inspect_details, prepare_up_image,
    prepare_up_image_for_platform, should_update_remote_user_uid, uid_update_base_image,
    uid_update_details, uid_update_local_image_name, uid_update_run_args_user,
    unique_uid_update_build_context,
};

#[test]
fn remote_user_uid_update_defaults_to_on_for_supported_platforms() {
    assert!(should_update_remote_user_uid(
        &json!({
            "remoteUser": "vscode"
        }),
        &[],
        true,
    ));
}

#[test]
fn remote_user_uid_update_can_inspect_image_user_when_config_omits_users() {
    assert!(should_update_remote_user_uid(&json!({}), &[], true));
}

#[test]
fn remote_user_uid_update_respects_option_and_config_overrides() {
    assert!(!should_update_remote_user_uid(
        &json!({
            "remoteUser": "vscode"
        }),
        &[
            "--update-remote-user-uid-default".to_string(),
            "off".to_string(),
        ],
        true,
    ));
    assert!(!should_update_remote_user_uid(
        &json!({
            "remoteUser": "vscode",
            "updateRemoteUserUID": false
        }),
        &[],
        true,
    ));
    assert!(!should_update_remote_user_uid(
        &json!({
            "remoteUser": "vscode"
        }),
        &[],
        false,
    ));
}

#[test]
fn remote_user_uid_update_respects_never_default() {
    assert!(!should_update_remote_user_uid(
        &json!({
            "remoteUser": "vscode"
        }),
        &[
            "--update-remote-user-uid-default".to_string(),
            "never".to_string(),
        ],
        true,
    ));
}

#[test]
fn remote_user_uid_update_uses_env_default() {
    let _env = test_env_defaults(&[(DEVCONTAINER_UPDATE_REMOTE_USER_UID_DEFAULT, "never")]);

    assert!(!should_update_remote_user_uid(
        &json!({
            "remoteUser": "vscode"
        }),
        &[],
        true,
    ));
}

#[test]
fn prepare_up_image_wrapper_skips_non_object_configurations() {
    let fixture = FakeEngineFixture::new();
    let workspace = fixture.root.join("workspace");
    fs::create_dir_all(&workspace).expect("workspace dir");
    let resolved = resolved_config(json!(null), &workspace);

    assert_eq!(
        prepare_up_image(&resolved, &fixture.args(), "alpine:3.20")
            .expect("non-object config should skip uid update"),
        "alpine:3.20"
    );
    assert!(
        !fixture.invocation_log.exists(),
        "engine should not run when uid update is disabled"
    );
}

#[test]
fn uid_update_details_fall_back_to_the_image_user() {
    let details = uid_update_details(
        &json!({}),
        Path::new("/tmp/example-workspace"),
        "ghcr.io/example/app:latest",
        "node",
        None,
        Some("linux/amd64"),
    )
    .expect("uid update details");

    assert_eq!(details.remote_user, "node");
    assert_eq!(details.image_user, "node");
    assert_eq!(details.platform.as_deref(), Some("linux/amd64"));
    assert!(details
        .updated_image_name
        .starts_with("vsc-example-workspace-"));
    assert!(details.updated_image_name.ends_with("-uid"));
}

#[test]
fn uid_update_details_preserve_the_image_user_when_remote_user_is_overridden() {
    let details = uid_update_details(
        &json!({
            "remoteUser": "vscode"
        }),
        Path::new("/tmp/example-workspace"),
        "ghcr.io/example/app:latest",
        "node",
        None,
        None,
    )
    .expect("uid update details");

    assert_eq!(details.remote_user, "vscode");
    assert_eq!(details.image_user, "node");
}

#[test]
fn uid_update_details_use_a_local_tag_for_digest_pinned_images() {
    let details = uid_update_details(
        &json!({
            "remoteUser": "vscode"
        }),
        Path::new("/tmp/example-workspace"),
        "ghcr.io/example/app@sha256:0123456789abcdef",
        "node",
        None,
        None,
    )
    .expect("uid update details");

    assert!(details
        .updated_image_name
        .starts_with("vsc-example-workspace-"));
    assert!(details.updated_image_name.ends_with("-uid"));
    assert!(!details.updated_image_name.contains('@'));
}

#[test]
fn prepare_up_image_pulls_missing_image_before_uid_update() {
    let fixture = FakeEngineFixture::new();
    fixture.write("image-inspect.exit", "1\n");
    fixture.write(
        "image-inspect.stderr",
        "Error: No such image: ghcr.io/example/app:latest\n",
    );
    fixture.write(
        "image-inspect-after-pull.stdout",
        &image_inspect_output("root", Some("linux/amd64")),
    );

    let workspace = fixture.root.join("workspace");
    fs::create_dir_all(&workspace).expect("workspace dir");
    let resolved = resolved_config(
        json!({
            "image": "ghcr.io/example/app:latest",
            "remoteUser": "vscode",
        }),
        &workspace,
    );

    let updated_image = prepare_up_image_for_platform(
        &resolved,
        &fixture.args(),
        "ghcr.io/example/app:latest",
        true,
    )
    .expect("prepare up image");

    assert_ne!(updated_image, "ghcr.io/example/app:latest");
    assert!(updated_image.ends_with("-uid"));
    let invocations = fixture.invocations();
    assert!(invocations.contains(&format!(
        "image inspect --format {} ghcr.io/example/app:latest",
        super::UID_UPDATE_IMAGE_INSPECT_FORMAT
    )));
    assert!(invocations.contains("pull ghcr.io/example/app:latest"));
    assert!(invocations.contains("build "));
    assert!(invocations.contains("--build-arg REMOTE_USER=vscode"));
    assert!(invocations.contains("--build-arg IMAGE_USER=root"));
}

#[test]
fn prepare_up_image_uses_run_args_user_for_uid_update_selection() {
    let fixture = FakeEngineFixture::new();
    fixture.write(
        "image-inspect.stdout",
        &image_inspect_output("root", Some("linux/amd64")),
    );

    let workspace = fixture.root.join("workspace");
    fs::create_dir_all(&workspace).expect("workspace dir");
    let resolved = resolved_config(
        json!({
            "runArgs": ["--user", "vscode"]
        }),
        &workspace,
    );

    let updated_image =
        prepare_up_image_for_platform(&resolved, &fixture.args(), "alpine:3.20", true)
            .expect("prepare up image");

    assert_ne!(updated_image, "alpine:3.20");
    assert!(updated_image.ends_with("-uid"));
    let invocations = fixture.invocations();
    assert!(invocations.contains("build "));
    assert!(invocations.contains("--build-arg REMOTE_USER=vscode"));
    assert!(invocations.contains("--build-arg IMAGE_USER=root"));
}

#[test]
fn prepare_up_image_preserves_the_inspected_platform_for_uid_update_builds() {
    let fixture = FakeEngineFixture::new();
    fixture.write(
        "image-inspect.stdout",
        &image_inspect_output("node", Some("linux/arm64/v8")),
    );

    let workspace = fixture.root.join("workspace");
    fs::create_dir_all(&workspace).expect("workspace dir");
    let resolved = resolved_config(
        json!({
            "remoteUser": "vscode"
        }),
        &workspace,
    );

    let updated_image = prepare_up_image_for_platform(
        &resolved,
        &fixture.args(),
        "ghcr.io/example/app:latest",
        true,
    )
    .expect("prepare up image");

    assert!(updated_image.ends_with("-uid"));
    let invocations = fixture.invocations();
    assert!(invocations.contains("build "));
    assert!(invocations.contains("--platform linux/arm64/v8"));
}

#[test]
fn prepare_up_image_prefixes_local_podman_base_images_with_localhost() {
    let fixture = FakeEngineFixture::new();
    fixture.write(
        "image-inspect.stdout",
        &image_inspect_output("node", Some("linux/amd64")),
    );

    let workspace = fixture.root.join("workspace");
    fs::create_dir_all(&workspace).expect("workspace dir");
    let resolved = resolved_config(
        json!({
            "remoteUser": "vscode"
        }),
        &workspace,
    );

    let local_image_name = uid_update_local_image_name(&workspace);
    let updated_image = prepare_up_image_for_platform(
        &resolved,
        &fixture.args_with_podman_name(),
        &local_image_name,
        true,
    )
    .expect("prepare up image");

    assert_eq!(updated_image, format!("{local_image_name}-uid"));
    let invocations = fixture.invocations();
    assert!(invocations.contains("build "));
    assert!(invocations.contains(&format!(
        "--build-arg BASE_IMAGE=localhost/{local_image_name}"
    )));
}

#[test]
fn prepare_up_image_retries_image_inspect_without_variant_template_for_podman() {
    let fixture = FakeEngineFixture::new();
    fixture.write("image-inspect-with-variant.exit", "1\n");
    fixture.write(
        "image-inspect-with-variant.stderr",
        "Error: template: inspect:2:30: executing \"inspect\" at <.Variant>: can't evaluate field Variant in type interface {}\n",
    );
    fixture.write(
        "image-inspect-without-variant.stdout",
        &image_inspect_output("node", Some("linux/amd64")),
    );

    let workspace = fixture.root.join("workspace");
    fs::create_dir_all(&workspace).expect("workspace dir");
    let resolved = resolved_config(
        json!({
            "remoteUser": "vscode"
        }),
        &workspace,
    );

    let updated_image = prepare_up_image_for_platform(
        &resolved,
        &fixture.args_with_podman_name(),
        "ghcr.io/example/app:latest",
        true,
    )
    .expect("prepare up image");

    assert!(updated_image.ends_with("-uid"));
    let invocations = fixture.invocations();
    assert!(invocations.contains(&format!(
        "image inspect --format {} ghcr.io/example/app:latest",
        super::UID_UPDATE_IMAGE_INSPECT_FORMAT
    )));
    assert!(invocations.contains(&format!(
        "image inspect --format {} ghcr.io/example/app:latest",
        super::UID_UPDATE_IMAGE_INSPECT_FORMAT_NO_VARIANT
    )));
}

#[test]
fn prepare_up_image_uses_compose_service_user_for_uid_update_selection() {
    let fixture = FakeEngineFixture::new();
    fixture.write(
        "image-inspect.stdout",
        &image_inspect_output("root", Some("linux/amd64")),
    );

    let workspace = fixture.root.join("workspace");
    let config_root = workspace.join(".devcontainer");
    fs::create_dir_all(&config_root).expect("config dir");
    fs::write(
        config_root.join("docker-compose.yml"),
        "services:\n  app:\n    image: ghcr.io/example/app:latest\n    user: vscode\n",
    )
    .expect("compose file");
    let resolved = ResolvedConfig {
        workspace_folder: workspace.clone(),
        config_file: config_root.join("devcontainer.json"),
        configuration: json!({
            "dockerComposeFile": "docker-compose.yml",
            "service": "app",
        }),
    };

    let updated_image = prepare_up_image_for_platform(
        &resolved,
        &fixture.args(),
        "ghcr.io/example/app:latest",
        true,
    )
    .expect("prepare up image");

    assert!(updated_image.ends_with("-uid"));
    let invocations = fixture.invocations();
    assert!(invocations.contains("build "));
    assert!(invocations.contains("--build-arg REMOTE_USER=vscode"));
}

#[test]
fn prepare_up_image_reports_compose_user_lookup_errors() {
    let fixture = FakeEngineFixture::new();
    let workspace = fixture.root.join("workspace");
    fs::create_dir_all(&workspace).expect("workspace dir");
    let resolved = ResolvedConfig {
        workspace_folder: workspace.clone(),
        config_file: workspace.join(".devcontainer").join("devcontainer.json"),
        configuration: json!({
            "dockerComposeFile": true,
            "service": "app",
            "remoteUser": "vscode"
        }),
    };

    let error = prepare_up_image_for_platform(
        &resolved,
        &fixture.args(),
        "ghcr.io/example/app:latest",
        true,
    )
    .expect_err("invalid compose file setting");

    assert!(error.contains("string or array"), "{error}");
}

#[test]
fn prepare_up_image_skips_non_updatable_users_and_missing_local_images() {
    let root_user_fixture = FakeEngineFixture::new();
    let workspace = root_user_fixture.root.join("workspace");
    fs::create_dir_all(&workspace).expect("workspace dir");
    let root_user_resolved = resolved_config(
        json!({
            "remoteUser": "root"
        }),
        &workspace,
    );

    assert_eq!(
        prepare_up_image_for_platform(
            &root_user_resolved,
            &root_user_fixture.args(),
            "alpine:3.20",
            true,
        )
        .expect("root user should skip uid update"),
        "alpine:3.20"
    );

    let missing_image_fixture = FakeEngineFixture::new();
    missing_image_fixture.write("image-inspect.exit", "1\n");
    missing_image_fixture.write("image-inspect.stderr", "Error: No such image: local-app\n");
    let workspace = missing_image_fixture.root.join("workspace");
    fs::create_dir_all(&workspace).expect("workspace dir");
    let missing_image_resolved = resolved_config(json!({}), &workspace);

    assert_eq!(
        prepare_up_image_for_platform(
            &missing_image_resolved,
            &missing_image_fixture.args(),
            "local-app",
            true,
        )
        .expect("missing local image should skip uid update"),
        "local-app"
    );
}

#[test]
fn uid_update_run_args_user_parses_supported_forms_and_edge_cases() {
    assert_eq!(
        uid_update_run_args_user(&json!({
            "runArgs": [42, "--user"]
        })),
        None
    );
    assert_eq!(
        uid_update_run_args_user(&json!({
            "runArgs": ["--user", "vscode"]
        })),
        Some("vscode".to_string())
    );
    assert_eq!(
        uid_update_run_args_user(&json!({
            "runArgs": ["-u=node"]
        })),
        Some("node".to_string())
    );
    assert_eq!(
        uid_update_run_args_user(&json!({
            "runArgs": ["--user=devcontainer"]
        })),
        Some("devcontainer".to_string())
    );
    assert_eq!(
        uid_update_run_args_user(&json!({
            "runArgs": ["--user", "first", "-u", "last"]
        })),
        Some("last".to_string())
    );
}

#[test]
fn parse_image_inspect_details_defaults_empty_user_and_platform() {
    let details = parse_image_inspect_details("\n\n").expect("inspect details");

    assert_eq!(details.expect("details").user, "root");
}

#[test]
fn prepare_up_image_reports_build_failures() {
    let fixture = FakeEngineFixture::new();
    fixture.write(
        "image-inspect.stdout",
        &image_inspect_output("root", Some("linux/amd64")),
    );
    fixture.write("build.exit", "1\n");
    fixture.write("build.stderr", "build failed\n");

    let workspace = fixture.root.join("workspace");
    fs::create_dir_all(&workspace).expect("workspace dir");
    let resolved = resolved_config(
        json!({
            "remoteUser": "vscode"
        }),
        &workspace,
    );

    let error = prepare_up_image_for_platform(&resolved, &fixture.args(), "alpine:3.20", true)
        .expect_err("build failure should propagate");

    assert_eq!(error, "build failed");
}

#[test]
fn prepare_up_image_reports_image_inspect_failures() {
    let fixture = FakeEngineFixture::new();
    fixture.write("image-inspect.exit", "1\n");
    fixture.write("image-inspect.stderr", "permission denied\n");

    let workspace = fixture.root.join("workspace");
    fs::create_dir_all(&workspace).expect("workspace dir");
    let resolved = resolved_config(
        json!({
            "remoteUser": "vscode"
        }),
        &workspace,
    );

    let error = prepare_up_image_for_platform(&resolved, &fixture.args(), "alpine:3.20", true)
        .expect_err("inspect failure should propagate");

    assert_eq!(error, "permission denied");
}

#[test]
fn inspect_image_details_reports_engine_spawn_failures() {
    let error = inspect_image_details_for_uid_update_once(
        &[
            "--docker-path".to_string(),
            "/path/that/does/not/exist".to_string(),
        ],
        "alpine:3.20",
    )
    .expect_err("spawn failure should propagate");

    assert!(error.contains("Container engine executable not found"));
}

#[test]
fn inspect_image_details_without_variant_reports_engine_spawn_failures() {
    let error = inspect_image_details_without_variant(
        &[
            "--docker-path".to_string(),
            "/path/that/does/not/exist".to_string(),
        ],
        "alpine:3.20",
    )
    .expect_err("spawn failure should propagate");

    assert!(error.contains("Container engine executable not found"));
}

#[test]
fn prepare_up_image_reports_variant_retry_failures() {
    let fixture = FakeEngineFixture::new();
    fixture.write("image-inspect-with-variant.exit", "1\n");
    fixture.write(
        "image-inspect-with-variant.stderr",
        "Error: can't evaluate field Variant in type interface {}\n",
    );
    fixture.write("image-inspect-without-variant.exit", "1\n");
    fixture.write("image-inspect-without-variant.stderr", "access denied\n");

    let workspace = fixture.root.join("workspace");
    fs::create_dir_all(&workspace).expect("workspace dir");
    let resolved = resolved_config(
        json!({
            "remoteUser": "vscode"
        }),
        &workspace,
    );

    let error = prepare_up_image_for_platform(
        &resolved,
        &fixture.args_with_podman_name(),
        "alpine:3.20",
        true,
    )
    .expect_err("variant retry failure should propagate");

    assert_eq!(error, "access denied");
}

#[test]
fn prepare_up_image_skips_when_variant_retry_reports_missing_local_image() {
    let fixture = FakeEngineFixture::new();
    fixture.write("image-inspect-with-variant.exit", "1\n");
    fixture.write(
        "image-inspect-with-variant.stderr",
        "Error: can't evaluate field Variant in type interface {}\n",
    );
    fixture.write("image-inspect-without-variant.exit", "1\n");
    fixture.write("image-inspect-without-variant.stderr", "image not known\n");

    let workspace = fixture.root.join("workspace");
    fs::create_dir_all(&workspace).expect("workspace dir");
    let resolved = resolved_config(json!({}), &workspace);

    assert_eq!(
        prepare_up_image_for_platform(
            &resolved,
            &fixture.args_with_podman_name(),
            "local-app",
            true,
        )
        .expect("missing image after variant retry should skip update"),
        "local-app"
    );
}

#[test]
fn prepare_up_image_reports_pull_failures() {
    let fixture = FakeEngineFixture::new();
    fixture.write("image-inspect.exit", "1\n");
    fixture.write(
        "image-inspect.stderr",
        "Error: No such image: ghcr.io/example/app:latest\n",
    );
    fixture.write("pull.exit", "1\n");
    fixture.write("pull.stderr", "pull failed\n");

    let workspace = fixture.root.join("workspace");
    fs::create_dir_all(&workspace).expect("workspace dir");
    let resolved = resolved_config(
        json!({
            "image": "ghcr.io/example/app:latest",
            "remoteUser": "vscode"
        }),
        &workspace,
    );

    let error = prepare_up_image_for_platform(
        &resolved,
        &fixture.args(),
        "ghcr.io/example/app:latest",
        true,
    )
    .expect_err("pull failure should propagate");

    assert_eq!(error, "pull failed");
}

#[test]
fn uid_update_base_image_preserves_registry_hostnames() {
    let fixture = FakeEngineFixture::new();

    assert_eq!(
        uid_update_base_image(&fixture.args_with_podman_name(), "localhost/app:latest"),
        "localhost/app:latest"
    );
    assert_eq!(
        uid_update_base_image(
            &fixture.args_with_podman_name(),
            "registry.example.com/app:latest"
        ),
        "registry.example.com/app:latest"
    );
    assert_eq!(
        uid_update_base_image(&fixture.args_with_podman_name(), "library/app:latest"),
        "localhost/library/app:latest"
    );
}

#[test]
fn uid_update_base_image_detects_podman_from_env_engine_path() {
    let fixture = FakeEngineFixture::new();
    let podman_path = fixture.podman_engine_path.display().to_string();
    let _env = test_env_defaults(&[(DEVCONTAINER_DOCKER_PATH, podman_path.as_str())]);

    assert_eq!(
        uid_update_base_image(&[], "library/app:latest"),
        "localhost/library/app:latest"
    );
}

#[test]
fn command_stdout_reports_stderr_on_failure() {
    let error = command_stdout("/bin/sh", &["-c", "printf 'boom' >&2; exit 3"])
        .expect_err("command failure should return stderr");

    assert_eq!(error, "boom");
}

#[test]
fn command_stdout_returns_trimmed_stdout_on_success() {
    assert_eq!(
        command_stdout("/bin/sh", &["-c", "printf '  ok  \n'"]).expect("stdout"),
        "ok"
    );
}

#[test]
fn command_stdout_reports_spawn_errors() {
    let error =
        command_stdout("/definitely/missing/id", &[]).expect_err("missing command should fail");

    assert!(!error.is_empty());
}

fn resolved_config(configuration: serde_json::Value, workspace_folder: &Path) -> ResolvedConfig {
    ResolvedConfig {
        workspace_folder: workspace_folder.to_path_buf(),
        config_file: workspace_folder
            .join(".devcontainer")
            .join("devcontainer.json"),
        configuration,
    }
}

struct FakeEngineFixture {
    root: PathBuf,
    engine_path: PathBuf,
    podman_engine_path: PathBuf,
    invocation_log: PathBuf,
}

impl FakeEngineFixture {
    fn new() -> Self {
        let root = unique_uid_update_build_context();
        fs::create_dir_all(&root).expect("fixture root");
        let engine_path = root.join("fake-engine");
        let podman_engine_path = root.join("podman");
        let invocation_log = root.join("invocations.log");
        let script = r#"#!/bin/sh
set -eu
PATH=/usr/bin:/bin:/usr/sbin:/sbin
export PATH

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
LOG="$ROOT/invocations.log"
printf '%s %s\n' "$1" "$*" >> "$LOG"

COMMAND="$1"
shift || true
case "$COMMAND" in
  image)
    SUBCOMMAND="${1:-}"
    shift || true
            case "$SUBCOMMAND" in
              inspect)
                if [ "${*}" != "${*#*'.Variant'*}" ]; then
                  if [ -f "$ROOT/image-inspect-with-variant.stdout" ]; then
                    cat "$ROOT/image-inspect-with-variant.stdout"
                  fi
                  if [ -f "$ROOT/image-inspect-with-variant.stderr" ]; then
                    cat "$ROOT/image-inspect-with-variant.stderr" >&2
                  fi
                  if [ -f "$ROOT/image-inspect-with-variant.exit" ]; then
                    exit "$(tr -d '\n' < "$ROOT/image-inspect-with-variant.exit")"
                  fi
                else
                  if [ -f "$ROOT/image-inspect-without-variant.stdout" ]; then
                    cat "$ROOT/image-inspect-without-variant.stdout"
                  fi
                  if [ -f "$ROOT/image-inspect-without-variant.stderr" ]; then
                    cat "$ROOT/image-inspect-without-variant.stderr" >&2
                  fi
                  if [ -f "$ROOT/image-inspect-without-variant.exit" ]; then
                    exit "$(tr -d '\n' < "$ROOT/image-inspect-without-variant.exit")"
                  fi
                fi
                if [ -f "$ROOT/pulled" ]; then
                  if [ -f "$ROOT/image-inspect-after-pull.stdout" ]; then
                    cat "$ROOT/image-inspect-after-pull.stdout"
                  fi
                  if [ -f "$ROOT/image-inspect-after-pull.stderr" ]; then
                    cat "$ROOT/image-inspect-after-pull.stderr" >&2
                  fi
                  if [ -f "$ROOT/image-inspect-after-pull.exit" ]; then
                    exit "$(tr -d '\n' < "$ROOT/image-inspect-after-pull.exit")"
                  fi
                  exit 0
                fi
                if [ -f "$ROOT/image-inspect.stdout" ]; then
                  cat "$ROOT/image-inspect.stdout"
                fi
                if [ -f "$ROOT/image-inspect.stderr" ]; then
                  cat "$ROOT/image-inspect.stderr" >&2
        fi
        if [ -f "$ROOT/image-inspect.exit" ]; then
          exit "$(tr -d '\n' < "$ROOT/image-inspect.exit")"
        fi
        exit 0
        ;;
    esac
    ;;
  pull)
    touch "$ROOT/pulled"
    if [ -f "$ROOT/pull.stdout" ]; then
      cat "$ROOT/pull.stdout"
    fi
    if [ -f "$ROOT/pull.stderr" ]; then
      cat "$ROOT/pull.stderr" >&2
    fi
    if [ -f "$ROOT/pull.exit" ]; then
      exit "$(tr -d '\n' < "$ROOT/pull.exit")"
    fi
    exit 0
    ;;
  build)
    if [ -f "$ROOT/build.stdout" ]; then
      cat "$ROOT/build.stdout"
    fi
    if [ -f "$ROOT/build.stderr" ]; then
      cat "$ROOT/build.stderr" >&2
    fi
    if [ -f "$ROOT/build.exit" ]; then
      exit "$(tr -d '\n' < "$ROOT/build.exit")"
    fi
    exit 0
    ;;
esac

echo "unsupported fake engine command: $COMMAND $*" >&2
exit 1
"#;
        fs::write(&engine_path, script).expect("engine script");
        fs::write(&podman_engine_path, script).expect("podman engine script");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let mut permissions = fs::metadata(&engine_path)
                .expect("engine metadata")
                .permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&engine_path, permissions.clone()).expect("engine permissions");
            fs::set_permissions(&podman_engine_path, permissions)
                .expect("podman engine permissions");
        }

        Self {
            root,
            engine_path,
            podman_engine_path,
            invocation_log,
        }
    }

    fn write(&self, name: &str, contents: &str) {
        fs::write(self.root.join(name), contents).expect("fixture file");
    }

    fn args(&self) -> Vec<String> {
        vec![
            "--docker-path".to_string(),
            self.engine_path.display().to_string(),
        ]
    }

    fn args_with_podman_name(&self) -> Vec<String> {
        vec![
            "--docker-path".to_string(),
            self.podman_engine_path.display().to_string(),
        ]
    }

    fn invocations(&self) -> String {
        fs::read_to_string(&self.invocation_log).expect("invocations")
    }
}

impl Drop for FakeEngineFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn image_inspect_output(user: &str, platform: Option<&str>) -> String {
    format!("{user}\n{}\n", platform.unwrap_or_default())
}
