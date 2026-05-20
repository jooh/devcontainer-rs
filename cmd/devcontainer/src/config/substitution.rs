//! Variable substitution helpers for local and container config contexts.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::Value;
use sha2::{Digest, Sha256};

#[derive(Clone)]
pub struct ConfigContext {
    pub workspace_folder: PathBuf,
    pub env: HashMap<String, String>,
    pub container_workspace_folder: Option<String>,
    pub id_labels: HashMap<String, String>,
}

fn parse_variable(input: &str) -> (&str, Vec<&str>) {
    let parts: Vec<&str> = input.split(':').collect();
    if parts.len() > 1 {
        (parts[0], parts[1..].to_vec())
    } else {
        (input, Vec::new())
    }
}

fn replace_variable(
    variable: &str,
    context: Option<&ConfigContext>,
    container_env: Option<&HashMap<String, String>>,
) -> Option<String> {
    let (name, args) = parse_variable(variable);
    match name {
        "localWorkspaceFolder" => {
            context.map(|value| value.workspace_folder.to_string_lossy().into_owned())
        }
        "localWorkspaceFolderBasename" => context.map(|value| {
            value
                .workspace_folder
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_string)
                .unwrap_or_else(|| value.workspace_folder.to_string_lossy().into_owned())
        }),
        "containerWorkspaceFolder" => {
            context.and_then(|value| value.container_workspace_folder.clone())
        }
        "containerWorkspaceFolderBasename" => context.and_then(|value| {
            value
                .container_workspace_folder
                .as_deref()
                .and_then(|folder| Path::new(folder).file_name())
                .and_then(|name| name.to_str())
                .map(str::to_string)
        }),
        "devcontainerId" => context
            .filter(|value| !value.id_labels.is_empty())
            .map(|value| devcontainer_id_for_labels(&value.id_labels)),
        "env" | "localEnv" => context.and_then(|value| {
            args.first().map(|variable_name| {
                value
                    .env
                    .get(*variable_name)
                    .cloned()
                    .or_else(|| args.get(1).map(|default| (*default).to_string()))
                    .unwrap_or_default()
            })
        }),
        "containerEnv" => container_env.and_then(|value| {
            args.first().map(|variable_name| {
                value
                    .get(*variable_name)
                    .cloned()
                    .or_else(|| args.get(1).map(|default| (*default).to_string()))
                    .unwrap_or_default()
            })
        }),
        _ => None,
    }
}

fn substitute_string(
    input: &str,
    context: Option<&ConfigContext>,
    container_env: Option<&HashMap<String, String>>,
) -> String {
    let mut output = String::with_capacity(input.len());
    let mut remaining = input;

    while let Some(start) = remaining.find("${") {
        output.push_str(&remaining[..start]);

        let variable_start = start + 2;
        let after_start = &remaining[variable_start..];
        let Some(end_offset) = after_start.find('}') else {
            output.push_str(&remaining[start..]);
            return output;
        };

        let variable = &after_start[..end_offset];
        if let Some(replacement) = replace_variable(variable, context, container_env) {
            output.push_str(&replacement);
        } else {
            output.push_str("${");
            output.push_str(variable);
            output.push('}');
        }

        remaining = &after_start[end_offset + 1..];
    }

    output.push_str(remaining);
    output
}

fn devcontainer_id_for_labels(labels: &HashMap<String, String>) -> String {
    let mut entries = labels.iter().collect::<Vec<_>>();
    entries.sort_by(|left, right| left.0.cmp(right.0));
    let serialized = format!(
        "{{{}}}",
        entries
            .into_iter()
            .map(|(key, value)| {
                format!(
                    "{}:{}",
                    serde_json::to_string(key).unwrap_or_default(),
                    serde_json::to_string(value).unwrap_or_default()
                )
            })
            .collect::<Vec<_>>()
            .join(",")
    );
    let hash = Sha256::digest(serialized.as_bytes());
    encode_base32hex_lower(&hash)
}

fn encode_base32hex_lower(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 32] = b"0123456789abcdefghijklmnopqrstuv";

    let mut output = String::new();
    let mut buffer = 0_u16;
    let mut bits = 0_u8;
    for byte in bytes {
        buffer = (buffer << 8) | u16::from(*byte);
        bits += 8;
        while bits >= 5 {
            let index = ((buffer >> (bits - 5)) & 0x1f) as usize;
            output.push(ALPHABET[index] as char);
            bits -= 5;
            if bits == 0 {
                buffer = 0;
            } else {
                buffer &= (1 << bits) - 1;
            }
        }
    }
    if bits > 0 {
        let index = ((buffer << (5 - bits)) & 0x1f) as usize;
        output.push(ALPHABET[index] as char);
    }
    while output.len() < 52 {
        output.insert(0, '0');
    }
    output
}

pub fn substitute_local_context(value: &Value, context: &ConfigContext) -> Value {
    let mut resolved_context = context.clone();
    if let Some(container_workspace_folder) = context.container_workspace_folder.as_deref() {
        resolved_context.container_workspace_folder = Some(substitute_string(
            container_workspace_folder,
            Some(&ConfigContext {
                workspace_folder: context.workspace_folder.clone(),
                env: context.env.clone(),
                container_workspace_folder: None,
                id_labels: context.id_labels.clone(),
            }),
            None,
        ));
    }
    substitute_local_context_resolved(value, &resolved_context)
}

fn substitute_local_context_resolved(value: &Value, context: &ConfigContext) -> Value {
    match value {
        Value::String(text) => Value::String(substitute_string(text, Some(context), None)),
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| substitute_local_context_resolved(item, context))
                .collect(),
        ),
        Value::Object(entries) => Value::Object(
            entries
                .iter()
                .map(|(key, value)| {
                    (
                        key.clone(),
                        substitute_local_context_resolved(value, context),
                    )
                })
                .collect(),
        ),
        _ => value.clone(),
    }
}

pub fn substitute_container_env(value: &Value, env: &HashMap<String, String>) -> Value {
    match value {
        Value::String(text) => Value::String(substitute_string(text, None, Some(env))),
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| substitute_container_env(item, env))
                .collect(),
        ),
        Value::Object(entries) => Value::Object(
            entries
                .iter()
                .map(|(key, value)| (key.clone(), substitute_container_env(value, env)))
                .collect(),
        ),
        _ => value.clone(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::PathBuf;

    use serde_json::json;

    use super::{
        encode_base32hex_lower, substitute_container_env, substitute_local_context, ConfigContext,
    };

    #[test]
    fn substitution_preserves_unknown_and_unclosed_tokens() {
        let context = ConfigContext {
            workspace_folder: PathBuf::from("/workspace"),
            env: HashMap::new(),
            container_workspace_folder: None,
            id_labels: HashMap::new(),
        };

        let substituted = substitute_local_context(
            &json!({
                "unknown": "${unknown:value}",
                "unclosed": "prefix ${localWorkspaceFolder",
                "bool": true
            }),
            &context,
        );

        assert_eq!(substituted["unknown"], "${unknown:value}");
        assert_eq!(substituted["unclosed"], "prefix ${localWorkspaceFolder");
        assert_eq!(substituted["bool"], true);
    }

    #[test]
    fn container_env_substitution_recurses_arrays_objects_and_scalars() {
        let substituted = substitute_container_env(
            &json!({
                "items": [
                    "${containerEnv:PATH}",
                    {
                        "fallback": "${containerEnv:MISSING:/bin}"
                    },
                    false
                ]
            }),
            &HashMap::from([("PATH".to_string(), "/usr/bin".to_string())]),
        );

        assert_eq!(substituted["items"][0], "/usr/bin");
        assert_eq!(substituted["items"][1]["fallback"], "/bin");
        assert_eq!(substituted["items"][2], false);
    }

    #[test]
    fn base32hex_encoding_pads_short_hash_inputs() {
        let zero = encode_base32hex_lower(&[0]);
        assert_eq!(zero.len(), 52);
        assert!(zero.ends_with("00"));

        let ones = encode_base32hex_lower(&[0xff]);
        assert_eq!(ones.len(), 52);
        assert!(ones.ends_with("vs"));
    }
}
