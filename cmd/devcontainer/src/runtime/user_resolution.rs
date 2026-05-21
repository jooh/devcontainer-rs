//! Runtime user and home-folder resolution helpers.

use std::collections::HashMap;

use serde_json::Value;

use super::{context, engine};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PasswdUser {
    pub(crate) name: String,
    pub(crate) uid: String,
    pub(crate) gid: String,
    pub(crate) home: String,
    pub(crate) shell: String,
}

#[derive(Debug, Default, Eq, PartialEq)]
struct InspectedContainerDetails {
    env: HashMap<String, String>,
    user: Option<String>,
}

pub(crate) fn combined_remote_env_with_home(
    args: &[String],
    configuration: &Value,
    container_id: &str,
) -> Result<HashMap<String, String>, String> {
    let mut remote_env = context::combined_remote_env(args, Some(configuration))?;
    if !remote_env.contains_key("HOME") {
        let home = resolve_home_folder(args, configuration, container_id)?;
        remote_env.insert("HOME".to_string(), home);
    }
    Ok(remote_env)
}

pub(crate) fn get_ent_passwd_shell_command(user_name_or_id: &str) -> String {
    let escaped_for_shell = escape_getent_shell_value(user_name_or_id);
    let escaped_for_regex = escape_regex_characters(user_name_or_id).replace('\'', "\\'");
    format!(
        " (command -v getent >/dev/null 2>&1 && getent passwd '{escaped_for_shell}' || grep -E '^{escaped_for_regex}|^[^:]*:[^:]*:{escaped_for_regex}:' /etc/passwd || true)"
    )
}

pub(crate) fn parse_passwd_user(stdout: &str) -> Option<PasswdUser> {
    let line = stdout
        .trim_end_matches('\n')
        .lines()
        .next()
        .unwrap_or(stdout);
    if line.trim().is_empty() {
        return None;
    }
    let fields = line.split(':').collect::<Vec<_>>();
    Some(PasswdUser {
        name: fields.first().unwrap_or(&"").to_string(),
        uid: fields.get(2).unwrap_or(&"").to_string(),
        gid: fields.get(3).unwrap_or(&"").to_string(),
        home: fields.get(5).unwrap_or(&"").to_string(),
        shell: fields.get(6).unwrap_or(&"").to_string(),
    })
}

pub(crate) fn select_home_folder(
    container_home: Option<&str>,
    passwd_user: Option<&PasswdUser>,
    mut is_missing_or_writable: impl FnMut(&str) -> Result<bool, String>,
) -> Result<String, String> {
    if let Some(home) = container_home.filter(|home| !home.is_empty()) {
        if Some(home) == passwd_user.map(|user| user.home.as_str())
            || passwd_user.is_some_and(|user| user.uid == "0")
            || is_missing_or_writable(home)?
        {
            return Ok(home.to_string());
        }
    }

    Ok(passwd_user
        .map(|user| user.home.clone())
        .unwrap_or_else(|| "/root".to_string()))
}

fn resolve_home_folder(
    args: &[String],
    configuration: &Value,
    container_id: &str,
) -> Result<String, String> {
    let inspected = inspected_container_details(args, container_id)?;
    let user_name_or_id = passwd_lookup_user(configuration, inspected.user.as_deref());
    let passwd_user = get_user_from_passwd_db(args, container_id, &user_name_or_id)?;
    let mut container_env = configuration_container_env(configuration);
    container_env.extend(inspected.env);
    select_home_folder(
        container_env.get("HOME").map(String::as_str),
        passwd_user.as_ref(),
        |home| container_home_missing_or_writable(args, configuration, container_id, home),
    )
}

fn get_user_from_passwd_db(
    args: &[String],
    container_id: &str,
    user_name_or_id: &str,
) -> Result<Option<PasswdUser>, String> {
    let result = crate::coverage_expect_result!(
        engine::run_engine(
            args,
            vec![
                "exec".to_string(),
                "-i".to_string(),
                container_id.to_string(),
                "/bin/sh".to_string(),
                "-lc".to_string(),
                get_ent_passwd_shell_command(user_name_or_id),
            ],
        ),
        "passwd lookup process launch failures are covered through engine tests"
    );
    #[cfg(not(coverage))]
    if result.status_code != 0 {
        return Err(engine::stderr_or_stdout(&result));
    }
    Ok(parse_passwd_user(&result.stdout))
}

fn container_home_missing_or_writable(
    args: &[String],
    configuration: &Value,
    container_id: &str,
    home: &str,
) -> Result<bool, String> {
    let quoted_home = shell_single_quote(home);
    let mut engine_args = vec!["exec".to_string(), "-i".to_string()];
    if let Some(user) = context::configured_user(configuration) {
        engine_args.push("--user".to_string());
        engine_args.push(user.to_string());
    }
    engine_args.push(container_id.to_string());
    engine_args.push("/bin/sh".to_string());
    engine_args.push("-lc".to_string());
    engine_args.push(format!("[ ! -e {quoted_home} ] || [ -w {quoted_home} ]"));

    let result = crate::coverage_expect_result!(
        engine::run_engine(args, engine_args),
        "container home writability process launch failures are covered through engine tests"
    );
    Ok(result.status_code == 0)
}

fn configuration_container_env(configuration: &Value) -> HashMap<String, String> {
    configuration
        .get("containerEnv")
        .and_then(Value::as_object)
        .map(|container_env| {
            container_env
                .iter()
                .filter_map(|(key, value)| value.as_str().map(|value| (key.clone(), value.into())))
                .collect()
        })
        .unwrap_or_default()
}

fn inspected_container_details(
    args: &[String],
    container_id: &str,
) -> Result<InspectedContainerDetails, String> {
    let result = crate::coverage_expect_result!(
        engine::run_engine(args, vec!["inspect".to_string(), container_id.to_string()]),
        "container inspect process launch failures are covered through engine tests"
    );
    #[cfg(not(coverage))]
    if result.status_code != 0 {
        return Err(engine::stderr_or_stdout(&result));
    }

    let inspected: Value = serde_json::from_str(&result.stdout)
        .map_err(|error| format!("Invalid inspect JSON: {error}"))?;
    let config = inspected
        .as_array()
        .and_then(|entries| entries.first())
        .and_then(|details| details.get("Config"));
    let env_entries = config
        .and_then(|config| config.get("Env"))
        .and_then(Value::as_array);
    let user = config
        .and_then(|config| config.get("User"))
        .and_then(Value::as_str)
        .filter(|user| !user.is_empty())
        .map(str::to_string);

    Ok(InspectedContainerDetails {
        env: env_entries
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .filter_map(|entry| {
                let (key, value) = entry.split_once('=')?;
                Some((key.to_string(), value.to_string()))
            })
            .collect(),
        user,
    })
}

fn passwd_lookup_user(configuration: &Value, inspected_user: Option<&str>) -> String {
    context::configured_user(configuration)
        .or(inspected_user)
        .and_then(|user| user.split(':').next())
        .filter(|user| !user.is_empty())
        .map(|user| if user == "0" { "root" } else { user })
        .unwrap_or("root")
        .to_string()
}

fn escape_getent_shell_value(value: &str) -> String {
    let mut escaped = String::new();
    for ch in value.chars() {
        if matches!(ch, '\'' | '\\') {
            escaped.push('\\');
        }
        escaped.push(ch);
    }
    escaped
}

fn escape_regex_characters(value: &str) -> String {
    let mut escaped = String::new();
    for ch in value.chars() {
        if matches!(
            ch,
            '.' | '*' | '+' | '?' | '^' | '$' | '{' | '}' | '(' | ')' | '|' | '[' | ']' | '\\'
        ) {
            escaped.push('\\');
        }
        escaped.push(ch);
    }
    escaped
}

fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        get_ent_passwd_shell_command, parse_passwd_user, passwd_lookup_user, select_home_folder,
        PasswdUser,
    };

    fn vscode_user() -> PasswdUser {
        PasswdUser {
            name: "vscode".to_string(),
            uid: "1000".to_string(),
            gid: "1000".to_string(),
            home: "/home/vscode".to_string(),
            shell: "/bin/bash".to_string(),
        }
    }

    fn root_user() -> PasswdUser {
        PasswdUser {
            name: "root".to_string(),
            uid: "0".to_string(),
            gid: "0".to_string(),
            home: "/root".to_string(),
            shell: "/bin/sh".to_string(),
        }
    }

    #[test]
    fn parse_passwd_user_reads_passwd_row_fields() {
        assert_eq!(
            parse_passwd_user("vscode:x:1000:1000::/home/vscode:/bin/bash\n"),
            Some(vscode_user())
        );
    }

    #[test]
    fn parse_passwd_user_ignores_empty_lookup_output() {
        assert_eq!(parse_passwd_user("\n"), None);
        assert_eq!(parse_passwd_user(""), None);
    }

    #[test]
    fn get_ent_passwd_shell_command_matches_upstream_for_names_and_ids() {
        assert_eq!(
            get_ent_passwd_shell_command("vscode"),
            " (command -v getent >/dev/null 2>&1 && getent passwd 'vscode' || grep -E '^vscode|^[^:]*:[^:]*:vscode:' /etc/passwd || true)"
        );
        assert_eq!(
            get_ent_passwd_shell_command("1000"),
            " (command -v getent >/dev/null 2>&1 && getent passwd '1000' || grep -E '^1000|^[^:]*:[^:]*:1000:' /etc/passwd || true)"
        );
    }

    #[test]
    fn get_ent_passwd_shell_command_matches_upstream_escaping() {
        assert_eq!(
            get_ent_passwd_shell_command("foo\\bar"),
            r" (command -v getent >/dev/null 2>&1 && getent passwd 'foo\\bar' || grep -E '^foo\\bar|^[^:]*:[^:]*:foo\\bar:' /etc/passwd || true)"
        );
        assert_eq!(
            get_ent_passwd_shell_command("o'brien"),
            r" (command -v getent >/dev/null 2>&1 && getent passwd 'o\'brien' || grep -E '^o\'brien|^[^:]*:[^:]*:o\'brien:' /etc/passwd || true)"
        );
    }

    #[test]
    fn select_home_folder_ignores_unwritable_non_root_home() {
        let home = select_home_folder(Some("/root"), Some(&vscode_user()), |_| Ok(false))
            .expect("home folder");

        assert_eq!(home, "/home/vscode");
    }

    #[test]
    fn select_home_folder_accepts_matching_or_writable_non_root_home() {
        let matching = select_home_folder(Some("/home/vscode"), Some(&vscode_user()), |_| {
            panic!("matching passwd home should not need a writability check")
        })
        .expect("matching home");
        let writable =
            select_home_folder(Some("/home/vscode/project"), Some(&vscode_user()), |_| {
                Ok(true)
            })
            .expect("writable home");

        assert_eq!(matching, "/home/vscode");
        assert_eq!(writable, "/home/vscode/project");
    }

    #[test]
    fn select_home_folder_accepts_any_home_for_root() {
        let home = select_home_folder(Some("/home/vscode"), Some(&root_user()), |_| {
            panic!("root home should not need a writability check")
        })
        .expect("root home");

        assert_eq!(home, "/home/vscode");
    }

    #[test]
    fn select_home_folder_falls_back_to_passwd_home_or_root() {
        assert_eq!(
            select_home_folder(None, Some(&vscode_user()), |_| Ok(false)).expect("passwd home"),
            "/home/vscode"
        );
        assert_eq!(
            select_home_folder(None, None, |_| Ok(false)).expect("root fallback"),
            "/root"
        );
    }

    #[test]
    fn passwd_lookup_user_prefers_devcontainer_user_then_inspected_user() {
        assert_eq!(
            passwd_lookup_user(&json!({ "remoteUser": "node" }), Some("vscode")),
            "node"
        );
        assert_eq!(
            passwd_lookup_user(&json!({ "containerUser": "1000:1000" }), Some("vscode")),
            "1000"
        );
        assert_eq!(
            passwd_lookup_user(&json!({}), Some("vscode:1000")),
            "vscode"
        );
        assert_eq!(passwd_lookup_user(&json!({}), Some("0:0")), "root");
        assert_eq!(passwd_lookup_user(&json!({}), None), "root");
    }
}
