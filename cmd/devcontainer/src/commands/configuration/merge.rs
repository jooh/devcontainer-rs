//! Native merge behavior for read-configuration output.

use std::collections::HashSet;

use serde_json::{Map, Value};

use crate::runtime;

pub(super) fn merge_configuration(configuration: &Value, metadata_entries: &[Value]) -> Value {
    let mut merged = configuration.as_object().cloned().unwrap_or_default();
    for key in [
        "customizations",
        "entrypoint",
        "onCreateCommand",
        "updateContentCommand",
        "postCreateCommand",
        "postStartCommand",
        "postAttachCommand",
        "shutdownAction",
    ] {
        merged.remove(key);
    }

    if metadata_entries
        .iter()
        .any(|entry| entry.get("init").and_then(Value::as_bool) == Some(true))
    {
        merged.insert("init".to_string(), Value::Bool(true));
    }
    if metadata_entries
        .iter()
        .any(|entry| entry.get("privileged").and_then(Value::as_bool) == Some(true))
    {
        merged.insert("privileged".to_string(), Value::Bool(true));
    }
    insert_merged_array(
        &mut merged,
        "capAdd",
        union_string_arrays(metadata_entries, "capAdd"),
    );
    insert_merged_array(
        &mut merged,
        "securityOpt",
        union_string_arrays(metadata_entries, "securityOpt"),
    );
    insert_merged_array(
        &mut merged,
        "entrypoints",
        collect_scalar_values(metadata_entries, "entrypoint"),
    );
    insert_merged_array(&mut merged, "mounts", merge_mounts(metadata_entries));
    insert_if_non_empty_object(
        &mut merged,
        "customizations",
        merge_customizations(metadata_entries),
    );
    insert_merged_array(
        &mut merged,
        "onCreateCommands",
        collect_command_values(metadata_entries, "onCreateCommand"),
    );
    insert_merged_array(
        &mut merged,
        "updateContentCommands",
        collect_command_values(metadata_entries, "updateContentCommand"),
    );
    insert_merged_array(
        &mut merged,
        "postCreateCommands",
        collect_command_values(metadata_entries, "postCreateCommand"),
    );
    insert_merged_array(
        &mut merged,
        "postStartCommands",
        collect_command_values(metadata_entries, "postStartCommand"),
    );
    insert_merged_array(
        &mut merged,
        "postAttachCommands",
        collect_command_values(metadata_entries, "postAttachCommand"),
    );
    insert_last_value(&mut merged, "workspaceFolder", metadata_entries);
    insert_last_value(&mut merged, "waitFor", metadata_entries);
    insert_last_value(&mut merged, "remoteUser", metadata_entries);
    insert_last_value(&mut merged, "containerUser", metadata_entries);
    insert_last_value(&mut merged, "userEnvProbe", metadata_entries);
    insert_if_non_empty_object(
        &mut merged,
        "remoteEnv",
        merge_object_entries(metadata_entries, "remoteEnv"),
    );
    insert_if_non_empty_object(
        &mut merged,
        "containerEnv",
        merge_object_entries(metadata_entries, "containerEnv"),
    );
    insert_last_value(&mut merged, "overrideCommand", metadata_entries);
    insert_if_non_empty_object(
        &mut merged,
        "portsAttributes",
        merge_object_entries(metadata_entries, "portsAttributes"),
    );
    insert_last_value(&mut merged, "otherPortsAttributes", metadata_entries);
    insert_merged_array(
        &mut merged,
        "forwardPorts",
        merge_forward_ports(metadata_entries),
    );
    insert_last_value(&mut merged, "shutdownAction", metadata_entries);
    insert_last_value(&mut merged, "updateRemoteUserUID", metadata_entries);
    if let Some(host_requirements) = merge_host_requirements(metadata_entries) {
        merged.insert("hostRequirements".to_string(), host_requirements);
    }

    Value::Object(merged)
}

fn insert_merged_array(merged: &mut Map<String, Value>, key: &str, values: Vec<Value>) {
    if !values.is_empty() {
        merged.insert(key.to_string(), Value::Array(values));
    }
}

fn insert_if_non_empty_object(merged: &mut Map<String, Value>, key: &str, value: Value) {
    if value.as_object().is_some_and(|entries| !entries.is_empty()) {
        merged.insert(key.to_string(), value);
    }
}

fn insert_last_value(merged: &mut Map<String, Value>, key: &str, entries: &[Value]) {
    if let Some(value) = entries
        .iter()
        .filter_map(|entry| entry.get(key))
        .next_back()
    {
        merged.insert(key.to_string(), value.clone());
    }
}

fn union_string_arrays(entries: &[Value], key: &str) -> Vec<Value> {
    let mut seen = HashSet::new();
    entries
        .iter()
        .filter_map(|entry| entry.get(key).and_then(Value::as_array))
        .flat_map(|values| values.iter())
        .filter_map(Value::as_str)
        .filter(|value| seen.insert((*value).to_string()))
        .map(|value| Value::String(value.to_string()))
        .collect()
}

fn collect_scalar_values(entries: &[Value], key: &str) -> Vec<Value> {
    entries
        .iter()
        .filter_map(|entry| entry.get(key))
        .filter(|value| value.is_string())
        .cloned()
        .collect()
}

fn collect_command_values(entries: &[Value], key: &str) -> Vec<Value> {
    entries
        .iter()
        .filter_map(|entry| entry.get(key))
        .cloned()
        .collect()
}

fn merge_customizations(entries: &[Value]) -> Value {
    let mut merged = Map::new();
    for entry in entries {
        let Some(customizations) = entry.get("customizations").and_then(Value::as_object) else {
            continue;
        };
        for (key, value) in customizations {
            merged
                .entry(key.clone())
                .or_insert_with(|| Value::Array(Vec::new()))
                .as_array_mut()
                .expect("customizations arrays")
                .push(value.clone());
        }
    }
    Value::Object(merged)
}

fn merge_object_entries(entries: &[Value], key: &str) -> Value {
    let mut merged = Map::new();
    for entry in entries {
        let Some(values) = entry.get(key).and_then(Value::as_object) else {
            continue;
        };
        merged.extend(
            values
                .iter()
                .map(|(name, value)| (name.clone(), value.clone())),
        );
    }
    Value::Object(merged)
}

fn merge_forward_ports(entries: &[Value]) -> Vec<Value> {
    let mut seen = HashSet::new();
    let mut merged = Vec::new();
    for entry in entries {
        let Some(values) = entry.get("forwardPorts").and_then(Value::as_array) else {
            continue;
        };
        for value in values {
            let Some(normalized) = normalize_forward_port(value) else {
                continue;
            };
            let fingerprint = serde_json::to_string(&normalized).unwrap_or_else(|_| String::new());
            if seen.insert(fingerprint) {
                merged.push(normalized);
            }
        }
    }
    merged
}

fn normalize_forward_port(value: &Value) -> Option<Value> {
    match value {
        Value::Number(number) => number
            .as_u64()
            .map(|port| Value::String(format!("localhost:{port}")))
            .and_then(|port| canonical_forward_port(&port)),
        Value::String(_) => canonical_forward_port(value),
        _ => None,
    }
}

fn canonical_forward_port(value: &Value) -> Option<Value> {
    match value {
        Value::String(text) => {
            if let Some(port) = text.strip_prefix("localhost:") {
                if port.chars().all(|character| character.is_ascii_digit()) {
                    return port
                        .parse::<u64>()
                        .ok()
                        .map(|port| Value::Number(port.into()));
                }
            }
            Some(Value::String(text.clone()))
        }
        _ => None,
    }
}

fn merge_host_requirements(entries: &[Value]) -> Option<Value> {
    let cpus = entries
        .iter()
        .filter_map(|entry| entry.get("hostRequirements"))
        .filter_map(|requirements| requirements.get("cpus"))
        .filter_map(Value::as_f64)
        .fold(0.0_f64, f64::max);
    let memory = entries
        .iter()
        .filter_map(|entry| entry.get("hostRequirements"))
        .filter_map(|requirements| requirements.get("memory"))
        .map(parse_host_requirement_bytes)
        .fold(0_u64, u64::max);
    let storage = entries
        .iter()
        .filter_map(|entry| entry.get("hostRequirements"))
        .filter_map(|requirements| requirements.get("storage"))
        .map(parse_host_requirement_bytes)
        .fold(0_u64, u64::max);
    let gpu = entries
        .iter()
        .filter_map(|entry| entry.get("hostRequirements"))
        .filter_map(|requirements| requirements.get("gpu"))
        .cloned()
        .reduce(|left, right| merge_gpu_requirement_values(&left, &right));

    if cpus == 0.0 && memory == 0 && storage == 0 && gpu.is_none() {
        return None;
    }

    let mut merged = Map::new();
    if cpus != 0.0 {
        let cpu_value = serde_json::Number::from_f64(cpus)
            .map(Value::Number)
            .unwrap_or_else(|| Value::from(cpus as i64));
        merged.insert("cpus".to_string(), cpu_value);
    }
    if memory != 0 {
        merged.insert("memory".to_string(), Value::String(memory.to_string()));
    }
    if storage != 0 {
        merged.insert("storage".to_string(), Value::String(storage.to_string()));
    }
    if let Some(gpu) = gpu {
        merged.insert("gpu".to_string(), gpu);
    }
    Some(Value::Object(merged))
}

fn parse_host_requirement_bytes(value: &Value) -> u64 {
    match value {
        Value::Number(number) => number.as_u64().unwrap_or(0),
        Value::String(text) => parse_byte_string(text),
        _ => 0,
    }
}

fn parse_byte_string(value: &str) -> u64 {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return 0;
    }
    let split_index = trimmed
        .find(|character: char| !(character.is_ascii_digit() || character == '.'))
        .unwrap_or(trimmed.len());
    let (number_text, unit_text) = trimmed.split_at(split_index);
    let Ok(number) = number_text.parse::<f64>() else {
        return 0;
    };
    let unit = unit_text.trim().to_ascii_lowercase();
    let multiplier = match unit.as_str() {
        "" | "b" => 1_f64,
        "k" | "kb" => 1_000_f64,
        "m" | "mb" => 1_000_000_f64,
        "g" | "gb" => 1_000_000_000_f64,
        "t" | "tb" => 1_000_000_000_000_f64,
        "ki" | "kib" => 1_024_f64,
        "mi" | "mib" => 1_048_576_f64,
        "gi" | "gib" => 1_073_741_824_f64,
        "ti" | "tib" => 1_099_511_627_776_f64,
        _ => return 0,
    };
    (number * multiplier) as u64
}

fn merge_gpu_requirement_values(left: &Value, right: &Value) -> Value {
    if matches!(left, Value::Bool(false) | Value::Null) {
        return right.clone();
    }
    if matches!(right, Value::Bool(false) | Value::Null) {
        return left.clone();
    }
    if left == &Value::String("optional".to_string())
        && right == &Value::String("optional".to_string())
    {
        return Value::String("optional".to_string());
    }

    let left_object = gpu_requirement_object(left);
    let right_object = gpu_requirement_object(right);
    let cores = left_object
        .get("cores")
        .and_then(Value::as_u64)
        .unwrap_or(0)
        .max(
            right_object
                .get("cores")
                .and_then(Value::as_u64)
                .unwrap_or(0),
        );
    let memory = parse_host_requirement_bytes(left_object.get("memory").unwrap_or(&Value::Null))
        .max(parse_host_requirement_bytes(
            right_object.get("memory").unwrap_or(&Value::Null),
        ));
    let mut merged = Map::new();
    if cores != 0 {
        merged.insert("cores".to_string(), Value::Number(cores.into()));
    }
    if memory != 0 {
        merged.insert("memory".to_string(), Value::String(memory.to_string()));
    }
    if merged.is_empty() {
        Value::Bool(true)
    } else {
        Value::Object(merged)
    }
}

fn gpu_requirement_object(value: &Value) -> Map<String, Value> {
    match value {
        Value::Object(entries) => entries.clone(),
        Value::Bool(true) => Map::new(),
        Value::String(text) if text == "optional" => Map::new(),
        _ => Map::new(),
    }
}

fn merge_mounts(entries: &[Value]) -> Vec<Value> {
    let mut seen = HashSet::new();
    let mut collected = Vec::new();
    let flattened = entries
        .iter()
        .filter_map(|entry| entry.get("mounts").and_then(Value::as_array))
        .flat_map(|values| values.iter().cloned())
        .collect::<Vec<_>>();
    for value in flattened.into_iter().rev() {
        let target = match &value {
            Value::String(text) => runtime::mounts::mount_option_target(text),
            Value::Object(entries) => entries
                .get("target")
                .and_then(Value::as_str)
                .map(str::to_string),
            _ => None,
        };
        if let Some(target) = target {
            if seen.insert(target) {
                collected.push(value);
            }
        } else {
            collected.push(value);
        }
    }
    collected.reverse();
    collected
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        canonical_forward_port, merge_configuration, merge_gpu_requirement_values,
        normalize_forward_port, parse_byte_string,
    };

    #[test]
    fn merge_configuration_covers_metadata_unions_and_normalization() {
        let merged = merge_configuration(
            &json!({
                "name": "demo",
                "customizations": {
                    "removed": true
                },
                "postCreateCommand": "removed"
            }),
            &[
                json!({
                    "init": true,
                    "privileged": true,
                    "capAdd": ["SYS_PTRACE", "SYS_PTRACE"],
                    "securityOpt": ["seccomp=unconfined"],
                    "entrypoint": "/entrypoint.sh",
                    "mounts": [
                        "source=old,target=/workspace,type=volume",
                        true
                    ],
                    "customizations": {
                        "vscode": {
                            "extensions": ["a.extension"]
                        }
                    },
                    "forwardPorts": [8080, "localhost:8080", false, "localhost:notaport"],
                    "hostRequirements": {
                        "cpus": 2,
                        "memory": "1GiB",
                        "storage": 500,
                        "gpu": false
                    }
                }),
                json!({
                    "mounts": [
                        {
                            "type": "bind",
                            "source": "/new",
                            "target": "/workspace"
                        },
                        {
                            "type": "volume"
                        }
                    ],
                    "remoteEnv": {
                        "DEMO": "1"
                    },
                    "containerEnv": {
                        "CONTAINER": "1"
                    },
                    "portsAttributes": {
                        "8080": {
                            "label": "web"
                        }
                    },
                    "otherPortsAttributes": {
                        "onAutoForward": "silent"
                    },
                    "forwardPorts": ["localhost:3000", "service:5000"],
                    "hostRequirements": {
                        "cpus": 4.5,
                        "memory": "2GB",
                        "storage": "1TB",
                        "gpu": {
                            "cores": 2,
                            "memory": "4GiB"
                        }
                    },
                    "shutdownAction": "stopContainer",
                    "updateRemoteUserUID": false
                }),
            ],
        );

        assert_eq!(merged["init"], true);
        assert_eq!(merged["privileged"], true);
        assert_eq!(merged["capAdd"], json!(["SYS_PTRACE"]));
        assert_eq!(merged["securityOpt"], json!(["seccomp=unconfined"]));
        assert_eq!(merged["entrypoints"], json!(["/entrypoint.sh"]));
        assert_eq!(
            merged["mounts"],
            json!([
                true,
                {
                    "type": "bind",
                    "source": "/new",
                    "target": "/workspace"
                },
                {
                    "type": "volume"
                }
            ])
        );
        assert_eq!(
            merged["forwardPorts"],
            json!([8080, "localhost:notaport", 3000, "service:5000"])
        );
        assert_eq!(merged["remoteEnv"]["DEMO"], "1");
        assert_eq!(merged["containerEnv"]["CONTAINER"], "1");
        assert_eq!(merged["portsAttributes"]["8080"]["label"], "web");
        assert_eq!(merged["otherPortsAttributes"]["onAutoForward"], "silent");
        assert_eq!(merged["hostRequirements"]["cpus"], 4.5);
        assert_eq!(merged["hostRequirements"]["memory"], "2000000000");
        assert_eq!(merged["hostRequirements"]["storage"], "1000000000000");
        assert_eq!(merged["hostRequirements"]["gpu"]["cores"], 2);
        assert_eq!(merged["hostRequirements"]["gpu"]["memory"], "4GiB");
        assert_eq!(merged["shutdownAction"], "stopContainer");
        assert_eq!(merged["updateRemoteUserUID"], false);
        assert!(merged.get("postCreateCommand").is_none());
    }

    #[test]
    fn byte_port_and_gpu_helpers_cover_edge_cases() {
        assert_eq!(parse_byte_string(""), 0);
        assert_eq!(parse_byte_string("bad"), 0);
        assert_eq!(parse_byte_string("1b"), 1);
        assert_eq!(parse_byte_string("1kb"), 1_000);
        assert_eq!(parse_byte_string("1mb"), 1_000_000);
        assert_eq!(parse_byte_string("1gb"), 1_000_000_000);
        assert_eq!(parse_byte_string("1tb"), 1_000_000_000_000);
        assert_eq!(parse_byte_string("1kib"), 1_024);
        assert_eq!(parse_byte_string("1mib"), 1_048_576);
        assert_eq!(parse_byte_string("1gib"), 1_073_741_824);
        assert_eq!(parse_byte_string("1tib"), 1_099_511_627_776);
        assert_eq!(parse_byte_string("1unknown"), 0);
        assert_eq!(normalize_forward_port(&json!(null)), None);
        assert_eq!(canonical_forward_port(&json!(123)), None);
        assert_eq!(
            merge_gpu_requirement_values(&json!(false), &json!("optional")),
            json!("optional")
        );
        assert_eq!(
            merge_gpu_requirement_values(&json!({"cores": 1}), &json!(null)),
            json!({"cores": 1})
        );
        assert_eq!(
            merge_gpu_requirement_values(&json!("optional"), &json!("optional")),
            json!("optional")
        );
        assert_eq!(
            merge_gpu_requirement_values(&json!(true), &json!(true)),
            json!(true)
        );
        assert_eq!(
            merge_gpu_requirement_values(
                &json!({
                    "cores": 1,
                    "memory": "1GiB"
                }),
                &json!({
                    "cores": 2,
                    "memory": "2GiB"
                })
            ),
            json!({
                "cores": 2,
                "memory": "2147483648"
            })
        );
    }
}
