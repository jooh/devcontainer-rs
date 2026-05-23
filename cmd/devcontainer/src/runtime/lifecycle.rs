//! Lifecycle command selection and execution for native runtime flows.

mod dotfiles;
mod requests;
mod selection;

use std::path::Path;
use std::thread;

use serde_json::Value;

use crate::process_runner::{self, ProcessRequest, ProcessResult};

use requests::{host_lifecycle_request, lifecycle_exec_args};
use selection::{lifecycle_command_value, selected_lifecycle_steps};

use super::{engine, user_resolution};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LifecycleMode {
    UpCreated,
    UpStarted,
    UpReused,
    SetUp,
    RunUserCommands,
}

enum LifecycleCommand {
    Shell(String),
    Exec(Vec<String>),
}

enum LifecycleStep {
    CommandGroup(Vec<LifecycleCommand>),
    InstallDotfiles,
}

pub(crate) fn run_lifecycle_commands(
    container_id: &str,
    args: &[String],
    configuration: &Value,
    remote_workspace_folder: &str,
    mode: LifecycleMode,
) -> Result<(), String> {
    let steps = selected_lifecycle_steps(configuration, args, mode);
    if steps.is_empty() {
        return Ok(());
    }
    let remote_env =
        user_resolution::combined_remote_env_with_home(args, configuration, container_id)?;

    for step in steps {
        match step {
            LifecycleStep::CommandGroup(command_group) => {
                run_process_group(command_group, |command| {
                    let engine_args = lifecycle_exec_args(
                        configuration,
                        &remote_env,
                        remote_workspace_folder,
                        container_id,
                        command,
                    );
                    Ok(engine::engine_request(args, engine_args))
                })?;
            }
            LifecycleStep::InstallDotfiles => {
                let command = dotfiles::dotfiles_install_command(args)
                    .expect("dotfiles install step requires dotfiles options");
                run_process_group(vec![LifecycleCommand::Shell(command)], |command| {
                    let engine_args = lifecycle_exec_args(
                        configuration,
                        &remote_env,
                        remote_workspace_folder,
                        container_id,
                        command,
                    );
                    Ok(engine::engine_request(args, engine_args))
                })?;
            }
        }
    }

    Ok(())
}

pub(crate) fn run_initialize_command(
    args: &[String],
    configuration: &Value,
    workspace_folder: &Path,
) -> Result<(), String> {
    let Some(command_group) = lifecycle_command_value(configuration, "initializeCommand") else {
        return Ok(());
    };

    run_process_group(command_group, |command| {
        Ok(host_lifecycle_request(args, workspace_folder, command))
    })
}

fn run_process_group(
    command_group: Vec<LifecycleCommand>,
    build_request: impl Fn(LifecycleCommand) -> Result<ProcessRequest, String>,
) -> Result<(), String> {
    if command_group.len() == 1 {
        let request = build_request(
            command_group
                .into_iter()
                .next()
                .expect("single lifecycle command"),
        )?;
        let result = run_lifecycle_process(request)?;
        if result.status_code != 0 {
            return Err(engine::stderr_or_stdout(&result));
        }
        return Ok(());
    }

    let mut handles = Vec::with_capacity(command_group.len());
    for command in command_group {
        let request = build_request(command);
        handles.push(thread::spawn(move || match request {
            Ok(request) => run_lifecycle_process(request),
            Err(error) => Err(error),
        }));
    }

    let mut first_error = None;
    for handle in handles {
        let result = match handle.join() {
            Ok(result) => result,
            Err(_) => return Err("Lifecycle command thread panicked unexpectedly".to_string()),
        };
        match result {
            Ok(result) if result.status_code == 0 => {}
            Ok(result) => {
                if first_error.is_none() {
                    first_error = Some(engine::stderr_or_stdout(&result));
                }
            }
            Err(error) => {
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
        }
    }

    if let Some(error) = first_error {
        return Err(error);
    }

    Ok(())
}

fn run_lifecycle_process(request: ProcessRequest) -> Result<ProcessResult, String> {
    #[cfg(test)]
    if request.program == "__devcontainer_lifecycle_test_panic__" {
        panic!("synthetic lifecycle process panic");
    }

    match process_runner::run_process(&request) {
        Ok(result) => Ok(result),
        Err(error) => Err(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    //! Unit tests for lifecycle helper behavior.

    use std::collections::HashMap;
    use std::fs;
    use std::path::Path;

    use serde_json::json;

    use crate::process_runner::{ProcessLogLevel, ProcessRequest};
    use crate::test_support::{unique_temp_dir, write_executable_script};

    use super::{
        dotfiles::dotfiles_install_command,
        requests::{host_lifecycle_request, lifecycle_exec_args},
        run_initialize_command, run_lifecycle_commands, run_process_group,
        selection::{lifecycle_command_group, selected_lifecycle_steps},
        LifecycleCommand, LifecycleMode, LifecycleStep,
    };

    fn shell_request(script: &str) -> ProcessRequest {
        ProcessRequest {
            program: "sh".to_string(),
            args: vec!["-c".to_string(), script.to_string()],
            cwd: None,
            env: HashMap::new(),
            log_level: ProcessLogLevel::Info,
        }
    }

    #[test]
    fn lifecycle_command_group_supports_strings_arrays_and_objects() {
        assert!(lifecycle_command_group(&json!("echo hello")).is_some());
        assert!(lifecycle_command_group(&json!(["/bin/echo", "hello"])).is_some());
        assert!(lifecycle_command_group(&json!({
            "a": "echo one",
            "b": ["/bin/echo", "two"]
        }))
        .is_some());
        assert!(lifecycle_command_group(&json!({
            "ignored": true
        }))
        .is_none());
        assert!(lifecycle_command_group(&json!(true)).is_none());
        assert!(lifecycle_command_group(&json!([])).is_none());
        assert!(lifecycle_command_group(&json!([true])).is_none());
        assert!(lifecycle_command_group(&json!({
            "ignored": {
                "nested": "not a command"
            }
        }))
        .is_none());
    }

    #[test]
    fn selected_lifecycle_steps_respect_mode_and_wait_for() {
        let initialize_wait = selected_lifecycle_steps(
            &json!({
                "waitFor": "initializeCommand",
                "postCreateCommand": "echo post-create"
            }),
            &["--skip-non-blocking-commands".to_string()],
            LifecycleMode::RunUserCommands,
        );

        assert!(initialize_wait.is_empty());

        let steps = selected_lifecycle_steps(
            &json!({
                "onCreateCommand": "echo on-create",
                "updateContentCommand": "echo update",
                "postCreateCommand": "echo post-create",
                "postStartCommand": "echo post-start",
                "postAttachCommand": "echo post-attach",
                "waitFor": "postStartCommand"
            }),
            &["--skip-non-blocking-commands".to_string()],
            LifecycleMode::RunUserCommands,
        );

        assert_eq!(steps.len(), 4);

        let reused = selected_lifecycle_steps(
            &json!({
                "postStartCommand": "echo post-start",
                "postAttachCommand": "echo post-attach"
            }),
            &[],
            LifecycleMode::UpReused,
        );

        assert_eq!(reused.len(), 1);
    }

    #[test]
    fn selected_lifecycle_steps_insert_dotfiles_after_post_create() {
        let steps = selected_lifecycle_steps(
            &json!({
                "postCreateCommand": "echo post-create",
                "postStartCommand": "echo post-start"
            }),
            &[
                "--dotfiles-repository".to_string(),
                "./dotfiles".to_string(),
            ],
            LifecycleMode::RunUserCommands,
        );

        assert!(matches!(steps[0], LifecycleStep::CommandGroup(_)));
        assert!(matches!(steps[1], LifecycleStep::InstallDotfiles));
        assert!(matches!(steps[2], LifecycleStep::CommandGroup(_)));
    }

    #[test]
    fn selected_lifecycle_steps_stop_for_personalization_after_dotfiles() {
        let steps = selected_lifecycle_steps(
            &json!({
                "postCreateCommand": "echo post-create",
                "postStartCommand": "echo post-start",
                "postAttachCommand": "echo post-attach"
            }),
            &[
                "--stop-for-personalization".to_string(),
                "--dotfiles-repository".to_string(),
                "./dotfiles".to_string(),
            ],
            LifecycleMode::RunUserCommands,
        );

        assert_eq!(steps.len(), 2);
        assert!(matches!(steps[0], LifecycleStep::CommandGroup(_)));
        assert!(matches!(steps[1], LifecycleStep::InstallDotfiles));
    }

    #[test]
    fn lifecycle_exec_args_use_absolute_shell_path() {
        let args = lifecycle_exec_args(
            &json!({}),
            &HashMap::new(),
            "/workspaces/sample",
            "container-id",
            LifecycleCommand::Shell("echo hello".to_string()),
        );

        assert!(
            args.contains(&"/bin/sh".to_string()),
            "expected lifecycle shell command to use /bin/sh: {args:?}"
        );
    }

    #[test]
    fn host_lifecycle_request_supports_exec_commands() {
        let request = host_lifecycle_request(
            &[
                "--log-level".to_string(),
                "trace".to_string(),
                "--terminal-columns".to_string(),
                "120".to_string(),
                "--terminal-rows".to_string(),
                "40".to_string(),
            ],
            std::path::Path::new("/workspace"),
            LifecycleCommand::Exec(vec![
                "echo".to_string(),
                "hello".to_string(),
                "world".to_string(),
            ]),
        );

        assert_eq!(request.program, "echo");
        assert_eq!(request.args, vec!["hello".to_string(), "world".to_string()]);
        assert_eq!(
            request.cwd.as_deref(),
            Some(std::path::Path::new("/workspace"))
        );
        assert_eq!(request.log_level, ProcessLogLevel::Trace);
        assert_eq!(request.env.get("COLUMNS").map(String::as_str), Some("120"));
        assert_eq!(request.env.get("LINES").map(String::as_str), Some("40"));
    }

    #[test]
    fn run_initialize_command_executes_host_shell_commands_in_workspace() {
        let root = unique_temp_dir("devcontainer-lifecycle-test");
        fs::create_dir_all(&root).expect("workspace");

        run_initialize_command(
            &[],
            &json!({
                "initializeCommand": "printf initialized > initialize-marker"
            }),
            &root,
        )
        .expect("initialize command");

        assert_eq!(
            fs::read_to_string(root.join("initialize-marker")).expect("marker"),
            "initialized"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn run_lifecycle_commands_executes_container_steps_and_dotfiles() {
        let root = unique_temp_dir("devcontainer-lifecycle-test");
        fs::create_dir_all(&root).expect("temp root");
        let log = root.join("engine.log");
        let engine = root.join("engine");
        write_lifecycle_engine(&engine, &log, 0);
        let args = vec![
            "--docker-path".to_string(),
            engine.display().to_string(),
            "--dotfiles-repository".to_string(),
            "./dotfiles".to_string(),
        ];

        run_lifecycle_commands(
            "container-id",
            &args,
            &json!({
                "remoteUser": "vscode",
                "remoteEnv": {
                    "HOME": "/home/vscode",
                    "FROM_CONFIG": "yes"
                },
                "postCreateCommand": ["/bin/echo", "created"],
                "postStartCommand": "echo started"
            }),
            "/workspace",
            LifecycleMode::RunUserCommands,
        )
        .expect("lifecycle commands");

        let invocations = fs::read_to_string(&log).expect("engine log");
        assert!(invocations.contains("exec --workdir /workspace --user vscode"));
        assert!(invocations.contains("FROM_CONFIG=yes"));
        assert!(invocations.contains("HOME=/home/vscode"));
        assert!(invocations.contains("container-id /bin/echo created"));
        assert!(invocations.contains("git clone --depth 1 './dotfiles'"));
        assert!(invocations.contains("container-id /bin/sh -lc echo started"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn run_lifecycle_commands_reports_home_resolution_errors_before_steps() {
        let error = run_lifecycle_commands(
            "container-id",
            &[
                "--docker-path".to_string(),
                "/definitely/missing/container-engine".to_string(),
            ],
            &json!({
                "postAttachCommand": "echo attach"
            }),
            "/workspace",
            LifecycleMode::UpReused,
        )
        .expect_err("home resolution failure");

        assert!(error.contains("Container engine executable not found"));
    }

    #[test]
    fn run_lifecycle_commands_reports_remote_env_errors_before_steps() {
        let error = run_lifecycle_commands(
            "container-id",
            &[
                "--secrets-file".to_string(),
                "/definitely/missing/secrets.json".to_string(),
            ],
            &json!({
                "remoteEnv": {
                    "HOME": "/home/vscode"
                },
                "postAttachCommand": "echo attach"
            }),
            "/workspace",
            LifecycleMode::UpReused,
        )
        .expect_err("remote env failure");

        assert!(error.contains("No such file") || error.contains("not found"));
    }

    #[test]
    fn run_lifecycle_commands_reports_container_command_failures() {
        let root = unique_temp_dir("devcontainer-lifecycle-test");
        fs::create_dir_all(&root).expect("temp root");
        let log = root.join("engine.log");
        let engine = root.join("engine");
        write_lifecycle_engine(&engine, &log, 12);
        let args = vec!["--docker-path".to_string(), engine.display().to_string()];

        let error = run_lifecycle_commands(
            "container-id",
            &args,
            &json!({
                "remoteEnv": {
                    "HOME": "/home/vscode"
                },
                "postAttachCommand": "echo attach"
            }),
            "/workspace",
            LifecycleMode::UpReused,
        )
        .expect_err("lifecycle failure");

        assert_eq!(error, "lifecycle failed");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn run_lifecycle_commands_reports_dotfiles_command_failures() {
        let root = unique_temp_dir("devcontainer-lifecycle-test");
        fs::create_dir_all(&root).expect("temp root");
        let log = root.join("engine.log");
        let engine = root.join("engine");
        write_lifecycle_engine(&engine, &log, 12);
        let args = vec![
            "--docker-path".to_string(),
            engine.display().to_string(),
            "--dotfiles-repository".to_string(),
            "./dotfiles".to_string(),
        ];

        let error = run_lifecycle_commands(
            "container-id",
            &args,
            &json!({
                "remoteEnv": {
                    "HOME": "/home/vscode"
                }
            }),
            "/workspace",
            LifecycleMode::RunUserCommands,
        )
        .expect_err("dotfiles lifecycle failure");

        assert_eq!(error, "lifecycle failed");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn dotfiles_install_command_defaults_target_path_and_marker_folder() {
        let command = dotfiles_install_command(&[
            "--dotfiles-repository".to_string(),
            "owner/repo".to_string(),
        ])
        .expect("dotfiles command");

        assert!(command.contains("https://github.com/owner/repo.git"));
        assert!(command.contains("~/.devcontainer/.dotfilesMarker"));
        assert!(command.contains("~/dotfiles"));
    }

    #[test]
    fn dotfiles_install_command_supports_explicit_install_command_and_paths() {
        let command = dotfiles_install_command(&[
            "--dotfiles-repository".to_string(),
            "git@github.com:owner/repo.git".to_string(),
            "--dotfiles-install-command".to_string(),
            "setup.sh".to_string(),
            "--dotfiles-target-path".to_string(),
            "/home/dev/dot files".to_string(),
            "--container-data-folder".to_string(),
            "/tmp/devcontainer-data/".to_string(),
        ])
        .expect("dotfiles command");

        assert!(command.contains("'git@github.com:owner/repo.git'"));
        assert!(command.contains("'/tmp/devcontainer-data/.dotfilesMarker'"));
        assert!(command.contains("'/home/dev/dot files'"));
        assert!(command.contains("install_path='./setup.sh'"));
        assert!(command.contains("Could not locate 'setup.sh'"));
    }

    #[test]
    fn run_process_group_reports_single_and_parallel_errors() {
        run_process_group(
            vec![
                LifecycleCommand::Shell("first".to_string()),
                LifecycleCommand::Shell("second".to_string()),
            ],
            |_| Ok(shell_request("exit 0")),
        )
        .expect("parallel commands should succeed");

        let single_error =
            run_process_group(vec![LifecycleCommand::Shell("single".to_string())], |_| {
                Ok(shell_request("echo single-failed >&2; exit 7"))
            })
            .expect_err("single command failure");
        assert_eq!(single_error, "single-failed");

        let parallel_error = run_process_group(
            vec![
                LifecycleCommand::Shell("ok".to_string()),
                LifecycleCommand::Shell("fail".to_string()),
            ],
            |command| {
                let script = match command {
                    LifecycleCommand::Shell(text) if text == "fail" => {
                        "echo parallel-failed >&2; exit 9"
                    }
                    _ => "exit 0",
                };
                Ok(shell_request(script))
            },
        )
        .expect_err("parallel command failure");
        assert_eq!(parallel_error, "parallel-failed");

        let build_error = run_process_group(
            vec![
                LifecycleCommand::Shell("ok".to_string()),
                LifecycleCommand::Shell("bad-request".to_string()),
            ],
            |command| match command {
                LifecycleCommand::Shell(text) if text == "bad-request" => {
                    Err("request failed".to_string())
                }
                _ => Ok(shell_request("exit 0")),
            },
        )
        .expect_err("request build failure");
        assert_eq!(build_error, "request failed");

        let second_error_ignored = run_process_group(
            vec![
                LifecycleCommand::Shell("first".to_string()),
                LifecycleCommand::Shell("second".to_string()),
            ],
            |_| Ok(shell_request("echo failed >&2; exit 9")),
        )
        .expect_err("parallel failures");
        assert_eq!(second_error_ignored, "failed");

        let parallel_spawn_error = run_process_group(
            vec![
                LifecycleCommand::Shell("first".to_string()),
                LifecycleCommand::Shell("second".to_string()),
            ],
            |_| {
                Ok(ProcessRequest {
                    program: "/definitely/missing/lifecycle-command".to_string(),
                    args: Vec::new(),
                    cwd: None,
                    env: HashMap::new(),
                    log_level: ProcessLogLevel::Info,
                })
            },
        )
        .expect_err("parallel spawn failures");
        assert_spawn_error(&parallel_spawn_error);

        let parallel_panic_error = run_process_group(
            vec![
                LifecycleCommand::Shell("first".to_string()),
                LifecycleCommand::Shell("second".to_string()),
            ],
            |command| {
                let program = match command {
                    LifecycleCommand::Shell(text) if text == "second" => {
                        "__devcontainer_lifecycle_test_panic__"
                    }
                    _ => "/bin/true",
                };
                Ok(ProcessRequest {
                    program: program.to_string(),
                    args: Vec::new(),
                    cwd: None,
                    env: HashMap::new(),
                    log_level: ProcessLogLevel::Info,
                })
            },
        )
        .expect_err("parallel process panic");
        assert_eq!(
            parallel_panic_error,
            "Lifecycle command thread panicked unexpectedly"
        );
    }

    #[test]
    fn run_process_group_reports_single_request_build_and_spawn_errors() {
        let build_error =
            run_process_group(vec![LifecycleCommand::Shell("bad".to_string())], |_| {
                Err("single request failed".to_string())
            })
            .expect_err("single request build failure");
        assert_eq!(build_error, "single request failed");

        let spawn_error =
            run_process_group(vec![LifecycleCommand::Shell("missing".to_string())], |_| {
                Ok(ProcessRequest {
                    program: "/definitely/missing/lifecycle-command".to_string(),
                    args: Vec::new(),
                    cwd: None,
                    env: HashMap::new(),
                    log_level: ProcessLogLevel::Info,
                })
            })
            .expect_err("spawn failure");
        assert_spawn_error(&spawn_error);
    }

    fn write_lifecycle_engine(engine: &Path, log: &Path, lifecycle_exit: i32) {
        let log = log.display();
        write_executable_script(
            engine,
            &format!(
                "#!/bin/sh\ncase \"$1\" in\n  inspect) printf '[{{\"Config\":{{\"Env\":[\"HOME=/home/vscode\"],\"User\":\"vscode\"}}}}]\\n' ;;\n  exec)\n    case \"$*\" in\n      *getent\\ passwd*) printf 'vscode:x:1000:1000::/home/vscode:/bin/sh\\n' ;;\n      *) printf '%s\\n' \"$*\" >> '{log}'; if [ {lifecycle_exit} -ne 0 ]; then printf 'lifecycle failed\\n' >&2; exit {lifecycle_exit}; fi ;;\n    esac\n    ;;\n  *) printf 'unexpected engine command: %s\\n' \"$*\" >&2; exit 2 ;;\nesac\n"
            ),
        );
    }

    fn assert_spawn_error(error: &str) {
        let recognized = ["No such file", "os error", "cannot find"]
            .iter()
            .any(|message| error.contains(message));
        assert!(recognized, "{error}");
    }
}
