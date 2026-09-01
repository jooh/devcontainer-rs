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

pub(crate) fn mount_args_for_engine(mount: &str, is_wslc: bool) -> Result<Vec<String>, String> {
    if !is_wslc {
        return Ok(vec!["--mount".to_string(), mount.to_string()]);
    }

    Ok(vec!["-v".to_string(), mount_to_wslc_volume_arg(mount)?])
}

fn mount_to_wslc_volume_arg(mount: &str) -> Result<String, String> {
    if mount.trim().is_empty() || mount.trim_end().ends_with(',') {
        return Err(wslc_mount_error(mount, "contains an empty option"));
    }
    let mut mount_type = None;
    let mut source = None;
    let mut target = None;
    let mut read_only = None;
    let mut consistency = None;
    let mut propagation = None;
    let mut no_copy = None;

    for option in split_mount_options(mount) {
        if option.is_empty() {
            return Err(wslc_mount_error(mount, "contains an empty option"));
        }
        match option.as_str() {
            "readonly" | "ro" => {
                set_wslc_mount_bool(&mut read_only, true, mount, &option)?;
                continue;
            }
            "volume-nocopy" | "nocopy" => {
                set_wslc_mount_bool(&mut no_copy, true, mount, &option)?;
                continue;
            }
            _ => {}
        }

        let Some((key, raw_value)) = option.split_once('=') else {
            return Err(wslc_mount_error(
                mount,
                &format!("option {option:?} is not supported by -v"),
            ));
        };
        let value = wslc_mount_option_value(mount, key, raw_value)?;
        match key {
            "type" => set_wslc_mount_field(&mut mount_type, value, mount, key)?,
            "source" | "src" => set_wslc_mount_field(&mut source, value, mount, key)?,
            "target" | "destination" | "dst" => {
                set_wslc_mount_field(&mut target, value, mount, key)?;
            }
            "readonly" | "ro" => set_wslc_mount_bool(
                &mut read_only,
                wslc_mount_bool_value(mount, key, &value)?,
                mount,
                key,
            )?,
            "consistency" => {
                if !matches!(value.as_str(), "consistent" | "cached" | "delegated") {
                    return Err(wslc_mount_error(
                        mount,
                        &format!("consistency value {value:?} is not supported by -v"),
                    ));
                }
                set_wslc_mount_field(&mut consistency, value, mount, key)?;
            }
            "bind-propagation" | "bind.propagation" => {
                if !matches!(
                    value.as_str(),
                    "private" | "rprivate" | "shared" | "rshared" | "slave" | "rslave"
                ) {
                    return Err(wslc_mount_error(
                        mount,
                        &format!("bind propagation value {value:?} is not supported by -v"),
                    ));
                }
                set_wslc_mount_field(&mut propagation, value, mount, key)?;
            }
            "volume-nocopy" | "nocopy" => set_wslc_mount_bool(
                &mut no_copy,
                wslc_mount_bool_value(mount, key, &value)?,
                mount,
                key,
            )?,
            _ => {
                return Err(wslc_mount_error(
                    mount,
                    &format!("option {key:?} is not supported by -v"),
                ));
            }
        }
    }

    let mount_type =
        mount_type.ok_or_else(|| wslc_mount_error(mount, "is missing the mount type"))?;
    if !matches!(mount_type.as_str(), "bind" | "volume") {
        return Err(wslc_mount_error(
            mount,
            &format!("mount type {mount_type:?} is not supported by -v"),
        ));
    }
    let target = target.ok_or_else(|| wslc_mount_error(mount, "is missing the target"))?;
    if target.contains(':') {
        return Err(wslc_mount_error(
            mount,
            "target paths containing ':' are ambiguous in -v syntax",
        ));
    }

    if mount_type == "bind" {
        let source = source
            .as_deref()
            .ok_or_else(|| wslc_mount_error(mount, "bind mounts require a source"))?;
        if !is_absolute_bind_source(source) || !has_representable_bind_source_colons(source) {
            return Err(wslc_mount_error(
                mount,
                "bind mount sources must be unambiguous absolute paths for -v",
            ));
        }
    } else {
        if let Some(source) = source.as_deref() {
            if source.contains('/') || source.contains('\\') || source.contains(':') {
                return Err(wslc_mount_error(
                    mount,
                    "volume sources that look like paths would become bind mounts with -v",
                ));
            }
        }
        if propagation.is_some() {
            return Err(wslc_mount_error(
                mount,
                "bind propagation is only valid for bind mounts",
            ));
        }
    }
    if no_copy.unwrap_or(false) && mount_type != "volume" {
        return Err(wslc_mount_error(
            mount,
            "volume-nocopy is only valid for volume mounts",
        ));
    }

    let mut volume_options = Vec::new();
    if read_only.unwrap_or(false) {
        volume_options.push("ro".to_string());
    }
    if let Some(consistency) = consistency {
        volume_options.push(consistency);
    }
    if let Some(propagation) = propagation {
        volume_options.push(propagation);
    }
    if no_copy.unwrap_or(false) {
        volume_options.push("nocopy".to_string());
    }
    let mut volume = source
        .map(|source| format!("{source}:{target}"))
        .unwrap_or(target);
    if !volume_options.is_empty() {
        volume.push(':');
        volume.push_str(&volume_options.join(","));
    }
    Ok(volume)
}

fn set_wslc_mount_field(
    field: &mut Option<String>,
    value: String,
    mount: &str,
    key: &str,
) -> Result<(), String> {
    match field.as_ref() {
        Some(previous) if previous != &value => Err(wslc_mount_error(
            mount,
            &format!("conflicting values were provided for {key:?}"),
        )),
        Some(_) => Ok(()),
        None => {
            *field = Some(value);
            Ok(())
        }
    }
}

fn set_wslc_mount_bool(
    field: &mut Option<bool>,
    value: bool,
    mount: &str,
    key: &str,
) -> Result<(), String> {
    match *field {
        Some(previous) if previous != value => Err(wslc_mount_error(
            mount,
            &format!("conflicting values were provided for {key:?}"),
        )),
        Some(_) => Ok(()),
        None => {
            *field = Some(value);
            Ok(())
        }
    }
}

fn wslc_mount_option_value(mount: &str, key: &str, raw_value: &str) -> Result<String, String> {
    let value = raw_value.trim();
    let starts_quoted = value.starts_with('"');
    let ends_quoted = value.ends_with('"');
    if starts_quoted != ends_quoted || (starts_quoted && value.len() < 2) {
        return Err(wslc_mount_error(
            mount,
            &format!("option {key:?} contains unmatched quotes"),
        ));
    }
    let value = if starts_quoted {
        &value[1..value.len() - 1]
    } else {
        value
    };
    if value.is_empty() || value.contains('"') {
        return Err(wslc_mount_error(
            mount,
            &format!("option {key:?} has an invalid value"),
        ));
    }
    Ok(value.to_string())
}

fn wslc_mount_bool_value(mount: &str, key: &str, value: &str) -> Result<bool, String> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(wslc_mount_error(
            mount,
            &format!("option {key:?} requires true or false"),
        )),
    }
}

fn is_absolute_bind_source(source: &str) -> bool {
    source.starts_with('/')
        || source.starts_with("\\\\")
        || (source.len() >= 3
            && source.as_bytes()[0].is_ascii_alphabetic()
            && source.as_bytes()[1] == b':'
            && matches!(source.as_bytes()[2], b'/' | b'\\'))
}

fn has_representable_bind_source_colons(source: &str) -> bool {
    let mut colon_positions = source
        .bytes()
        .enumerate()
        .filter_map(|(index, byte)| (byte == b':').then_some(index));
    match (colon_positions.next(), colon_positions.next()) {
        (None, None) => true,
        (Some(1), None) => {
            source.as_bytes()[0].is_ascii_alphabetic()
                && source
                    .as_bytes()
                    .get(2)
                    .is_some_and(|separator| matches!(*separator, b'/' | b'\\'))
        }
        _ => false,
    }
}

fn wslc_mount_error(mount: &str, reason: &str) -> String {
    format!("WSLc cannot represent mount with -v: {reason}: {mount}")
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
        cli_mount_values, mount_args_for_engine, mount_option_target, mount_value_to_engine_arg,
        validate_cli_mount_value, validate_cli_mount_values,
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
    fn wslc_mount_arguments_preserve_aliases_read_only_and_supported_options() {
        assert_eq!(
            mount_args_for_engine(
                r#"type=bind,src="/src,with,commas",destination=/dst,ro,consistency=delegated,bind.propagation=rshared"#,
                true,
            ),
            Ok(vec![
                "-v".to_string(),
                "/src,with,commas:/dst:ro,delegated,rshared".to_string(),
            ])
        );
        assert_eq!(
            mount_args_for_engine(
                "type=volume,source=cache,dst=/cache,readonly=true,volume-nocopy",
                true,
            ),
            Ok(vec!["-v".to_string(), "cache:/cache:ro,nocopy".to_string(),])
        );
    }

    #[test]
    fn wslc_mount_arguments_keep_read_write_defaults_and_anonymous_volumes() {
        assert_eq!(
            mount_args_for_engine("type=bind,source=C:\\src,target=/dst,readonly=false", true,),
            Ok(vec!["-v".to_string(), "C:\\src:/dst".to_string()])
        );
        assert_eq!(
            mount_args_for_engine("type=volume,target=/cache,readonly", true),
            Ok(vec!["-v".to_string(), "/cache:ro".to_string()])
        );
        assert_eq!(
            mount_args_for_engine(
                "type=volume,source=cache,target=/cache,ro=true,nocopy=false",
                true,
            ),
            Ok(vec!["-v".to_string(), "cache:/cache:ro".to_string()])
        );
        assert_eq!(
            mount_args_for_engine(
                "type=bind,type=bind,source=/src,src=/src,target=/dst,dst=/dst,readonly,ro=true",
                true,
            ),
            Ok(vec!["-v".to_string(), "/src:/dst:ro".to_string()])
        );
    }

    #[test]
    fn wslc_mount_arguments_reject_semantics_that_volume_syntax_cannot_preserve() {
        for mount in [
            "type=bind,source=/src,target=/dst,bind-nonrecursive",
            "type=volume,source=cache,target=/dst,volume-opt=o=uid=1000",
            "type=volume,source=/host/path,target=/dst",
            "type=bind,source=relative,target=/dst",
            "type=bind,source=/src:alternate,target=/dst",
            "type=bind,source=/src,target=/dst:alternate",
            "type=bind,source=/src,target=/dst,",
        ] {
            let error = mount_args_for_engine(mount, true).expect_err("unrepresentable mount");
            assert!(
                error.contains("WSLc cannot represent mount with -v"),
                "{mount}: {error}"
            );
            assert!(!error.contains("--mount"), "{mount}: {error}");
        }
    }

    #[test]
    fn wslc_mount_arguments_report_each_invalid_option_kind() {
        for (mount, reason) in [
            (
                "type=bind,,source=/src,target=/dst",
                "contains an empty option",
            ),
            (
                "type=bind,source=/src,target=/dst,consistency=eventual",
                "consistency value \"eventual\" is not supported by -v",
            ),
            (
                "type=bind,source=/src,target=/dst,bind-propagation=recursive",
                "bind propagation value \"recursive\" is not supported by -v",
            ),
            (
                "type=tmpfs,target=/dst",
                "mount type \"tmpfs\" is not supported by -v",
            ),
            (
                "type=volume,target=/dst,bind-propagation=rshared",
                "bind propagation is only valid for bind mounts",
            ),
            (
                "type=bind,source=/src,target=/dst,volume-nocopy",
                "volume-nocopy is only valid for volume mounts",
            ),
            (
                "type=bind,type=volume,source=/src,target=/dst",
                "conflicting values were provided for \"type\"",
            ),
            (
                "type=volume,target=/dst,readonly,readonly=false",
                "conflicting values were provided for \"readonly\"",
            ),
            (
                "type=volume,target=/dst,volume-nocopy,nocopy=false",
                "conflicting values were provided for \"nocopy\"",
            ),
            (
                "type=bind,source=\"/src,target=/dst",
                "option \"source\" contains unmatched quotes",
            ),
            (
                "type=bind,source=,target=/dst",
                "option \"source\" has an invalid value",
            ),
            (
                r#"type=bind,source=/sr"c"d,target=/dst"#,
                "option \"source\" has an invalid value",
            ),
            (
                "type=volume,target=/dst,volume-nocopy=maybe",
                "option \"volume-nocopy\" requires true or false",
            ),
        ] {
            let error = mount_args_for_engine(mount, true).expect_err("invalid WSLc mount");

            assert!(error.contains(reason), "{mount}: {error}");
        }
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
