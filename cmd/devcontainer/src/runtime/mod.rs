//! Public runtime command orchestration for build, up, exec, and lifecycle flows.

mod build;
pub(crate) mod compose;
mod container;
pub(crate) mod context;
mod dockerfile;
pub(crate) mod engine;
mod exec;
mod image;
mod lifecycle;
pub(crate) mod metadata;
pub(crate) mod mounts;
mod paths;
mod user_resolution;

use serde_json::{json, Value};

use crate::commands::common;
use crate::commands::configuration;

fn effective_up_resolved_config(
    args: &[String],
    resolved: context::ResolvedConfig,
) -> Result<context::ResolvedConfig, String> {
    let feature_support = configuration::resolve_feature_support(
        args,
        &resolved.workspace_folder,
        &resolved.config_file,
        &resolved.configuration,
    )?;
    let effective_configuration = feature_support
        .as_ref()
        .map(|resolved_features| {
            configuration::apply_feature_metadata(
                &resolved.configuration,
                &resolved_features.metadata_entries,
            )
        })
        .unwrap_or_else(|| resolved.configuration.clone());
    Ok(context::ResolvedConfig {
        workspace_folder: resolved.workspace_folder,
        config_file: resolved.config_file,
        configuration: effective_configuration,
    })
}

pub fn run_build(args: &[String]) -> Result<Value, String> {
    let resolved = context::load_required_config(args)?;
    let feature_support = configuration::resolve_feature_support(
        args,
        &resolved.workspace_folder,
        &resolved.config_file,
        &resolved.configuration,
    )?;
    let skip_feature_customizations =
        common::runtime_options(args).skip_persisting_customizations_from_features;
    let effective_configuration = feature_support
        .as_ref()
        .map(|resolved_features| {
            configuration::apply_feature_metadata_with_options(
                &resolved.configuration,
                &resolved_features.metadata_entries,
                skip_feature_customizations,
            )
        })
        .unwrap_or_else(|| resolved.configuration.clone());
    let image_name = build::build_image(&resolved, args)?;

    Ok(json!({
        "outcome": "success",
        "command": "build",
        "workspaceFolder": resolved.workspace_folder,
        "configFile": resolved.config_file,
        "imageName": image_name,
        "configuration": effective_configuration,
    }))
}

pub fn run_up(args: &[String]) -> Result<Value, String> {
    let _ = mounts::cli_mount_values(args)?;
    let effective_resolved =
        effective_up_resolved_config(args, context::load_required_config(args)?)?;
    let effective_resolved =
        match container::probe_up_container_id_labels(&effective_resolved, args)? {
            Some(id_labels) => effective_up_resolved_config(
                args,
                context::load_required_config_with_id_labels(args, id_labels)?,
            )?,
            None => effective_resolved,
        };
    lifecycle::run_initialize_command(
        args,
        &effective_resolved.configuration,
        &effective_resolved.workspace_folder,
    )?;
    let compose_project_name =
        compose::load_compose_spec(&effective_resolved)?.map(|spec| spec.project_name);
    let image_name = build::runtime_image_name(&effective_resolved, args)?;
    let image_name = container::prepare_up_image(&effective_resolved, args, &image_name)?;
    let remote_workspace_folder =
        context::remote_workspace_folder_for_args(&effective_resolved, args);
    let up_container = container::ensure_up_container(
        &effective_resolved,
        args,
        &image_name,
        &remote_workspace_folder,
    )?;
    let lifecycle_resolved = match up_container.matched_id_labels.clone() {
        Some(id_labels) => effective_up_resolved_config(
            args,
            context::load_required_config_with_id_labels(args, id_labels)?,
        )?,
        None => effective_resolved,
    };
    let remote_workspace_folder =
        context::remote_workspace_folder_for_args(&lifecycle_resolved, args);
    lifecycle::run_lifecycle_commands(
        &up_container.container_id,
        args,
        &lifecycle_resolved.configuration,
        &remote_workspace_folder,
        up_container.lifecycle_mode,
    )?;

    Ok(json!({
        "outcome": "success",
        "command": "up",
        "containerId": up_container.container_id,
        "composeProjectName": compose_project_name,
        "remoteUser": context::remote_user(&lifecycle_resolved.configuration),
        "remoteWorkspaceFolder": remote_workspace_folder,
        "configuration": if common::has_flag(args, "--include-configuration") { lifecycle_resolved.configuration.clone() } else { Value::Null },
        "mergedConfiguration": if common::has_flag(args, "--include-merged-configuration") { lifecycle_resolved.configuration.clone() } else { Value::Null },
        "workspaceFolder": lifecycle_resolved.workspace_folder,
        "configFile": lifecycle_resolved.config_file,
    }))
}

pub fn run_set_up(args: &[String]) -> Result<Value, String> {
    let context = context::resolve_existing_container_context(args)?;
    lifecycle::run_lifecycle_commands(
        &context.container_id,
        args,
        &context.configuration,
        &context.remote_workspace_folder,
        lifecycle::LifecycleMode::SetUp,
    )?;

    Ok(json!({
        "outcome": "success",
        "command": "set-up",
        "containerId": context.container_id,
        "configuration": if common::has_flag(args, "--include-configuration") { context.configuration.clone() } else { Value::Null },
        "mergedConfiguration": if common::has_flag(args, "--include-merged-configuration") { context.configuration } else { Value::Null },
        "remoteWorkspaceFolder": context.remote_workspace_folder,
    }))
}

pub fn run_user_commands(args: &[String]) -> Result<Value, String> {
    let context = context::resolve_existing_container_context(args)?;
    lifecycle::run_lifecycle_commands(
        &context.container_id,
        args,
        &context.configuration,
        &context.remote_workspace_folder,
        lifecycle::LifecycleMode::RunUserCommands,
    )?;

    Ok(json!({
        "outcome": "success",
        "command": "run-user-commands",
        "containerId": context.container_id,
        "remoteWorkspaceFolder": context.remote_workspace_folder,
    }))
}

pub fn run_exec(args: &[String]) -> Result<i32, String> {
    let command_args = exec::exec_command_and_args(args)?;
    let context = context::resolve_existing_container_context(args)?;
    let engine_args = exec::exec_engine_args(
        args,
        &context.configuration,
        &context.remote_workspace_folder,
        &context.container_id,
        command_args,
        exec::ExecStdio::current(),
    )?;

    engine::run_engine_streaming(args, engine_args)
}
