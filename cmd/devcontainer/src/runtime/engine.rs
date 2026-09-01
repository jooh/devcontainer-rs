//! Container engine invocation helpers for native runtime commands.

use std::io;
use std::path::Path;

use crate::commands::common;
use crate::process_runner::{self, ProcessRequest, ProcessResult};

pub(crate) fn engine_request(args: &[String], engine_args: Vec<String>) -> ProcessRequest {
    let mut request =
        common::runtime_process_request(args, effective_engine_program(args), engine_args, None);
    let request_args = request.args.clone();
    apply_buildkit_env(args, &request_args, &mut request);
    request
}

pub(crate) fn run_engine(
    args: &[String],
    engine_args: Vec<String>,
) -> Result<ProcessResult, String> {
    let request = engine_request(args, engine_args);
    process_runner::run_process(&request)
        .map_err(|error| normalize_process_error(args, &request, error))
}

pub(crate) fn run_engine_streaming(
    args: &[String],
    engine_args: Vec<String>,
) -> Result<i32, String> {
    let request = engine_request(args, engine_args);
    process_runner::run_process_streaming(&request)
        .map_err(|error| normalize_process_error(args, &request, error))
}

pub(crate) fn compose_request(args: &[String], compose_args: Vec<String>) -> ProcessRequest {
    if let Some(compose_program) = requested_compose_program(args) {
        let mut request =
            common::runtime_process_request(args, compose_program, compose_args, None);
        let request_args = request.args.clone();
        apply_buildkit_env(args, &request_args, &mut request);
        request
    } else if default_compose_subcommand_available(args) {
        let mut args_with_subcommand = vec!["compose".to_string()];
        args_with_subcommand.extend(compose_args);
        let mut request = common::runtime_process_request(
            args,
            effective_engine_program(args),
            args_with_subcommand,
            None,
        );
        let request_args = request.args.clone();
        apply_buildkit_env(args, &request_args, &mut request);
        request
    } else {
        let mut request =
            common::runtime_process_request(args, "docker-compose".to_string(), compose_args, None);
        let request_args = request.args.clone();
        apply_buildkit_env(args, &request_args, &mut request);
        request
    }
}

pub(crate) fn run_compose(
    args: &[String],
    compose_args: Vec<String>,
) -> Result<ProcessResult, String> {
    let request = compose_request(args, compose_args);
    process_runner::run_process(&request)
        .map_err(|error| normalize_process_error(args, &request, error))
}

pub(crate) fn stderr_or_stdout(result: &ProcessResult) -> String {
    if result.stderr.trim().is_empty() {
        result.stdout.trim().to_string()
    } else {
        result.stderr.trim().to_string()
    }
}

pub(crate) fn requested_engine_program(args: &[String]) -> Option<String> {
    common::env_default_option_value(args, "--docker-path", common::DEVCONTAINER_DOCKER_PATH)
}

pub(crate) fn effective_engine_program(args: &[String]) -> String {
    requested_engine_program(args).unwrap_or_else(|| "docker".to_string())
}

pub(crate) fn is_wslc(args: &[String]) -> bool {
    run_engine(args, vec!["-v".to_string()])
        .map(|result| {
            result.status_code == 0 && result.stdout.to_ascii_lowercase().contains("wslc")
        })
        .unwrap_or(false)
}

pub(crate) fn requested_compose_program(args: &[String]) -> Option<String> {
    common::env_default_option_value(
        args,
        "--docker-compose-path",
        common::DEVCONTAINER_DOCKER_COMPOSE_PATH,
    )
}

pub(crate) fn pull_always_requested(args: &[String]) -> bool {
    let mut requested = false;
    let mut index = 0;
    while let Some(arg) = args.get(index) {
        if arg == "--pull-always" {
            if let Some(value) = args
                .get(index + 1)
                .and_then(|value| cli_boolean_value(value))
            {
                requested = value;
                index += 2;
            } else {
                requested = true;
                index += 1;
            }
            continue;
        }
        if let Some(value) = arg
            .strip_prefix("--pull-always=")
            .and_then(cli_boolean_value)
        {
            requested = value;
        }
        index += 1;
    }
    requested
}

fn cli_boolean_value(value: &str) -> Option<bool> {
    match value {
        "true" | "1" | "yes" | "on" => Some(true),
        "false" | "0" | "no" | "off" => Some(false),
        _ => None,
    }
}

fn default_compose_subcommand_available(args: &[String]) -> bool {
    let request = common::runtime_process_request(
        args,
        effective_engine_program(args),
        vec![
            "compose".to_string(),
            "version".to_string(),
            "--short".to_string(),
        ],
        None,
    );
    process_runner::run_process(&request)
        .map(|result| result.status_code == 0)
        .unwrap_or(false)
}

fn normalize_process_error(args: &[String], request: &ProcessRequest, error: io::Error) -> String {
    if error.kind() != io::ErrorKind::NotFound {
        return error.to_string();
    }

    let executable = request.program.as_str();
    if requested_compose_program(args)
        .as_deref()
        .is_some_and(|program| program == executable)
        || Path::new(executable)
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.eq_ignore_ascii_case("docker-compose"))
    {
        return format!(
            "Container compose executable not found: {executable}. Verify --docker-compose-path or DEVCONTAINER_DOCKER_COMPOSE_PATH, or install the requested compose CLI."
        );
    }

    let requested_engine = requested_engine_program(args);
    if requested_engine.is_none()
        && Path::new(executable)
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.eq_ignore_ascii_case("docker"))
    {
        return "Container engine executable not found: docker. Install Docker or rerun with --docker-path podman or set DEVCONTAINER_DOCKER_PATH=podman.".to_string();
    }

    format!(
        "Container engine executable not found: {executable}. Verify --docker-path or DEVCONTAINER_DOCKER_PATH, or install the requested container engine."
    )
}

fn apply_buildkit_env(args: &[String], request_args: &[String], request: &mut ProcessRequest) {
    if !is_build_request(request_args) {
        return;
    }
    match common::runtime_options(args).buildkit.as_deref() {
        Some("never") => {
            request
                .env
                .insert("DOCKER_BUILDKIT".to_string(), "0".to_string());
        }
        Some("auto") => {
            request
                .env
                .insert("DOCKER_BUILDKIT".to_string(), "1".to_string());
        }
        _ => {}
    }
}

fn is_build_request(request_args: &[String]) -> bool {
    let mut index = usize::from(request_args.first().map(String::as_str) == Some("compose"));

    if request_args.get(index).map(String::as_str) == Some("build") {
        return true;
    }

    while index < request_args.len() {
        match request_args[index].as_str() {
            "--project-name" | "-f" => {
                index += 2;
            }
            value if value.starts_with('-') => {
                index += 1;
            }
            "build" => return true,
            _ => return false,
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use std::io;

    use crate::commands::common::{
        test_env_defaults, DEVCONTAINER_BUILDKIT, DEVCONTAINER_DOCKER_COMPOSE_PATH,
        DEVCONTAINER_DOCKER_PATH,
    };
    use crate::process_runner::{ProcessLogLevel, ProcessRequest, ProcessResult};

    use super::{
        compose_request, default_compose_subcommand_available, engine_request, is_build_request,
        is_wslc, normalize_process_error, pull_always_requested, run_compose, run_engine,
        run_engine_streaming, stderr_or_stdout,
    };

    #[test]
    fn pull_always_requested_honors_the_cli_boolean_vocabulary_and_last_value() {
        for value in ["true", "1", "yes", "on"] {
            assert!(
                pull_always_requested(&["--pull-always".to_string(), value.to_string()]),
                "spaced {value}"
            );
            assert!(
                pull_always_requested(&[format!("--pull-always={value}")]),
                "equals {value}"
            );
        }
        for value in ["false", "0", "no", "off"] {
            assert!(
                !pull_always_requested(&["--pull-always".to_string(), value.to_string()]),
                "spaced {value}"
            );
            assert!(
                !pull_always_requested(&[format!("--pull-always={value}")]),
                "equals {value}"
            );
        }

        assert!(pull_always_requested(&["--pull-always".to_string()]));
        assert!(!pull_always_requested(&[]));
        assert!(!pull_always_requested(&[
            "--pull-always=yes".to_string(),
            "--pull-always".to_string(),
            "off".to_string(),
        ]));
        assert!(pull_always_requested(&[
            "--pull-always".to_string(),
            "false".to_string(),
            "--pull-always=on".to_string(),
        ]));
        assert!(!pull_always_requested(&[
            "--pull-always=invalid".to_string()
        ]));
        assert!(pull_always_requested(&[
            "--pull-always".to_string(),
            "invalid".to_string(),
        ]));
    }

    #[test]
    fn engine_request_applies_terminal_env_and_log_level() {
        let request = engine_request(
            &[
                "--log-level".to_string(),
                "debug".to_string(),
                "--terminal-columns".to_string(),
                "160".to_string(),
                "--terminal-rows".to_string(),
                "48".to_string(),
            ],
            vec!["ps".to_string()],
        );

        assert_eq!(request.log_level, ProcessLogLevel::Debug);
        assert_eq!(request.env.get("COLUMNS").map(String::as_str), Some("160"));
        assert_eq!(request.env.get("LINES").map(String::as_str), Some("48"));
    }

    #[test]
    fn detects_wslc_from_the_engine_version_banner() {
        let root = crate::test_support::unique_temp_dir("devcontainer-wslc-engine-test");
        std::fs::create_dir_all(&root).expect("root");
        let engine = root.join("docker");
        crate::test_support::write_executable_script(
            &engine,
            "#!/bin/sh\nprintf 'wslc version 0.1.0\\n'\n",
        );

        assert!(is_wslc(&[
            "--docker-path".to_string(),
            engine.display().to_string(),
        ]));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn stderr_or_stdout_falls_back_to_stdout_when_stderr_is_empty() {
        let result = ProcessResult {
            status_code: 1,
            stdout: " useful stdout \n".to_string(),
            stderr: " \n".to_string(),
        };

        assert_eq!(stderr_or_stdout(&result), "useful stdout");
    }

    #[test]
    fn detects_build_requests_for_compose_invocations() {
        assert!(!is_build_request(&[]));
        assert!(is_build_request(&["build".to_string()]));
        assert!(is_build_request(&[
            "--pull".to_string(),
            "build".to_string(),
        ]));
        assert!(!is_build_request(&["--pull".to_string()]));
        assert!(is_build_request(&[
            "compose".to_string(),
            "build".to_string(),
            "app".to_string(),
        ]));
        assert!(is_build_request(&[
            "--project-name".to_string(),
            "workspace".to_string(),
            "-f".to_string(),
            "docker-compose.yml".to_string(),
            "build".to_string(),
            "app".to_string(),
        ]));
        assert!(is_build_request(&[
            "compose".to_string(),
            "--project-name".to_string(),
            "workspace".to_string(),
            "build".to_string(),
            "app".to_string(),
        ]));
        assert!(!is_build_request(&[
            "--project-name".to_string(),
            "workspace".to_string(),
            "up".to_string(),
        ]));
        assert!(!is_build_request(&[
            "compose".to_string(),
            "up".to_string(),
        ]));
        assert!(!is_build_request(&[
            "compose".to_string(),
            "--ansi".to_string(),
            "never".to_string(),
        ]));
    }

    #[test]
    fn compose_request_applies_buildkit_env_for_default_docker_compose_builds() {
        let request = compose_request(
            &[
                "--docker-path".to_string(),
                "/path/that/does/not/exist".to_string(),
                "--buildkit".to_string(),
                "never".to_string(),
            ],
            vec!["build".to_string(), "app".to_string()],
        );

        assert_eq!(request.program, "docker-compose");
        assert_eq!(request.args, vec!["build".to_string(), "app".to_string()]);
        assert_eq!(
            request.env.get("DOCKER_BUILDKIT").map(String::as_str),
            Some("0")
        );
    }

    #[test]
    fn engine_request_uses_env_engine_path_and_ignores_blank_env() {
        let env = test_env_defaults(&[(DEVCONTAINER_DOCKER_PATH, "/env/bin/podman")]);
        let request = engine_request(&[], vec!["ps".to_string()]);
        assert_eq!(request.program, "/env/bin/podman");

        drop(env);
        let _env = test_env_defaults(&[(DEVCONTAINER_DOCKER_PATH, " \t ")]);
        let request = engine_request(&[], vec!["ps".to_string()]);
        assert_eq!(request.program, "docker");
    }

    #[test]
    fn engine_request_prefers_cli_engine_path_over_env() {
        let _env = test_env_defaults(&[(DEVCONTAINER_DOCKER_PATH, "/env/bin/podman")]);

        let request = engine_request(
            &["--docker-path".to_string(), "/cli/bin/docker".to_string()],
            vec!["ps".to_string()],
        );

        assert_eq!(request.program, "/cli/bin/docker");
    }

    #[test]
    fn compose_request_uses_env_compose_path_before_engine_probe() {
        let _env = test_env_defaults(&[
            (DEVCONTAINER_DOCKER_PATH, "/path/that/does/not/exist"),
            (DEVCONTAINER_DOCKER_COMPOSE_PATH, "/env/bin/podman-compose"),
        ]);

        let request = compose_request(&[], vec!["ps".to_string()]);

        assert_eq!(request.program, "/env/bin/podman-compose");
        assert_eq!(request.args, vec!["ps".to_string()]);
    }

    #[test]
    fn compose_request_prefers_cli_compose_path_over_env() {
        let _env =
            test_env_defaults(&[(DEVCONTAINER_DOCKER_COMPOSE_PATH, "/env/bin/podman-compose")]);

        let request = compose_request(
            &[
                "--docker-compose-path".to_string(),
                "/cli/bin/docker-compose".to_string(),
            ],
            vec!["ps".to_string()],
        );

        assert_eq!(request.program, "/cli/bin/docker-compose");
    }

    #[test]
    fn engine_request_applies_buildkit_env_default_for_builds() {
        let env = test_env_defaults(&[(DEVCONTAINER_BUILDKIT, "never")]);

        let request = engine_request(&[], vec!["build".to_string(), "app".to_string()]);

        assert_eq!(
            request.env.get("DOCKER_BUILDKIT").map(String::as_str),
            Some("0")
        );

        drop(env);
        let _env = test_env_defaults(&[(DEVCONTAINER_BUILDKIT, " ")]);
        let request = engine_request(&[], vec!["build".to_string(), "app".to_string()]);
        assert_eq!(request.env.get("DOCKER_BUILDKIT"), None);
    }

    #[test]
    fn engine_request_prefers_cli_buildkit_over_env() {
        let _env = test_env_defaults(&[(DEVCONTAINER_BUILDKIT, "never")]);

        let request = engine_request(
            &["--buildkit".to_string(), "auto".to_string()],
            vec!["build".to_string(), "app".to_string()],
        );

        assert_eq!(
            request.env.get("DOCKER_BUILDKIT").map(String::as_str),
            Some("1")
        );
    }

    #[test]
    fn engine_request_applies_buildkit_never_for_builds() {
        let request = engine_request(
            &["--buildkit".to_string(), "never".to_string()],
            vec!["build".to_string(), "app".to_string()],
        );

        assert_eq!(
            request.env.get("DOCKER_BUILDKIT").map(String::as_str),
            Some("0")
        );
    }

    #[test]
    fn compose_request_honors_explicit_compose_path_and_buildkit_auto() {
        let request = compose_request(
            &[
                "--docker-compose-path".to_string(),
                "/opt/bin/docker-compose".to_string(),
                "--buildkit".to_string(),
                "auto".to_string(),
            ],
            vec!["build".to_string(), "app".to_string()],
        );

        assert_eq!(request.program, "/opt/bin/docker-compose");
        assert_eq!(request.args, vec!["build".to_string(), "app".to_string()]);
        assert_eq!(
            request.env.get("DOCKER_BUILDKIT").map(String::as_str),
            Some("1")
        );
    }

    #[test]
    fn default_compose_subcommand_probe_fails_without_engine() {
        assert!(!default_compose_subcommand_available(&[
            "--docker-path".to_string(),
            "/path/that/does/not/exist".to_string(),
        ]));
    }

    #[test]
    fn run_engine_reports_missing_requested_engine() {
        let error = run_engine(
            &[
                "--docker-path".to_string(),
                "/path/that/does/not/exist".to_string(),
            ],
            vec!["ps".to_string()],
        )
        .expect_err("missing requested engine should fail");

        assert_eq!(
            error,
            "Container engine executable not found: /path/that/does/not/exist. Verify --docker-path or DEVCONTAINER_DOCKER_PATH, or install the requested container engine."
        );
    }

    #[test]
    fn run_engine_reports_missing_env_requested_engine() {
        let _env =
            test_env_defaults(&[(DEVCONTAINER_DOCKER_PATH, "/path/that/does/not/exist-env")]);

        let error = run_engine(&[], vec!["ps".to_string()])
            .expect_err("missing env requested engine should fail");

        assert_eq!(
            error,
            "Container engine executable not found: /path/that/does/not/exist-env. Verify --docker-path or DEVCONTAINER_DOCKER_PATH, or install the requested container engine."
        );
    }

    #[test]
    fn run_compose_reports_missing_env_requested_compose_binary() {
        let _env = test_env_defaults(&[(
            DEVCONTAINER_DOCKER_COMPOSE_PATH,
            "/path/that/does/not/exist-compose-env",
        )]);

        let error = run_compose(&[], vec!["ps".to_string()])
            .expect_err("missing env requested compose should fail");

        assert_eq!(
            error,
            "Container compose executable not found: /path/that/does/not/exist-compose-env. Verify --docker-compose-path or DEVCONTAINER_DOCKER_COMPOSE_PATH, or install the requested compose CLI."
        );
    }

    #[test]
    fn run_engine_streaming_reports_missing_requested_engine() {
        let error = run_engine_streaming(
            &[
                "--docker-path".to_string(),
                "/path/that/does/not/exist".to_string(),
            ],
            vec!["ps".to_string()],
        )
        .expect_err("missing requested engine should fail");

        assert_eq!(
            error,
            "Container engine executable not found: /path/that/does/not/exist. Verify --docker-path or DEVCONTAINER_DOCKER_PATH, or install the requested container engine."
        );
    }

    #[test]
    fn normalize_process_error_reports_missing_compose_binary() {
        let request = ProcessRequest {
            program: "/path/that/does/not/exist-compose".to_string(),
            args: vec!["ps".to_string()],
            cwd: None,
            env: Default::default(),
            log_level: ProcessLogLevel::Info,
        };

        let error = normalize_process_error(
            &[
                "--docker-compose-path".to_string(),
                "/path/that/does/not/exist-compose".to_string(),
            ],
            &request,
            io::Error::new(io::ErrorKind::NotFound, "missing"),
        );

        assert_eq!(
            error,
            "Container compose executable not found: /path/that/does/not/exist-compose. Verify --docker-compose-path or DEVCONTAINER_DOCKER_COMPOSE_PATH, or install the requested compose CLI."
        );
    }

    #[test]
    fn normalize_process_error_preserves_non_not_found_errors() {
        let request = ProcessRequest {
            program: "docker".to_string(),
            args: vec!["ps".to_string()],
            cwd: None,
            env: Default::default(),
            log_level: ProcessLogLevel::Info,
        };

        let error = normalize_process_error(
            &[],
            &request,
            io::Error::new(io::ErrorKind::PermissionDenied, "permission denied"),
        );

        assert_eq!(error, "permission denied");
    }

    #[test]
    fn normalize_process_error_reports_missing_default_docker() {
        let request = ProcessRequest {
            program: "docker".to_string(),
            args: vec!["ps".to_string()],
            cwd: None,
            env: Default::default(),
            log_level: ProcessLogLevel::Info,
        };

        let error = normalize_process_error(
            &[],
            &request,
            io::Error::new(io::ErrorKind::NotFound, "missing"),
        );

        assert_eq!(
            error,
            "Container engine executable not found: docker. Install Docker or rerun with --docker-path podman or set DEVCONTAINER_DOCKER_PATH=podman."
        );
    }
}
