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

    use serde_json::json;

    use super::super::{FeatureTestCase, FeatureTestExecution, FeatureTestOptions};
    use super::{
        execute_feature_tests_with_runtime, ContainerEngineFeatureTestRuntime, FeatureTestRuntime,
    };

    #[derive(Default)]
    struct FailingBaseBuildRuntime {
        context_paths: Vec<PathBuf>,
    }

    #[derive(Default)]
    struct RecordingRuntime {
        start_workspace: Option<PathBuf>,
        removed_containers: Vec<String>,
    }

    impl FeatureTestRuntime for FailingBaseBuildRuntime {
        fn build_image(
            &mut self,
            _args: &[String],
            _image_name: &str,
            _dockerfile_path: &Path,
            context_path: &Path,
        ) -> Result<(), String> {
            self.context_paths.push(context_path.to_path_buf());
            Err("base build failed".to_string())
        }

        fn start_container(
            &mut self,
            _args: &[String],
            _image_name: &str,
            _workspace_dir: &Path,
        ) -> Result<String, String> {
            panic!("container should not start")
        }

        fn exec_script(
            &mut self,
            _args: &[String],
            _container_id: &str,
            _workspace_dir: &Path,
            _remote_user: Option<&str>,
            _env: &[(String, String)],
            _script_name: &str,
        ) -> Result<i32, String> {
            panic!("script should not execute")
        }

        fn remove_container(
            &mut self,
            _args: &[String],
            _container_id: &str,
        ) -> Result<(), String> {
            panic!("container should not be removed")
        }
    }

    impl FeatureTestRuntime for RecordingRuntime {
        fn build_image(
            &mut self,
            _args: &[String],
            _image_name: &str,
            _dockerfile_path: &Path,
            _context_path: &Path,
        ) -> Result<(), String> {
            Ok(())
        }

        fn start_container(
            &mut self,
            _args: &[String],
            _image_name: &str,
            workspace_dir: &Path,
        ) -> Result<String, String> {
            self.start_workspace = Some(workspace_dir.to_path_buf());
            Ok("container-from-recording-runtime".to_string())
        }

        fn exec_script(
            &mut self,
            _args: &[String],
            _container_id: &str,
            workspace_dir: &Path,
            _remote_user: Option<&str>,
            _env: &[(String, String)],
            script_name: &str,
        ) -> Result<i32, String> {
            assert!(workspace_dir.join(script_name).is_file());
            Ok(0)
        }

        fn remove_container(&mut self, _args: &[String], container_id: &str) -> Result<(), String> {
            self.removed_containers.push(container_id.to_string());
            Ok(())
        }
    }

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

    fn write_feature_manifest(feature_dir: &Path) {
        fs::create_dir_all(feature_dir).expect("feature dir");
        fs::write(
            feature_dir.join("devcontainer-feature.json"),
            r#"{
  "id": "demo",
  "version": "1.0.0"
}"#,
        )
        .expect("manifest");
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

    #[test]
    fn execute_feature_tests_with_runtime_reports_scenario_base_build_failures() {
        let root = crate::test_support::unique_temp_dir("feature-test-runtime");
        let feature_dir = root.join("src").join("demo");
        let test_dir = root.join("test").join("demo");
        let scenario_dir = test_dir.join("custom");
        write_feature_manifest(&feature_dir);
        fs::create_dir_all(&scenario_dir).expect("scenario dir");
        fs::write(test_dir.join("custom.sh"), "#!/bin/sh\n").expect("scenario script");
        fs::write(scenario_dir.join("Dockerfile.base"), "FROM scratch\n").expect("dockerfile");
        let options = FeatureTestOptions {
            project_folder: root.clone(),
            base_image: "debian:bookworm-slim".to_string(),
            remote_user: None,
            preserve_test_containers: true,
            permit_randomization: false,
            quiet: true,
        };
        let case = FeatureTestCase {
            name: "custom".to_string(),
            script_path: test_dir.join("custom.sh"),
            execution: FeatureTestExecution::Scenario {
                scenario_dir: "custom".to_string(),
                config: json!({
                    "build": {
                        "dockerfile": "Dockerfile.base",
                        "context": "."
                    }
                }),
            },
        };
        let mut runtime = FailingBaseBuildRuntime::default();

        let error = execute_feature_tests_with_runtime(&[], &mut runtime, &options, vec![case])
            .expect_err("base build should fail");

        assert_eq!(error, "base build failed");
        for context_path in &runtime.context_paths {
            if let Some(workspace_dir) = context_path.parent() {
                let _ = fs::remove_dir_all(workspace_dir);
            }
        }
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn execute_feature_tests_with_runtime_removes_container_and_workspace_by_default() {
        let root = crate::test_support::unique_temp_dir("feature-test-runtime");
        let feature_dir = root.join("src").join("demo");
        let test_dir = root.join("test").join("demo");
        write_feature_manifest(&feature_dir);
        fs::write(feature_dir.join("install.sh"), "#!/bin/sh\n").expect("install");
        fs::create_dir_all(&test_dir).expect("test dir");
        fs::write(test_dir.join("test.sh"), "#!/bin/sh\n").expect("test script");
        let options = FeatureTestOptions {
            project_folder: root.clone(),
            base_image: "debian:bookworm-slim".to_string(),
            remote_user: None,
            preserve_test_containers: false,
            permit_randomization: false,
            quiet: true,
        };
        let case = FeatureTestCase {
            name: "demo".to_string(),
            script_path: test_dir.join("test.sh"),
            execution: FeatureTestExecution::Autogenerated {
                feature: "demo".to_string(),
            },
        };
        let mut runtime = RecordingRuntime::default();

        let results = execute_feature_tests_with_runtime(&[], &mut runtime, &options, vec![case])
            .expect("feature test execution");

        assert_eq!(results.len(), 1);
        assert!(results[0].passed);
        assert_eq!(
            runtime.removed_containers,
            vec!["container-from-recording-runtime".to_string()]
        );
        let workspace = runtime.start_workspace.expect("workspace captured");
        assert!(!workspace.exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn failing_base_build_runtime_panics_if_execution_continues_after_base_build() {
        assert!(std::panic::catch_unwind(|| {
            let mut runtime = FailingBaseBuildRuntime::default();
            let _ = runtime.start_container(&[], "image", Path::new("."));
        })
        .is_err());
        assert!(std::panic::catch_unwind(|| {
            let mut runtime = FailingBaseBuildRuntime::default();
            let _ = runtime.exec_script(&[], "container", Path::new("."), None, &[], "test.sh");
        })
        .is_err());
        assert!(std::panic::catch_unwind(|| {
            let mut runtime = FailingBaseBuildRuntime::default();
            let _ = runtime.remove_container(&[], "container");
        })
        .is_err());
    }
}
