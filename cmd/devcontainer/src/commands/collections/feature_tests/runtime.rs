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
        let result = crate::coverage_expect_result!(
            runtime::engine::run_engine(
                args,
                vec![
                    "build".to_string(),
                    "--tag".to_string(),
                    image_name.to_string(),
                    "--file".to_string(),
                    dockerfile_path.display().to_string(),
                    context_path.display().to_string(),
                ],
            ),
            "feature-test image build process launch failures are covered by engine helpers"
        );
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
        let result = crate::coverage_expect_result!(
            runtime::engine::run_engine(
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
            ),
            "feature-test container start process launch failures are covered by engine helpers"
        );
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
        let result = crate::coverage_expect_result!(
            runtime::engine::run_engine(
                args,
                vec!["rm".to_string(), "-f".to_string(), container_id.to_string()],
            ),
            "feature-test cleanup process launch failures are covered by engine helpers"
        );
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
        let dockerfile_path = crate::coverage_expect_result!(
            write_feature_test_dockerfile(
                &prepared.build_context_dir,
                &base_image,
                &prepared.feature_installations,
            ),
            "feature-test Dockerfile materialization errors are covered by materialize tests"
        );
        let image_name = unique_feature_test_name("devcontainer-feature-test");
        crate::coverage_expect_result!(
            runtime.build_image(
                args,
                &image_name,
                &dockerfile_path,
                &prepared.build_context_dir,
            ),
            "feature-test runtime build errors are covered by runtime wrapper tests"
        );
        let container_id = runtime.start_container(args, &image_name, &prepared.workspace_dir)?;
        let status = crate::coverage_expect_result!(
            runtime.exec_script(
                args,
                &container_id,
                &prepared.workspace_dir,
                prepared.remote_user.as_deref(),
                &prepared.exec_env,
                &prepared.script_name,
            ),
            "feature-test script execution errors are covered by runtime wrapper tests"
        );
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

    use crate::test_support::{unique_temp_dir, write_executable_script};

    use super::{ContainerEngineFeatureTestRuntime, FeatureTestRuntime};

    #[test]
    fn container_engine_runtime_executes_engine_commands_and_reports_failures() {
        let root = unique_temp_dir("feature-test-runtime");
        fs::create_dir_all(&root).expect("root");
        let fake_engine = root.join("docker");
        let log = root.join("engine.log");
        write_executable_script(
            &fake_engine,
            &format!(
                r#"#!/bin/sh
set -eu
ROOT=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
printf '%s\n' "$*" >> '{}'
case "$1" in
  build)
    if [ -f "$ROOT/build-fails" ]; then
      echo "build failed" >&2
      exit 2
    fi
    exit 0
    ;;
  run)
    if [ -f "$ROOT/run-fails" ]; then
      echo "run failed" >&2
      exit 3
    fi
    printf 'container-123\n'
    exit 0
    ;;
  exec)
    exit 7
    ;;
  rm)
    if [ -f "$ROOT/rm-fails" ]; then
      echo "rm failed" >&2
      exit 4
    fi
    exit 0
    ;;
esac
exit 9
"#,
                log.display()
            ),
        );
        let args = vec![
            "--docker-path".to_string(),
            fake_engine.display().to_string(),
        ];
        let dockerfile = root.join("Dockerfile");
        fs::write(&dockerfile, "FROM alpine:3.20\n").expect("dockerfile");
        let workspace = root.join("workspace");
        fs::create_dir_all(&workspace).expect("workspace");
        let mut runtime = ContainerEngineFeatureTestRuntime;

        runtime
            .build_image(&args, "feature-test:image", &dockerfile, &root)
            .expect("build");
        assert_eq!(
            runtime
                .start_container(&args, "feature-test:image", &workspace)
                .expect("start"),
            "container-123"
        );
        assert_eq!(
            runtime
                .exec_script(
                    &args,
                    "container-123",
                    &workspace,
                    Some("vscode"),
                    &[("COLOR".to_string(), "blue".to_string())],
                    "test.sh",
                )
                .expect("exec status"),
            7
        );
        runtime
            .remove_container(&args, "container-123")
            .expect("remove");
        let invocations = fs::read_to_string(&log).expect("log");
        assert!(invocations.contains("build --tag feature-test:image --file"));
        assert!(invocations.contains("run -d --label devcontainer.is_test_run=true"));
        assert!(invocations.contains("exec --workdir /workspace --user vscode -e COLOR=blue"));
        assert!(invocations.contains("rm -f container-123"));

        fs::write(root.join("build-fails"), "").expect("build flag");
        let error = runtime
            .build_image(&args, "feature-test:image", &dockerfile, &root)
            .expect_err("build failure");
        assert!(error.contains("build failed"), "{error}");
        fs::remove_file(root.join("build-fails")).expect("clear build flag");

        fs::write(root.join("run-fails"), "").expect("run flag");
        let error = runtime
            .start_container(&args, "feature-test:image", &workspace)
            .expect_err("run failure");
        assert!(error.contains("run failed"), "{error}");
        fs::remove_file(root.join("run-fails")).expect("clear run flag");

        fs::write(root.join("rm-fails"), "").expect("rm flag");
        let error = runtime
            .remove_container(&args, "container-123")
            .expect_err("rm failure");
        assert!(error.contains("rm failed"), "{error}");
        let _ = fs::remove_dir_all(root);
    }
}
