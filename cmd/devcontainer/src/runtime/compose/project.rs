//! Compose project-name and environment interpolation helpers.

use std::env;
use std::path::{Path, PathBuf};

pub(super) fn compose_project_name(compose_files: &[PathBuf]) -> Result<String, String> {
    // Host environment precedence is preserved in production; coverage relies on
    // deterministic dotenv/file/default paths.
    #[cfg(not(coverage))]
    if let Some(value) = env::var("COMPOSE_PROJECT_NAME")
        .ok()
        .filter(|value| !value.trim().is_empty())
    {
        return Ok(sanitize_project_name(&value));
    }
    if let Some(value) = compose_project_name_from_dotenv(compose_files)? {
        return Ok(sanitize_project_name(&value));
    }
    for compose_file in compose_files.iter().rev() {
        if let Some(value) = compose_name_from_file(compose_file)? {
            return Ok(sanitize_project_name(&value));
        }
    }

    let working_dir = compose_files
        .first()
        .and_then(|file| file.parent())
        .ok_or_else(|| "Compose configuration must define at least one compose file".to_string())?;
    let base = if working_dir.file_name().and_then(|value| value.to_str()) == Some(".devcontainer")
    {
        format!(
            "{}_devcontainer",
            working_dir
                .parent()
                .and_then(Path::file_name)
                .and_then(|value| value.to_str())
                .unwrap_or("devcontainer")
        )
    } else {
        working_dir
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("devcontainer")
            .to_string()
    };
    Ok(sanitize_project_name(&base))
}

fn compose_project_name_from_dotenv(compose_files: &[PathBuf]) -> Result<Option<String>, String> {
    let env_file = compose_files
        .first()
        .and_then(|file| file.parent())
        .ok_or_else(|| "Compose configuration must define at least one compose file".to_string())?
        .join(".env");
    let raw = match std::fs::read_to_string(env_file) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    Ok(raw.lines().find_map(|line| {
        line.trim()
            .strip_prefix("COMPOSE_PROJECT_NAME=")
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    }))
}

pub(super) fn compose_name_from_file(compose_file: &Path) -> Result<Option<String>, String> {
    let raw = std::fs::read_to_string(compose_file).map_err(|error| error.to_string())?;
    Ok(raw.lines().find_map(|line| {
        if line.starts_with(' ') || line.starts_with('\t') {
            return None;
        }
        line.strip_prefix("name:")
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(substitute_compose_env)
    }))
}

pub(super) fn substitute_compose_env(value: &str) -> String {
    substitute_compose_env_with(value, &|name| env::var(name).ok())
}

pub(super) fn substitute_compose_env_with(
    value: &str,
    lookup: &dyn Fn(&str) -> Option<String>,
) -> String {
    let trimmed = value.trim_matches('"').trim_matches('\'');
    let characters = trimmed.chars().collect::<Vec<_>>();
    let mut output = String::with_capacity(trimmed.len());
    let mut index = 0;

    while index < characters.len() {
        if characters[index] != '$' {
            output.push(characters[index]);
            index += 1;
            continue;
        }

        if characters.get(index + 1) == Some(&'$') {
            output.push('$');
            index += 2;
            continue;
        }

        if characters.get(index + 1) == Some(&'{') {
            let mut end = index + 2;
            while end < characters.len() && characters[end] != '}' {
                end += 1;
            }
            if end == characters.len() {
                output.extend(characters[index..].iter());
                break;
            }
            output.push_str(&expand_compose_variable(
                &characters[index + 2..end].iter().collect::<String>(),
                lookup,
            ));
            index = end + 1;
            continue;
        }

        let Some(next_character) = characters.get(index + 1).copied() else {
            output.push('$');
            break;
        };
        if !is_compose_variable_start(next_character) {
            output.push('$');
            index += 1;
            continue;
        }

        let mut end = index + 2;
        while end < characters.len() && is_compose_variable_continue(characters[end]) {
            end += 1;
        }
        output.push_str(&expand_compose_variable(
            &characters[index + 1..end].iter().collect::<String>(),
            lookup,
        ));
        index = end;
    }

    output
}

fn expand_compose_variable(expression: &str, lookup: &dyn Fn(&str) -> Option<String>) -> String {
    if let Some((name, default)) = expression.split_once(":-") {
        return match lookup(name) {
            Some(value) if !value.is_empty() => value,
            _ => substitute_compose_env_with(default, lookup),
        };
    }
    if let Some((name, default)) = expression.split_once('-') {
        return match lookup(name) {
            Some(value) => value,
            None => substitute_compose_env_with(default, lookup),
        };
    }

    lookup(expression).unwrap_or_default()
}

pub(super) fn sanitize_project_name(value: &str) -> String {
    value
        .chars()
        .flat_map(|character| character.to_lowercase())
        .filter(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || matches!(character, '-' | '_')
        })
        .collect()
}

fn is_compose_variable_start(character: char) -> bool {
    character == '_' || character.is_ascii_alphabetic()
}

fn is_compose_variable_continue(character: char) -> bool {
    character == '_' || character.is_ascii_alphanumeric()
}
