#![allow(dead_code)]

//! Runtime smoke-test harness for invoking the devcontainer binary with a fake engine.

mod fake_engine;

use serde_json::Value;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};

use super::test_support::{devcontainer_command, unique_temp_dir};
use super::workspace_fixture::WorkspaceFixture;
use fake_engine::write_fake_podman;

pub(crate) struct RuntimeHarness {
    pub(crate) root: PathBuf,
    pub(crate) log_dir: PathBuf,
    pub(crate) fake_podman: PathBuf,
}

impl RuntimeHarness {
    pub(crate) fn new() -> Self {
        let root = unique_temp_dir("devcontainer-runtime-smoke");
        let log_dir = root.join("logs");
        fs::create_dir_all(&log_dir).expect("log dir");
        let fake_podman = write_fake_podman(&root);

        Self {
            root,
            log_dir,
            fake_podman,
        }
    }

    pub(crate) fn workspace(&self) -> PathBuf {
        self.root.join("workspace")
    }

    pub(crate) fn run(&self, args: &[&str], envs: &[(&str, &str)]) -> Output {
        self.run_in_dir(args, envs, None)
    }

    pub(crate) fn run_in_dir(
        &self,
        args: &[&str],
        envs: &[(&str, &str)],
        cwd: Option<&Path>,
    ) -> Output {
        let mut command = command(args, cwd);
        command.env(
            "FAKE_PODMAN_LOG_DIR",
            self.log_dir.to_string_lossy().as_ref(),
        );
        for (key, value) in envs {
            command.env(key, value);
        }
        command.output().expect("command should run")
    }

    pub(crate) fn run_with_input(
        &self,
        args: &[&str],
        envs: &[(&str, &str)],
        input: &str,
    ) -> Output {
        let mut command = command(args, None);
        command.stdin(Stdio::piped());
        command.stdout(Stdio::piped());
        command.stderr(Stdio::piped());
        command.env(
            "FAKE_PODMAN_LOG_DIR",
            self.log_dir.to_string_lossy().as_ref(),
        );
        for (key, value) in envs {
            command.env(key, value);
        }

        let mut child = command.spawn().expect("command should spawn");
        child
            .stdin
            .take()
            .expect("stdin")
            .write_all(input.as_bytes())
            .expect("write stdin");
        child.wait_with_output().expect("command should complete")
    }

    pub(crate) fn read_invocations(&self) -> String {
        fs::read_to_string(self.log_dir.join("invocations.log")).expect("invocations")
    }

    pub(crate) fn read_exec_log(&self) -> String {
        fs::read_to_string(self.log_dir.join("exec.log")).expect("exec log")
    }

    pub(crate) fn read_exec_argv_log(&self) -> String {
        fs::read_to_string(self.log_dir.join("exec-argv.log")).expect("exec argv log")
    }

    pub(crate) fn read_compose_file_log(&self) -> String {
        fs::read_to_string(self.log_dir.join("compose-file-contents.log"))
            .expect("compose file log")
    }

    pub(crate) fn parse_stdout_json(&self, output: &Output) -> Value {
        serde_json::from_slice(&output.stdout).expect("json payload")
    }
}

pub(crate) fn write_devcontainer_config(root: &Path, body: &str) -> PathBuf {
    WorkspaceFixture::new(root.to_path_buf()).write_devcontainer_config(body)
}

fn command(args: &[&str], cwd: Option<&Path>) -> std::process::Command {
    let mut command = devcontainer_command(cwd);
    command.args(args);
    command
}
