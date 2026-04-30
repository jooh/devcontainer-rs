//! Lifecycle command selection and execution for native runtime flows.

mod dotfiles;
mod requests;
mod selection;

use std::path::Path;
use std::thread;

use serde_json::Value;

use crate::process_runner::{self, ProcessRequest};

use requests::{host_lifecycle_request, lifecycle_exec_args};
use selection::{lifecycle_command_value, selected_lifecycle_steps};

use super::{engine, user_resolution};

#[derive(Clone, Copy)]
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
                let Some(command) = dotfiles::dotfiles_install_command(args) else {
                    continue;
                };
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
        let result = process_runner::run_process(&build_request(
            command_group
                .into_iter()
                .next()
                .expect("single lifecycle command"),
        )?)
        .map_err(|error| error.to_string())?;
        if result.status_code != 0 {
            return Err(engine::stderr_or_stdout(&result));
        }
        return Ok(());
    }

    let handles = command_group
        .into_iter()
        .map(|command| {
            let request = build_request(command);
            thread::spawn(move || match request {
                Ok(request) => {
                    process_runner::run_process(&request).map_err(|error| error.to_string())
                }
                Err(error) => Err(error),
            })
        })
        .collect::<Vec<_>>();

    let mut first_error = None;
    for handle in handles {
        match handle.join() {
            Ok(Ok(result)) if result.status_code == 0 => {}
            Ok(Ok(result)) => {
                if first_error.is_none() {
                    first_error = Some(engine::stderr_or_stdout(&result));
                }
            }
            Ok(Err(error)) => {
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
            Err(_) => {
                if first_error.is_none() {
                    first_error =
                        Some("Lifecycle command thread panicked unexpectedly".to_string());
                }
            }
        }
    }

    if let Some(error) = first_error {
        return Err(error);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    //! Unit tests for lifecycle helper behavior.

    use std::collections::HashMap;

    use serde_json::json;

    use super::{
        dotfiles::dotfiles_install_command,
        requests::lifecycle_exec_args,
        selection::{lifecycle_command_group, selected_lifecycle_steps},
        LifecycleCommand, LifecycleMode, LifecycleStep,
    };

    #[test]
    fn lifecycle_command_group_supports_strings_arrays_and_objects() {
        assert!(lifecycle_command_group(&json!("echo hello")).is_some());
        assert!(lifecycle_command_group(&json!(["/bin/echo", "hello"])).is_some());
        assert!(lifecycle_command_group(&json!({
            "a": "echo one",
            "b": ["/bin/echo", "two"]
        }))
        .is_some());
    }

    #[test]
    fn selected_lifecycle_steps_respect_mode_and_wait_for() {
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
}
