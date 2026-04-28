//! Native runtime exec argument assembly helpers.

use std::io::IsTerminal;

use serde_json::Value;

use super::context::{combined_remote_env, configured_user};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ExecStdio {
    stdin_is_terminal: bool,
    stdout_is_terminal: bool,
}

impl ExecStdio {
    pub(crate) fn current() -> Self {
        Self {
            stdin_is_terminal: std::io::stdin().is_terminal(),
            stdout_is_terminal: std::io::stdout().is_terminal(),
        }
    }

    fn should_allocate_tty(self) -> bool {
        self.stdin_is_terminal && self.stdout_is_terminal
    }
}

pub(crate) fn exec_command_and_args(args: &[String]) -> Result<Vec<String>, String> {
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if matches!(
            arg.as_str(),
            "--docker-path"
                | "--docker-compose-path"
                | "--workspace-folder"
                | "--config"
                | "--override-config"
                | "--workspace-mount-consistency"
                | "--remote-env"
                | "--secrets-file"
                | "--container-id"
                | "--id-label"
                | "--log-level"
                | "--user-data-folder"
                | "--container-data-folder"
                | "--container-system-data-folder"
                | "--container-session-data-folder"
                | "--default-user-env-probe"
                | "--terminal-columns"
                | "--terminal-rows"
        ) {
            index += 2;
            continue;
        }
        if matches!(
            arg.as_str(),
            "--mount-workspace-git-root"
                | "--mount-git-worktree-common-dir"
                | "--skip-feature-auto-mapping"
        ) {
            index += if args
                .get(index + 1)
                .is_some_and(|next| is_explicit_bool_literal(next))
            {
                2
            } else {
                1
            };
            continue;
        }
        if arg.starts_with("--") {
            return Err(format!("Unsupported exec option: {arg}"));
        }
        break;
    }

    if index >= args.len() {
        return Err("exec requires a command to run".to_string());
    }

    Ok(args[index..].to_vec())
}

fn is_explicit_bool_literal(value: &str) -> bool {
    matches!(
        value,
        "false" | "0" | "no" | "off" | "true" | "1" | "yes" | "on"
    )
}

pub(crate) fn exec_engine_args(
    args: &[String],
    configuration: &Value,
    remote_workspace_folder: &str,
    container_id: &str,
    command_args: Vec<String>,
    stdio: ExecStdio,
) -> Result<Vec<String>, String> {
    let mut engine_args = vec!["exec".to_string()];
    engine_args.push("-i".to_string());
    if stdio.should_allocate_tty() {
        engine_args.push("-t".to_string());
    }
    engine_args.push("--workdir".to_string());
    engine_args.push(remote_workspace_folder.to_string());
    if let Some(user) = configured_user(configuration) {
        engine_args.push("--user".to_string());
        engine_args.push(user.to_string());
    }
    for (key, value) in combined_remote_env(args, Some(configuration))? {
        engine_args.push("-e".to_string());
        engine_args.push(format!("{key}={value}"));
    }
    engine_args.push(container_id.to_string());
    engine_args.extend(command_args);
    Ok(engine_args)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{exec_command_and_args, exec_engine_args, ExecStdio};

    #[test]
    fn exec_command_and_args_rejects_unknown_options() {
        let error = exec_command_and_args(&[
            "--workspace-folder".to_string(),
            "/tmp/workspace".to_string(),
            "--mystery".to_string(),
        ])
        .expect_err("expected unsupported option");

        assert_eq!(error, "Unsupported exec option: --mystery");
    }

    #[test]
    fn exec_command_and_args_rejects_interactive_option() {
        let error = exec_command_and_args(&[
            "--interactive".to_string(),
            "/bin/echo".to_string(),
            "hello".to_string(),
        ])
        .expect_err("expected unsupported option");

        assert_eq!(error, "Unsupported exec option: --interactive");
    }

    #[test]
    fn exec_engine_args_include_workdir_user_and_remote_env() {
        let args = exec_engine_args(
            &["--remote-env".to_string(), "CLI_ENV=cli".to_string()],
            &json!({
                "remoteUser": "vscode",
                "remoteEnv": {
                    "CONFIG_ENV": "config"
                }
            }),
            "/workspace",
            "container-id",
            vec!["/bin/echo".to_string(), "hello".to_string()],
            ExecStdio {
                stdin_is_terminal: false,
                stdout_is_terminal: false,
            },
        )
        .expect("engine args");

        assert_eq!(args[0], "exec");
        assert_eq!(args[1], "-i");
        assert!(!args.contains(&"-t".to_string()));
        assert!(args.contains(&"--workdir".to_string()));
        assert!(args.contains(&"/workspace".to_string()));
        assert!(args.contains(&"--user".to_string()));
        assert!(args.contains(&"vscode".to_string()));
        assert!(args.contains(&"container-id".to_string()));
        assert!(args.contains(&"/bin/echo".to_string()));
        assert!(args.iter().any(|arg| arg == "CONFIG_ENV=config"));
        assert!(args.iter().any(|arg| arg == "CLI_ENV=cli"));
    }

    #[test]
    fn exec_engine_args_attach_stdin_by_default() {
        let args = minimal_exec_engine_args(ExecStdio {
            stdin_is_terminal: false,
            stdout_is_terminal: false,
        });

        assert_eq!(args[0], "exec");
        assert_eq!(args[1], "-i");
    }

    #[test]
    fn exec_engine_args_allocate_tty_only_when_stdin_and_stdout_are_terminals() {
        let cases = [
            (
                ExecStdio {
                    stdin_is_terminal: false,
                    stdout_is_terminal: false,
                },
                false,
            ),
            (
                ExecStdio {
                    stdin_is_terminal: true,
                    stdout_is_terminal: false,
                },
                false,
            ),
            (
                ExecStdio {
                    stdin_is_terminal: false,
                    stdout_is_terminal: true,
                },
                false,
            ),
            (
                ExecStdio {
                    stdin_is_terminal: true,
                    stdout_is_terminal: true,
                },
                true,
            ),
        ];

        for (stdio, expect_tty) in cases {
            let args = minimal_exec_engine_args(stdio);

            assert_eq!(args.contains(&"-t".to_string()), expect_tty);
        }
    }

    fn minimal_exec_engine_args(stdio: ExecStdio) -> Vec<String> {
        exec_engine_args(
            &[],
            &json!({}),
            "/workspace",
            "container-id",
            vec!["/bin/echo".to_string(), "hello".to_string()],
            stdio,
        )
        .expect("engine args")
    }

    #[test]
    fn exec_command_and_args_accepts_docker_compose_path() {
        let args = exec_command_and_args(&[
            "--docker-compose-path".to_string(),
            "/usr/local/bin/podman-compose".to_string(),
            "/bin/echo".to_string(),
            "hello".to_string(),
        ])
        .expect("command args");

        assert_eq!(args, vec!["/bin/echo".to_string(), "hello".to_string()]);
    }

    #[test]
    fn exec_command_and_args_accept_workspace_mount_flags() {
        let args = exec_command_and_args(&[
            "--workspace-folder".to_string(),
            "/workspace/packages/app".to_string(),
            "--mount-workspace-git-root".to_string(),
            "false".to_string(),
            "--workspace-mount-consistency".to_string(),
            "delegated".to_string(),
            "/bin/echo".to_string(),
            "hello".to_string(),
        ])
        .expect("command args");

        assert_eq!(args, vec!["/bin/echo".to_string(), "hello".to_string()]);
    }

    #[test]
    fn exec_command_and_args_does_not_consume_command_after_bare_mount_flag() {
        let args = exec_command_and_args(&[
            "--mount-workspace-git-root".to_string(),
            "/bin/bash".to_string(),
        ])
        .expect("command args");

        assert_eq!(args, vec!["/bin/bash".to_string()]);
    }

    #[test]
    fn exec_command_and_args_accepts_explicit_bool_for_git_worktree_flag() {
        let args = exec_command_and_args(&[
            "--mount-git-worktree-common-dir".to_string(),
            "true".to_string(),
            "/bin/echo".to_string(),
            "hello".to_string(),
        ])
        .expect("command args");

        assert_eq!(args, vec!["/bin/echo".to_string(), "hello".to_string()]);
    }

    #[test]
    fn exec_command_and_args_accept_shared_runtime_options() {
        let args = exec_command_and_args(&[
            "--log-level".to_string(),
            "debug".to_string(),
            "--user-data-folder".to_string(),
            "/tmp/devcontainer".to_string(),
            "--container-data-folder".to_string(),
            "/tmp/container".to_string(),
            "--container-system-data-folder".to_string(),
            "/var/devcontainer".to_string(),
            "--container-session-data-folder".to_string(),
            "/tmp/session".to_string(),
            "--default-user-env-probe".to_string(),
            "loginShell".to_string(),
            "--skip-feature-auto-mapping".to_string(),
            "--terminal-columns".to_string(),
            "120".to_string(),
            "--terminal-rows".to_string(),
            "40".to_string(),
            "/bin/echo".to_string(),
            "hello".to_string(),
        ])
        .expect("command args");

        assert_eq!(args, vec!["/bin/echo".to_string(), "hello".to_string()]);
    }
}
