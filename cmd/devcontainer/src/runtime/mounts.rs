//! Shared mount parsing and normalization helpers for runtime code.

use serde_json::{Map, Value};

use crate::commands::common;

pub(crate) fn mount_option_target(mount: &str) -> Option<String> {
    split_mount_options(mount).into_iter().find_map(|option| {
        for key in ["target", "destination", "dst"] {
            if let Some(value) = option.strip_prefix(&format!("{key}=")) {
                return Some(value.trim_matches('"').to_string());
            }
        }
        None
    })
}

pub(crate) fn split_mount_options(mount: &str) -> Vec<String> {
    let mut options = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    for character in mount.chars() {
        match character {
            '"' => {
                in_quotes = !in_quotes;
                current.push(character);
            }
            ',' if !in_quotes => {
                options.push(current.trim().to_string());
                current.clear();
            }
            _ => current.push(character),
        }
    }
    if !current.is_empty() {
        options.push(current.trim().to_string());
    }
    options
}

pub(crate) fn mount_value_to_engine_arg(value: &Value) -> Option<String> {
    match value {
        Value::String(mount) => Some(mount.clone()),
        Value::Object(entries) => mount_object_to_engine_arg(entries),
        _ => None,
    }
}

fn mount_object_to_engine_arg(entries: &Map<String, Value>) -> Option<String> {
    let mut options = Vec::new();
    if let Some(value) = entries.get("type").and_then(mount_option_value) {
        options.push(format!("type={value}"));
    }
    if let Some(value) = entries
        .get("source")
        .or_else(|| entries.get("src"))
        .and_then(mount_option_value)
    {
        options.push(format!("source={value}"));
    }
    if let Some(value) = entries
        .get("target")
        .or_else(|| entries.get("destination"))
        .or_else(|| entries.get("dst"))
        .and_then(mount_option_value)
    {
        options.push(format!("target={value}"));
    }
    if entries
        .get("readonly")
        .or_else(|| entries.get("readOnly"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        options.push("readonly".to_string());
    }
    for (key, value) in entries {
        if matches!(
            key.as_str(),
            "type" | "source" | "src" | "target" | "destination" | "dst" | "readonly" | "readOnly"
        ) {
            continue;
        }
        if let Some(value) = mount_option_value(value) {
            options.push(format!("{key}={value}"));
        }
    }
    if options.is_empty() {
        None
    } else {
        Some(options.join(","))
    }
}

pub(crate) fn cli_mount_values(args: &[String]) -> Result<Vec<String>, String> {
    common::validate_option_values(args, &["--mount"])?;
    let mounts = common::parse_option_values(args, "--mount");
    validate_cli_mount_values(&mounts)?;
    Ok(mounts)
}

pub(crate) fn validate_cli_mount_values(mounts: &[String]) -> Result<(), String> {
    for mount in mounts {
        validate_cli_mount_value(mount)?;
    }
    Ok(())
}

pub(crate) fn validate_cli_mount_value(mount: &str) -> Result<(), String> {
    let mut is_volume_mount = false;
    let mut has_mount_type = false;
    let mut has_source = false;
    let mut has_target = false;

    for option in split_mount_options(mount) {
        if matches!(option.as_str(), "readonly" | "ro") {
            continue;
        }

        let Some((key, value)) = option.split_once('=') else {
            return Err(invalid_cli_mount_error(mount));
        };
        let value = value.trim_matches('"');
        if key == "type" {
            if matches!(value, "bind" | "volume") {
                has_mount_type = true;
                is_volume_mount = value == "volume";
            } else {
                return Err(invalid_cli_mount_error(mount));
            }
        } else if matches!(key, "source" | "src") && !value.is_empty() {
            has_source = true;
        } else if matches!(key, "target" | "destination" | "dst") && !value.is_empty() {
            has_target = true;
        }
    }

    let requires_source = !is_volume_mount;

    if !has_mount_type || !has_target || (requires_source && !has_source) {
        return Err(invalid_cli_mount_error(mount));
    }

    Ok(())
}

fn mount_option_value(value: &Value) -> Option<String> {
    match value {
        Value::Bool(boolean) => Some(boolean.to_string()),
        Value::Number(number) => Some(number.to_string()),
        Value::String(text) => Some(text.clone()),
        _ => None,
    }
}

fn invalid_cli_mount_error(mount: &str) -> String {
    format!(
        "Invalid value for option --mount: {mount}. Expected type=<bind|volume>,target=<target>[,...], with source=<source> required for bind mounts"
    )
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        cli_mount_values, mount_option_target, mount_value_to_engine_arg, validate_cli_mount_value,
        validate_cli_mount_values,
    };

    #[test]
    fn mount_option_target_reads_quoted_targets() {
        assert_eq!(
            mount_option_target(r#"type=bind,source=/tmp/src,target="/workspace,with,comma""#),
            Some("/workspace,with,comma".to_string())
        );
    }

    #[test]
    fn mount_value_to_engine_arg_preserves_read_only_and_alias_keys() {
        assert_eq!(
            mount_value_to_engine_arg(&json!("type=bind,source=/src,target=/dst")),
            Some("type=bind,source=/src,target=/dst".to_string())
        );
        assert_eq!(mount_value_to_engine_arg(&json!(null)), None);

        let mount = mount_value_to_engine_arg(&json!({
            "type": "bind",
            "src": "/cache",
            "dst": "/workspace/cache",
            "readOnly": true,
        }))
        .expect("mount argument");

        assert_eq!(
            mount,
            "type=bind,source=/cache,target=/workspace/cache,readonly"
        );
    }

    #[test]
    fn mount_value_to_engine_arg_preserves_additional_scalar_options() {
        let mount = mount_value_to_engine_arg(&json!({
            "type": "volume",
            "source": "devcontainer-cache",
            "target": "/cache",
            "size": 10,
            "external": true,
            "ignored": ["not", "scalar"],
            "consistency": "delegated",
        }))
        .expect("mount argument");

        assert_eq!(
            mount,
            "type=volume,source=devcontainer-cache,target=/cache,consistency=delegated,external=true,size=10"
        );
        assert_eq!(mount_value_to_engine_arg(&json!({ "ignored": {} })), None);
    }

    #[test]
    fn validate_cli_mount_value_accepts_extended_scalar_options() {
        validate_cli_mount_value(
            "type=bind,source=/tmp/src,target=/tmp/dst,consistency=delegated,bind.propagation=rshared,readonly",
        )
        .expect("valid mount");
    }

    #[test]
    fn validate_cli_mount_value_accepts_aliases_and_quoted_values() {
        validate_cli_mount_value(
            r#"type="bind",src="/tmp/src",destination="/tmp/dst",volume-opt=keep,ro"#,
        )
        .expect("valid mount");
    }

    #[test]
    fn validate_cli_mount_value_accepts_anonymous_volume_mounts() {
        validate_cli_mount_value("type=volume,target=/cache").expect("valid mount");
    }

    #[test]
    fn validate_cli_mount_value_rejects_missing_required_keys() {
        let error =
            validate_cli_mount_value("type=bind,source=/tmp/src").expect_err("missing target");

        assert!(error.contains("Invalid value for option --mount"));
        assert!(validate_cli_mount_value("type=bind,source,target=/tmp/dst").is_err());
        assert!(validate_cli_mount_value("type=tmpfs,target=/tmp/dst").is_err());
    }

    #[test]
    fn cli_mount_values_require_option_values() {
        let error = cli_mount_values(&["--mount".to_string()]).expect_err("missing mount value");

        assert_eq!(error, "Missing value for option: --mount");
    }

    #[test]
    fn cli_mount_values_returns_valid_mount_values() {
        let args = vec![
            "--mount".to_string(),
            "type=bind,source=/tmp/src,target=/tmp/dst".to_string(),
            "--mount".to_string(),
            "type=volume,target=/cache".to_string(),
        ];

        assert_eq!(
            cli_mount_values(&args).expect("valid mount values"),
            vec![
                "type=bind,source=/tmp/src,target=/tmp/dst".to_string(),
                "type=volume,target=/cache".to_string(),
            ]
        );
    }

    #[test]
    fn validate_cli_mount_values_checks_each_mount() {
        validate_cli_mount_values(&[
            "type=bind,source=/tmp/src,target=/tmp/dst".to_string(),
            "type=volume,target=/cache".to_string(),
        ])
        .expect("valid mount list");
    }
}
