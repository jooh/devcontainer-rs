//! Process request and result types shared across runtime execution helpers.

use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::process::Command;
use std::thread;
use std::time::Duration;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ProcessLogLevel {
    #[default]
    Info,
    Debug,
    Trace,
}

pub struct ProcessRequest {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub env: HashMap<String, String>,
    pub log_level: ProcessLogLevel,
}

#[derive(Debug)]
pub struct ProcessResult {
    pub status_code: i32,
    pub stdout: String,
    pub stderr: String,
}

pub fn run_process(request: &ProcessRequest) -> Result<ProcessResult, io::Error> {
    log_request(request);

    retry_executable_file_busy(|| build_command(request).output()).map(|output| {
        let result = ProcessResult {
            status_code: output.status.code().unwrap_or(1),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        };
        log_result(request, result.status_code);
        result
    })
}

pub fn run_process_streaming(request: &ProcessRequest) -> Result<i32, io::Error> {
    log_request(request);
    retry_executable_file_busy(|| build_command(request).status()).map(|status| {
        let status_code = status.code().unwrap_or(1);
        log_result(request, status_code);
        status_code
    })
}

fn build_command(request: &ProcessRequest) -> Command {
    let mut command = Command::new(&request.program);
    command.args(&request.args);

    if let Some(cwd) = &request.cwd {
        command.current_dir(cwd);
    }

    if !request.env.is_empty() {
        command.envs(&request.env);
    }

    command
}

fn retry_executable_file_busy<T>(
    mut run: impl FnMut() -> Result<T, io::Error>,
) -> Result<T, io::Error> {
    const MAX_ATTEMPTS: u32 = 4;

    let mut attempt = 1;
    loop {
        match run() {
            Err(error) if is_executable_file_busy(&error) && attempt < MAX_ATTEMPTS => {
                thread::sleep(Duration::from_millis(10 * u64::from(attempt)));
                attempt += 1;
            }
            result => return result,
        }
    }
}

fn is_executable_file_busy(error: &io::Error) -> bool {
    error.raw_os_error() == Some(26)
}

fn log_request(request: &ProcessRequest) {
    match request.log_level {
        ProcessLogLevel::Info => {}
        ProcessLogLevel::Debug => eprintln!("+ {}", command_summary(request)),
        ProcessLogLevel::Trace => {
            eprintln!("+ {}", command_summary(request));
            if let Some(cwd) = &request.cwd {
                eprintln!("  cwd={}", cwd.display());
            }
            for env_entry in env_summary_entries(request) {
                eprintln!("  env {env_entry}");
            }
        }
    }
}

fn log_result(request: &ProcessRequest, status_code: i32) {
    if request.log_level == ProcessLogLevel::Trace {
        eprintln!("  exit={status_code}");
    }
}

fn command_summary(request: &ProcessRequest) -> String {
    let mut summary = vec![request.program.clone()];
    let mut redact_next_env_assignment = false;

    for arg in &request.args {
        if redact_next_env_assignment {
            summary.push(redacted_env_assignment(arg));
            redact_next_env_assignment = false;
            continue;
        }

        if matches!(arg.as_str(), "-e" | "--env") {
            summary.push(arg.clone());
            redact_next_env_assignment = true;
            continue;
        }

        if let Some(value) = arg.strip_prefix("--env=") {
            summary.push(format!("--env={}", redacted_env_assignment(value)));
            continue;
        }

        summary.push(arg.clone());
    }

    summary.join(" ")
}

fn redacted_env_assignment(value: &str) -> String {
    let Some((key, _)) = value.split_once('=') else {
        return value.to_string();
    };
    format!("{key}=<redacted>")
}

fn env_summary_entries(request: &ProcessRequest) -> Vec<String> {
    let mut entries = request
        .env
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>();
    entries.sort();
    entries
}

#[cfg(test)]
mod tests {
    use super::{
        command_summary, env_summary_entries, run_process, run_process_streaming, ProcessLogLevel,
        ProcessRequest,
    };
    use std::collections::HashMap;
    use std::path::PathBuf;

    #[test]
    fn captures_stdout_and_exit_status() {
        let result = run_process(&ProcessRequest {
            program: "/bin/sh".to_string(),
            args: vec!["-c".to_string(), "printf native-process".to_string()],
            cwd: None,
            env: HashMap::new(),
            log_level: ProcessLogLevel::Info,
        })
        .expect("expected process to run");

        assert_eq!(result.status_code, 0);
        assert_eq!(result.stdout, "native-process");
        assert_eq!(result.stderr, "");
    }

    #[test]
    fn returns_status_for_streaming_processes() {
        let status = run_process_streaming(&ProcessRequest {
            program: "/bin/sh".to_string(),
            args: vec!["-c".to_string(), "exit 0".to_string()],
            cwd: None,
            env: HashMap::new(),
            log_level: ProcessLogLevel::Info,
        })
        .expect("expected streaming process to run");

        assert_eq!(status, 0);
    }

    #[test]
    fn executable_file_busy_process_spawn_is_retried() {
        let mut attempts = 0;
        let result = super::retry_executable_file_busy(|| {
            attempts += 1;
            if attempts < 3 {
                return Err(std::io::Error::from_raw_os_error(26));
            }
            Ok("started")
        })
        .expect("retry should eventually succeed");

        assert_eq!(result, "started");
        assert_eq!(attempts, 3);
    }

    #[test]
    fn executable_file_busy_retry_returns_final_error() {
        let mut attempts = 0;
        let error = super::retry_executable_file_busy(|| -> Result<(), std::io::Error> {
            attempts += 1;
            Err(std::io::Error::from_raw_os_error(26))
        })
        .expect_err("final busy error should be returned");

        assert_eq!(error.raw_os_error(), Some(26));
        assert_eq!(attempts, 4);
    }

    #[test]
    fn non_busy_process_spawn_errors_are_not_retried() {
        let mut attempts = 0;
        let error = super::retry_executable_file_busy(|| -> Result<(), std::io::Error> {
            attempts += 1;
            Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied))
        })
        .expect_err("non-busy errors should not retry");

        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
        assert_eq!(attempts, 1);
    }

    #[test]
    fn run_process_applies_cwd_env_and_trace_logging() {
        let cwd = crate::test_support::unique_temp_dir("process-runner-cwd");
        std::fs::create_dir_all(&cwd).expect("cwd");
        let expected_cwd = std::fs::canonicalize(&cwd).expect("canonical cwd");
        let result = run_process(&ProcessRequest {
            program: "/bin/sh".to_string(),
            args: vec![
                "-c".to_string(),
                "test \"$CHECK_VALUE\" = expected && test \"$(pwd)\" = \"$EXPECTED_CWD\" && printf ok"
                    .to_string(),
            ],
            cwd: Some(cwd.clone()),
            env: HashMap::from([
                ("CHECK_VALUE".to_string(), "expected".to_string()),
                ("EXPECTED_CWD".to_string(), expected_cwd.display().to_string()),
            ]),
            log_level: ProcessLogLevel::Trace,
        })
        .expect("expected process to run");

        assert_eq!(result.status_code, 0);
        assert_eq!(result.stdout, "ok");
        let _ = std::fs::remove_dir_all(cwd);
    }

    #[test]
    fn run_process_streaming_supports_debug_logging() {
        let status = run_process_streaming(&ProcessRequest {
            program: "/bin/sh".to_string(),
            args: vec!["-c".to_string(), "exit 0".to_string()],
            cwd: None,
            env: HashMap::new(),
            log_level: ProcessLogLevel::Debug,
        })
        .expect("expected streaming process to run");

        assert_eq!(status, 0);
    }

    #[test]
    fn run_process_streaming_reports_spawn_errors() {
        let error = run_process_streaming(&ProcessRequest {
            program: "/definitely/missing/devcontainer-test-command".to_string(),
            args: Vec::new(),
            cwd: None,
            env: HashMap::new(),
            log_level: ProcessLogLevel::Info,
        })
        .expect_err("missing executable should be reported");

        assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
    }

    #[test]
    fn trace_request_summaries_include_sorted_env_and_cwd() {
        let request = ProcessRequest {
            program: "docker".to_string(),
            args: vec!["exec".to_string(), "container".to_string()],
            cwd: Some(PathBuf::from("/tmp/workspace")),
            env: HashMap::from([
                ("LINES".to_string(), "40".to_string()),
                ("COLUMNS".to_string(), "120".to_string()),
            ]),
            log_level: ProcessLogLevel::Trace,
        };

        assert_eq!(command_summary(&request), "docker exec container");
        assert_eq!(
            env_summary_entries(&request),
            vec!["COLUMNS=120".to_string(), "LINES=40".to_string()]
        );
    }

    #[test]
    fn command_summary_redacts_env_assignment_values() {
        let request = ProcessRequest {
            program: "docker".to_string(),
            args: vec![
                "exec".to_string(),
                "-e".to_string(),
                "TOKEN=super-secret".to_string(),
                "--env".to_string(),
                "API_KEY=hunter2".to_string(),
                "--env=SESSION=abcdef".to_string(),
                "container".to_string(),
            ],
            cwd: None,
            env: HashMap::new(),
            log_level: ProcessLogLevel::Debug,
        };

        assert_eq!(
            command_summary(&request),
            "docker exec -e TOKEN=<redacted> --env API_KEY=<redacted> --env=SESSION=<redacted> container"
        );
    }

    #[test]
    fn command_summary_preserves_env_arguments_without_assignment() {
        let request = ProcessRequest {
            program: "docker".to_string(),
            args: vec![
                "exec".to_string(),
                "--env".to_string(),
                "TOKEN".to_string(),
                "container".to_string(),
            ],
            cwd: None,
            env: HashMap::new(),
            log_level: ProcessLogLevel::Debug,
        };

        assert_eq!(
            command_summary(&request),
            "docker exec --env TOKEN container"
        );
    }
}
