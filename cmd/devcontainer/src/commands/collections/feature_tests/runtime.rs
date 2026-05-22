//! Runtime execution helpers for feature test commands.

use std::fs;
use std::path::Path;

use super::discovery::prepare_feature_test_case;
use super::materialize::{
    shell_single_quote, unique_feature_test_name, write_feature_test_dockerfile,
};
use super::{BaseImageSource, FeatureTestCase, FeatureTestOptions, FeatureTestResult};
use crate::runtime;

pub(crate) trait FeatureTestRuntime {
    fn build_image(
        &mut self,
        args: &[String],
        image_name: &str,
        dockerfile_path: &Path,
        context_path: &Path,
    ) -> Result<(), String>;
    fn start_container(
        &mut self,
        args: &[String],
        image_name: &str,
        workspace_dir: &Path,
    ) -> Result<String, String>;
    fn exec_script(
        &mut self,
        args: &[String],
        container_id: &str,
        workspace_dir: &Path,
        remote_user: Option<&str>,
        env: &[(String, String)],
        script_name: &str,
    ) -> Result<i32, String>;
    fn remove_container(&mut self, args: &[String], container_id: &str) -> Result<(), String>;
}

pub(super) struct ContainerEngineFeatureTestRuntime;

impl FeatureTestRuntime for ContainerEngineFeatureTestRuntime {
    fn build_image(
        &mut self,
        args: &[String],
        image_name: &str,
        dockerfile_path: &Path,
        context_path: &Path,
    ) -> Result<(), String> {
        let result = runtime::engine::run_engine(
            args,
            vec![
                "build".to_string(),
                "--tag".to_string(),
                image_name.to_string(),
                "--file".to_string(),
                dockerfile_path.display().to_string(),
                context_path.display().to_string(),
            ],
        )?;
        if result.status_code != 0 {
            return Err(runtime::engine::stderr_or_stdout(&result));
        }
        Ok(())
    }

    fn start_container(
        &mut self,
        args: &[String],
        image_name: &str,
        workspace_dir: &Path,
    ) -> Result<String, String> {
        let result = runtime::engine::run_engine(
            args,
            vec![
                "run".to_string(),
                "-d".to_string(),
                "--label".to_string(),
                "devcontainer.is_test_run=true".to_string(),
                "--mount".to_string(),
                format!(
                    "type=bind,source={},target=/workspace",
                    workspace_dir.display()
                ),
                "--workdir".to_string(),
                "/workspace".to_string(),
                image_name.to_string(),
                "/bin/sh".to_string(),
                "-lc".to_string(),
                "while sleep 1000; do :; done".to_string(),
            ],
        )?;
        if result.status_code != 0 {
            return Err(runtime::engine::stderr_or_stdout(&result));
        }
        Ok(result.stdout.trim().to_string())
    }

    fn exec_script(
        &mut self,
        args: &[String],
        container_id: &str,
        _workspace_dir: &Path,
        remote_user: Option<&str>,
        env: &[(String, String)],
        script_name: &str,
    ) -> Result<i32, String> {
        let mut engine_args = vec![
            "exec".to_string(),
            "--workdir".to_string(),
            "/workspace".to_string(),
        ];
        if let Some(remote_user) = remote_user {
            engine_args.push("--user".to_string());
            engine_args.push(remote_user.to_string());
        }
        for (key, value) in env {
            engine_args.push("-e".to_string());
            engine_args.push(format!("{key}={value}"));
        }
        engine_args.push(container_id.to_string());
        engine_args.push("/bin/bash".to_string());
        engine_args.push("-lc".to_string());
        engine_args.push(format!(
            "chmod -R 777 /workspace && {}",
            shell_single_quote(&format!("./{script_name}"))
        ));
        runtime::engine::run_engine_streaming(args, engine_args)
    }

    fn remove_container(&mut self, args: &[String], container_id: &str) -> Result<(), String> {
        let result = runtime::engine::run_engine(
            args,
            vec!["rm".to_string(), "-f".to_string(), container_id.to_string()],
        )?;
        if result.status_code != 0 {
            return Err(runtime::engine::stderr_or_stdout(&result));
        }
        Ok(())
    }
}

pub(super) fn execute_feature_tests_with_runtime<R: FeatureTestRuntime>(
    args: &[String],
    runtime: &mut R,
    options: &FeatureTestOptions,
    cases: Vec<FeatureTestCase>,
) -> Result<Vec<FeatureTestResult>, String> {
    let mut results = Vec::with_capacity(cases.len());

    for case in cases {
        let prepared = prepare_feature_test_case(options, &case)?;
        let base_image = match &prepared.base_image {
            BaseImageSource::Image(image) => image.clone(),
            BaseImageSource::Build {
                dockerfile_path,
                context_path,
            } => {
                let image_name = unique_feature_test_name("devcontainer-feature-test-base");
                runtime.build_image(args, &image_name, dockerfile_path, context_path)?;
                image_name
            }
        };
        let dockerfile_path = write_feature_test_dockerfile(
            &prepared.build_context_dir,
            &base_image,
            &prepared.feature_installations,
        )?;
        let image_name = unique_feature_test_name("devcontainer-feature-test");
        runtime.build_image(
            args,
            &image_name,
            &dockerfile_path,
            &prepared.build_context_dir,
        )?;
        let container_id = runtime.start_container(args, &image_name, &prepared.workspace_dir)?;
        let status = runtime.exec_script(
            args,
            &container_id,
            &prepared.workspace_dir,
            prepared.remote_user.as_deref(),
            &prepared.exec_env,
            &prepared.script_name,
        )?;
        if !options.preserve_test_containers {
            runtime.remove_container(args, &container_id)?;
            let _ = fs::remove_dir_all(&prepared.workspace_dir);
        }
        results.push(FeatureTestResult {
            name: case.name,
            passed: status == 0,
        });
    }

    Ok(results)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use super::{ContainerEngineFeatureTestRuntime, FeatureTestRuntime};

    fn write_engine_script(root: &Path, fail_command: Option<&str>) -> PathBuf {
        fs::create_dir_all(root).expect("runtime test root");
        let script_path = root.join("fake-engine");
        let log_path = root.join("engine.log");
        let fail_command = fail_command.unwrap_or("");
        crate::test_support::write_executable_script(
            &script_path,
            &format!(
                r#"#!/bin/sh
set -eu
command="$1"
shift
printf '%s %s\n' "$command" "$*" >> {}
if [ "$command" = "{fail_command}" ]; then
  printf '%s failed\n' "$command" >&2
  exit 10
fi
case "$command" in
  build)
    exit 0
    ;;
  run)
    echo "container-from-runtime"
    exit 0
    ;;
  exec)
    exit 0
    ;;
  rm)
    exit 0
    ;;
  *)
    echo "unsupported command: $command" >&2
    exit 1
    ;;
esac
"#,
                super::shell_single_quote(log_path.to_string_lossy().as_ref())
            ),
        );
        script_path
    }

    #[test]
    fn container_engine_runtime_passes_build_run_exec_and_remove_arguments() {
        let root = crate::test_support::unique_temp_dir("feature-test-runtime");
        let engine = write_engine_script(&root, None);
        let args = vec!["--docker-path".to_string(), engine.display().to_string()];
        let dockerfile = root.join("Dockerfile");
        let context = root.join("context");
        fs::write(&dockerfile, "FROM scratch\n").expect("dockerfile");
        fs::create_dir_all(&context).expect("context");

        let mut runtime = ContainerEngineFeatureTestRuntime;
        runtime
            .build_image(&args, "feature-test-image", &dockerfile, &context)
            .expect("build image");
        let container_id = runtime
            .start_container(&args, "feature-test-image", &root)
            .expect("start container");
        let status = runtime
            .exec_script(
                &args,
                &container_id,
                &root,
                Some("vscode"),
                &[("COLOR".to_string(), "green".to_string())],
                "test.sh",
            )
            .expect("exec script");
        runtime
            .remove_container(&args, &container_id)
            .expect("remove container");

        assert_eq!(container_id, "container-from-runtime");
        assert_eq!(status, 0);
        let log = fs::read_to_string(root.join("engine.log")).expect("engine log");
        assert!(
            log.contains("build --tag feature-test-image --file"),
            "{log}"
        );
        assert!(
            log.contains("run -d --label devcontainer.is_test_run=true"),
            "{log}"
        );
        assert!(
            log.contains("exec --workdir /workspace --user vscode -e COLOR=green"),
            "{log}"
        );
        assert!(log.contains("rm -f container-from-runtime"), "{log}");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn container_engine_runtime_omits_optional_exec_arguments_when_absent() {
        let root = crate::test_support::unique_temp_dir("feature-test-runtime");
        let engine = write_engine_script(&root, None);
        let args = vec!["--docker-path".to_string(), engine.display().to_string()];

        let mut runtime = ContainerEngineFeatureTestRuntime;
        let status = runtime
            .exec_script(&args, "container-from-runtime", &root, None, &[], "test.sh")
            .expect("exec script");

        assert_eq!(status, 0);
        let log = fs::read_to_string(root.join("engine.log")).expect("engine log");
        assert!(log.contains("exec --workdir /workspace"), "{log}");
        assert!(!log.contains(" --user "), "{log}");
        assert!(!log.contains(" -e "), "{log}");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn container_engine_runtime_reports_build_run_and_remove_failures() {
        for (command, operation) in [
            ("build", "build failed"),
            ("run", "run failed"),
            ("rm", "rm failed"),
        ] {
            let root = crate::test_support::unique_temp_dir("feature-test-runtime");
            let engine = write_engine_script(&root, Some(command));
            let args = vec!["--docker-path".to_string(), engine.display().to_string()];
            let dockerfile = root.join("Dockerfile");
            let context = root.join("context");
            fs::write(&dockerfile, "FROM scratch\n").expect("dockerfile");
            fs::create_dir_all(&context).expect("context");

            let mut runtime = ContainerEngineFeatureTestRuntime;
            let error = if command == "build" {
                runtime
                    .build_image(&args, "feature-test-image", &dockerfile, &context)
                    .expect_err("build should fail")
            } else if command == "run" {
                runtime
                    .start_container(&args, "feature-test-image", &root)
                    .expect_err("run should fail")
            } else {
                runtime
                    .remove_container(&args, "container-from-runtime")
                    .expect_err("rm should fail")
            };

            assert_eq!(error, operation);
            let _ = fs::remove_dir_all(root);
        }
    }

    #[test]
    fn container_engine_runtime_reports_process_spawn_failures() {
        let root = crate::test_support::unique_temp_dir("feature-test-runtime");
        let missing_engine = root.join("missing-engine");
        let args = vec![
            "--docker-path".to_string(),
            missing_engine.display().to_string(),
        ];
        let dockerfile = root.join("Dockerfile");
        let context = root.join("context");
        fs::create_dir_all(&root).expect("runtime test root");
        fs::write(&dockerfile, "FROM scratch\n").expect("dockerfile");
        fs::create_dir_all(&context).expect("context");

        let mut runtime = ContainerEngineFeatureTestRuntime;
        for error in [
            runtime
                .build_image(&args, "feature-test-image", &dockerfile, &context)
                .expect_err("build spawn should fail"),
            runtime
                .start_container(&args, "feature-test-image", &root)
                .expect_err("run spawn should fail"),
            runtime
                .remove_container(&args, "container-from-runtime")
                .expect_err("rm spawn should fail"),
            runtime
                .exec_script(&args, "container-from-runtime", &root, None, &[], "test.sh")
                .expect_err("exec spawn should fail"),
        ] {
            assert!(
                error.contains("No such file") || error.contains("not found"),
                "{error}"
            );
        }

        let _ = fs::remove_dir_all(root);
    }
}
