//! Dockerfile parsing helpers that mirror deterministic upstream utility behavior.

#![cfg_attr(not(test), allow(dead_code))]

use std::collections::{HashMap, HashSet};

#[derive(Debug, Eq, PartialEq)]
struct FinalStageName {
    last_stage_name: String,
    modified_dockerfile: Option<String>,
}

#[derive(Debug, Eq, PartialEq)]
struct Dockerfile {
    preamble: Preamble,
    stages: Vec<Stage>,
    stages_by_label: HashMap<String, usize>,
}

#[derive(Debug, Eq, PartialEq)]
struct Preamble {
    version: Option<String>,
    directives: HashMap<String, String>,
    instructions: Vec<Instruction>,
}

#[derive(Debug, Eq, PartialEq)]
struct Stage {
    from: From,
    instructions: Vec<Instruction>,
}

#[derive(Debug, Eq, PartialEq)]
struct From {
    platform: Option<String>,
    image: String,
    label: Option<String>,
}

#[derive(Debug, Eq, PartialEq)]
struct Instruction {
    instruction: String,
    name: String,
    value: Option<String>,
}

#[derive(Debug, Eq, PartialEq)]
enum BuildContextSupport {
    Supported,
    Unsupported,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum ScopeId {
    Preamble,
    Stage(usize),
}

struct FromLine {
    from: From,
    matched_end: usize,
}

struct Token {
    text: String,
    end: usize,
}

enum VariableOperator {
    Default,
    Alternate,
}

struct VariableExpression<'a> {
    name: &'a str,
    operator: Option<VariableOperator>,
    word: Option<&'a str>,
}

fn ensure_dockerfile_has_final_stage_name(
    dockerfile: &str,
    default_last_stage_name: &str,
) -> Result<FinalStageName, String> {
    let Some((line_start, line)) = last_from_line(dockerfile) else {
        return Err(
            "Error parsing Dockerfile: Dockerfile contains no FROM instructions".to_string(),
        );
    };
    let parsed = parse_from_line(line)
        .ok_or_else(|| "Error parsing Dockerfile: failed to parse final FROM line".to_string())?;
    if let Some(label) = parsed.from.label {
        return Ok(FinalStageName {
            last_stage_name: label,
            modified_dockerfile: None,
        });
    }

    let insert_at = line_start + parsed.matched_end;
    let mut modified_dockerfile =
        String::with_capacity(dockerfile.len() + " AS ".len() + default_last_stage_name.len());
    modified_dockerfile.push_str(&dockerfile[..insert_at]);
    modified_dockerfile.push_str(" AS ");
    modified_dockerfile.push_str(default_last_stage_name);
    modified_dockerfile.push_str(&dockerfile[insert_at..]);

    Ok(FinalStageName {
        last_stage_name: default_last_stage_name.to_string(),
        modified_dockerfile: Some(modified_dockerfile),
    })
}

fn extract_dockerfile(dockerfile: &str) -> Dockerfile {
    let mut preamble = String::new();
    let mut stage_strings = Vec::new();
    let mut current_stage = None::<String>;

    for line in dockerfile.split_inclusive('\n') {
        if is_from_line(line) {
            if let Some(stage) = current_stage.replace(line.to_string()) {
                stage_strings.push(stage);
            }
        } else if let Some(stage) = current_stage.as_mut() {
            stage.push_str(line);
        } else {
            preamble.push_str(line);
        }
    }
    if let Some(stage) = current_stage {
        stage_strings.push(stage);
    }

    let directives = extract_directives(&preamble);
    let version = directives
        .get("syntax")
        .and_then(|syntax| dockerfile_syntax_version(syntax));
    let stages = stage_strings
        .iter()
        .map(|stage| Stage {
            from: stage
                .lines()
                .find_map(parse_from_line)
                .map(|line| line.from)
                .unwrap_or_else(|| From {
                    platform: None,
                    image: "unknown".to_string(),
                    label: None,
                }),
            instructions: extract_instructions(stage),
        })
        .collect::<Vec<_>>();
    let stages_by_label = stages
        .iter()
        .enumerate()
        .filter_map(|(index, stage)| {
            stage
                .from
                .label
                .as_ref()
                .map(|label| (label.clone(), index))
        })
        .collect::<HashMap<_, _>>();

    Dockerfile {
        preamble: Preamble {
            version,
            directives,
            instructions: extract_instructions(&preamble),
        },
        stages,
        stages_by_label,
    }
}

fn find_base_image(
    dockerfile: &Dockerfile,
    build_args: &HashMap<String, String>,
    target: Option<&str>,
    global_buildx_platform_args: &HashMap<String, String>,
) -> Option<String> {
    let mut stage = target_stage_index(dockerfile, target)?;
    let mut seen = HashSet::new();

    loop {
        if !seen.insert(stage) {
            return None;
        }
        let image = replace_variables(
            dockerfile,
            build_args,
            &HashMap::new(),
            global_buildx_platform_args,
            &dockerfile.stages[stage].from.image,
            ScopeId::Preamble,
            dockerfile.preamble.instructions.len(),
        );
        let Some(next_stage) = dockerfile.stages_by_label.get(&image).copied() else {
            return Some(image);
        };
        stage = next_stage;
    }
}

fn find_user_statement(
    dockerfile: &Dockerfile,
    build_args: &HashMap<String, String>,
    base_image_env: &HashMap<String, String>,
    global_buildx_platform_args: &HashMap<String, String>,
    target: Option<&str>,
) -> Option<String> {
    let mut stage = target_stage_index(dockerfile, target)?;
    let mut seen = HashSet::new();

    loop {
        if !seen.insert(stage) {
            return None;
        }
        if let Some(index) = find_last_instruction_index(
            &dockerfile.stages[stage].instructions,
            |instruction| instruction.instruction == "USER",
            dockerfile.stages[stage].instructions.len(),
        ) {
            return non_empty(replace_variables(
                dockerfile,
                build_args,
                base_image_env,
                global_buildx_platform_args,
                &dockerfile.stages[stage].instructions[index].name,
                ScopeId::Stage(stage),
                index,
            ));
        }

        let image = replace_variables(
            dockerfile,
            build_args,
            base_image_env,
            global_buildx_platform_args,
            &dockerfile.stages[stage].from.image,
            ScopeId::Preamble,
            dockerfile.preamble.instructions.len(),
        );
        let next_stage = dockerfile.stages_by_label.get(&image).copied()?;
        stage = next_stage;
    }
}

fn target_stage_index(dockerfile: &Dockerfile, target: Option<&str>) -> Option<usize> {
    match target {
        Some(target) => dockerfile.stages_by_label.get(target).copied(),
        None => dockerfile.stages.len().checked_sub(1),
    }
}

fn supports_build_contexts(dockerfile: &Dockerfile) -> BuildContextSupport {
    let Some(version) = dockerfile.preamble.version.as_deref() else {
        return if dockerfile.preamble.directives.contains_key("syntax") {
            BuildContextSupport::Unknown
        } else {
            BuildContextSupport::Unsupported
        };
    };
    let numeric_version = version
        .chars()
        .take_while(|character| character.is_ascii_digit() || *character == '.')
        .collect::<String>();
    if numeric_version.is_empty() {
        return BuildContextSupport::Supported;
    }
    if numeric_version_supports_build_contexts(&numeric_version) {
        BuildContextSupport::Supported
    } else {
        BuildContextSupport::Unsupported
    }
}

fn last_from_line(dockerfile: &str) -> Option<(usize, &str)> {
    let mut offset = 0;
    let mut result = None;
    for raw_line in dockerfile.split_inclusive('\n') {
        let line = raw_line.trim_end_matches(['\r', '\n']);
        if is_from_line(line) {
            result = Some((offset, line));
        }
        offset += raw_line.len();
    }
    result
}

fn is_from_line(line: &str) -> bool {
    tokenize_line(line)
        .first()
        .is_some_and(|token| token.text.eq_ignore_ascii_case("FROM"))
}

fn parse_from_line(line: &str) -> Option<FromLine> {
    let tokens = tokenize_line(line);
    if !tokens
        .first()
        .is_some_and(|token| token.text.eq_ignore_ascii_case("FROM"))
    {
        return None;
    }

    let mut index = 1;
    let platform = tokens
        .get(index)
        .filter(|token| token.text.starts_with("--platform="))
        .map(|token| {
            index += 1;
            token.text.clone()
        });
    let image = tokens.get(index)?;
    index += 1;
    let label = tokens
        .get(index)
        .filter(|token| token.text.eq_ignore_ascii_case("AS"))
        .and_then(|_| tokens.get(index + 1))
        .map(|token| strip_edge_quotes(&token.text));
    let matched_end = label
        .as_ref()
        .and_then(|_| tokens.get(index + 1))
        .map(|token| token.end)
        .unwrap_or(image.end);

    Some(FromLine {
        from: From {
            platform,
            image: strip_edge_quotes(&image.text),
            label,
        },
        matched_end,
    })
}

fn tokenize_line(line: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut token_start = None;
    let mut quote = None;

    for (index, character) in line.char_indices() {
        if token_start.is_none() {
            if character.is_whitespace() {
                continue;
            }
            if character == '#' {
                break;
            }
            token_start = Some(index);
        }

        if let Some(active_quote) = quote {
            if character == active_quote {
                quote = None;
            }
            continue;
        }
        if character == '\'' || character == '"' {
            quote = Some(character);
            continue;
        }
        if character.is_whitespace() {
            let start = token_start.take().expect("token start");
            tokens.push(Token {
                text: line[start..index].to_string(),
                end: index,
            });
        }
    }

    if let Some(start) = token_start {
        tokens.push(Token {
            text: line[start..].to_string(),
            end: line.len(),
        });
    }

    tokens
}

fn extract_directives(preamble: &str) -> HashMap<String, String> {
    let mut directives = HashMap::new();
    for line in preamble.lines() {
        let trimmed = line.trim_start();
        let Some(after_comment) = trimmed.strip_prefix('#') else {
            break;
        };
        let Some((name, value)) = after_comment.split_once('=') else {
            break;
        };
        let name = name.trim().to_ascii_lowercase();
        let value = value.trim().to_string();
        if name.is_empty() || value.is_empty() {
            break;
        }
        directives.entry(name).or_insert(value);
    }
    directives
}

fn dockerfile_syntax_version(syntax: &str) -> Option<String> {
    let syntax = syntax.to_ascii_lowercase();
    for prefix in ["docker/dockerfile", "docker.io/docker/dockerfile"] {
        if syntax == prefix {
            return Some("latest".to_string());
        }
        if let Some(version) = syntax
            .strip_prefix(prefix)
            .and_then(|rest| rest.strip_prefix(':'))
        {
            return Some(version.to_string());
        }
    }
    None
}

fn numeric_version_supports_build_contexts(version: &str) -> bool {
    let parts = version
        .split('.')
        .filter_map(|part| part.parse::<u64>().ok())
        .collect::<Vec<_>>();
    match parts.as_slice() {
        [major] => *major >= 1,
        [major, minor, ..] => *major > 1 || (*major == 1 && *minor >= 4),
        _ => false,
    }
}

fn extract_instructions(section: &str) -> Vec<Instruction> {
    section.lines().filter_map(parse_instruction_line).collect()
}

fn parse_instruction_line(line: &str) -> Option<Instruction> {
    let trimmed = line.trim_start();
    for keyword in ["ARG", "ENV", "USER"] {
        if let Some(rest) = strip_instruction_keyword(trimmed, keyword) {
            let (name, value) = parse_instruction_name_value(rest)?;
            return Some(Instruction {
                instruction: keyword.to_string(),
                name,
                value,
            });
        }
    }
    None
}

fn strip_instruction_keyword<'a>(line: &'a str, keyword: &str) -> Option<&'a str> {
    if line.len() < keyword.len() {
        return None;
    }
    let (candidate, rest) = line.split_at(keyword.len());
    if !candidate.eq_ignore_ascii_case(keyword) {
        return None;
    }
    rest.chars()
        .next()
        .is_some_and(char::is_whitespace)
        .then(|| rest.trim_start())
}

fn parse_instruction_name_value(rest: &str) -> Option<(String, Option<String>)> {
    let name_end = rest
        .char_indices()
        .find_map(|(index, character)| {
            (character.is_whitespace() || character == '=').then_some(index)
        })
        .unwrap_or(rest.len());
    let name = rest[..name_end].to_string();
    if name.is_empty() {
        return None;
    }

    let mut value = rest[name_end..].trim_start();
    if let Some(after_equals) = value.strip_prefix('=') {
        value = after_equals.trim_start();
    }
    let value = first_field(value).map(strip_edge_quotes);
    Some((name, value))
}

fn first_field(value: &str) -> Option<&str> {
    let trimmed = value.trim_start();
    if trimmed.is_empty() {
        return None;
    }
    let end = trimmed
        .char_indices()
        .find_map(|(index, character)| character.is_whitespace().then_some(index))
        .unwrap_or(trimmed.len());
    Some(&trimmed[..end])
}

fn replace_variables(
    dockerfile: &Dockerfile,
    build_args: &HashMap<String, String>,
    base_image_env: &HashMap<String, String>,
    global_buildx_platform_args: &HashMap<String, String>,
    input: &str,
    scope: ScopeId,
    before_instruction_index: usize,
) -> String {
    let mut output = String::with_capacity(input.len());
    let mut index = 0;

    while index < input.len() {
        if !input[index..].starts_with('$') {
            let character = input[index..].chars().next().expect("character");
            output.push(character);
            index += character.len_utf8();
            continue;
        }

        if input[index + 1..].starts_with('{') {
            let content_start = index + 2;
            let Some(close_offset) = input[content_start..].find('}') else {
                output.push('$');
                index += 1;
                continue;
            };
            let content_end = content_start + close_offset;
            let content = &input[content_start..content_end];
            if let Some(expression) = parse_variable_expression(content) {
                output.push_str(&resolve_variable_expression(
                    dockerfile,
                    build_args,
                    base_image_env,
                    global_buildx_platform_args,
                    scope,
                    before_instruction_index,
                    expression,
                ));
                index = content_end + 1;
            } else {
                output.push('$');
                index += 1;
            }
            continue;
        }

        let variable_start = index + 1;
        let variable_end = input[variable_start..]
            .char_indices()
            .find_map(|(offset, character)| {
                (!is_variable_character(character)).then_some(variable_start + offset)
            })
            .unwrap_or(input.len());
        if variable_end == variable_start {
            output.push('$');
            index += 1;
            continue;
        }
        let expression = VariableExpression {
            name: &input[variable_start..variable_end],
            operator: None,
            word: None,
        };
        output.push_str(&resolve_variable_expression(
            dockerfile,
            build_args,
            base_image_env,
            global_buildx_platform_args,
            scope,
            before_instruction_index,
            expression,
        ));
        index = variable_end;
    }

    output
}

fn parse_variable_expression(content: &str) -> Option<VariableExpression<'_>> {
    let variable_end = content
        .char_indices()
        .find_map(|(index, character)| (!is_variable_character(character)).then_some(index))
        .unwrap_or(content.len());
    if variable_end == 0 {
        return None;
    }

    let name = &content[..variable_end];
    let rest = &content[variable_end..];
    if rest.is_empty() {
        return Some(VariableExpression {
            name,
            operator: None,
            word: None,
        });
    }
    let (operator, word) = if let Some(word) = rest.strip_prefix(":-") {
        (VariableOperator::Default, word)
    } else if let Some(word) = rest.strip_prefix(":+") {
        (VariableOperator::Alternate, word)
    } else {
        return None;
    };
    Some(VariableExpression {
        name,
        operator: Some(operator),
        word: Some(word),
    })
}

fn resolve_variable_expression(
    dockerfile: &Dockerfile,
    build_args: &HashMap<String, String>,
    base_image_env: &HashMap<String, String>,
    global_buildx_platform_args: &HashMap<String, String>,
    scope: ScopeId,
    before_instruction_index: usize,
    expression: VariableExpression<'_>,
) -> String {
    let value = find_value(
        dockerfile,
        build_args,
        base_image_env,
        global_buildx_platform_args,
        expression.name,
        scope,
        before_instruction_index,
    )
    .unwrap_or_default();
    match (expression.operator, expression.word) {
        (Some(VariableOperator::Default), Some(word)) => {
            if value.is_empty() {
                strip_edge_quotes(word)
            } else {
                value
            }
        }
        (Some(VariableOperator::Alternate), Some(word)) => {
            if value.is_empty() {
                value
            } else {
                strip_edge_quotes(word)
            }
        }
        _ => value,
    }
}

fn find_value(
    dockerfile: &Dockerfile,
    build_args: &HashMap<String, String>,
    base_image_env: &HashMap<String, String>,
    global_buildx_platform_args: &HashMap<String, String>,
    variable: &str,
    initial_scope: ScopeId,
    initial_before_instruction_index: usize,
) -> Option<String> {
    let mut scope = initial_scope;
    let mut before_instruction_index = initial_before_instruction_index;
    let mut consider_arg = true;
    let mut seen = HashSet::new();

    loop {
        if !seen.insert(scope) {
            return None;
        }
        let instructions = scope_instructions(dockerfile, scope);
        if let Some(index) = find_last_instruction_index(
            instructions,
            |instruction| {
                instruction.name == variable
                    && (instruction.instruction == "ENV"
                        || (consider_arg
                            && (build_args.contains_key(&instruction.name)
                                || instruction.value.is_some())))
            },
            before_instruction_index.min(instructions.len()),
        ) {
            let instruction = &instructions[index];
            if instruction.instruction == "ENV" {
                return instruction.value.as_ref().map(|value| {
                    replace_variables(
                        dockerfile,
                        build_args,
                        base_image_env,
                        global_buildx_platform_args,
                        value,
                        scope,
                        index,
                    )
                });
            }
            if instruction.instruction == "ARG" {
                let value = build_args
                    .get(&instruction.name)
                    .or(instruction.value.as_ref())?;
                return Some(replace_variables(
                    dockerfile,
                    build_args,
                    base_image_env,
                    global_buildx_platform_args,
                    value,
                    scope,
                    index,
                ));
            }
        }

        let Some(from) = scope_from(dockerfile, scope) else {
            return base_image_env
                .get(variable)
                .or_else(|| global_buildx_platform_args.get(variable))
                .cloned();
        };
        let image = replace_variables(
            dockerfile,
            build_args,
            base_image_env,
            global_buildx_platform_args,
            &from.image,
            ScopeId::Preamble,
            dockerfile.preamble.instructions.len(),
        );
        scope = dockerfile
            .stages_by_label
            .get(&image)
            .copied()
            .map(ScopeId::Stage)
            .unwrap_or(ScopeId::Preamble);
        before_instruction_index = scope_instructions(dockerfile, scope).len();
        consider_arg = matches!(scope, ScopeId::Preamble);
    }
}

fn find_last_instruction_index(
    instructions: &[Instruction],
    predicate: impl Fn(&Instruction) -> bool,
    before_instruction_index: usize,
) -> Option<usize> {
    (0..before_instruction_index)
        .rev()
        .find(|index| predicate(&instructions[*index]))
}

fn scope_instructions(dockerfile: &Dockerfile, scope: ScopeId) -> &[Instruction] {
    match scope {
        ScopeId::Preamble => &dockerfile.preamble.instructions,
        ScopeId::Stage(index) => &dockerfile.stages[index].instructions,
    }
}

fn scope_from(dockerfile: &Dockerfile, scope: ScopeId) -> Option<&From> {
    match scope {
        ScopeId::Preamble => None,
        ScopeId::Stage(index) => Some(&dockerfile.stages[index].from),
    }
}

fn non_empty(value: String) -> Option<String> {
    (!value.is_empty()).then_some(value)
}

fn is_variable_character(character: char) -> bool {
    character.is_ascii_alphanumeric() || character == '_'
}

fn strip_edge_quotes(value: &str) -> String {
    let mut start = 0;
    let mut end = value.len();
    if value
        .as_bytes()
        .first()
        .is_some_and(|byte| matches!(byte, b'\'' | b'"'))
    {
        start += 1;
    }
    if end > start
        && value
            .as_bytes()
            .get(end - 1)
            .is_some_and(|byte| matches!(byte, b'\'' | b'"'))
    {
        end -= 1;
    }
    value[start..end].to_string()
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::{
        ensure_dockerfile_has_final_stage_name, extract_dockerfile, find_base_image,
        find_user_statement, supports_build_contexts, BuildContextSupport,
    };

    #[test]
    fn ensures_final_stage_names_without_rewriting_named_stages() {
        let dockerfile = r#"
FROM ubuntu:latest as base

RUN some command

FROM base as final

COPY src dest
"#;

        let result =
            ensure_dockerfile_has_final_stage_name(dockerfile, "placeholder").expect("stage name");

        assert_eq!(result.last_stage_name, "final");
        assert_eq!(result.modified_dockerfile, None);
    }

    #[test]
    fn inserts_final_stage_names_before_comments_and_preserves_spacing() {
        let dockerfile = r#"
FROM ubuntu:latest as base

RUN some command

 	FROM  --platform=my-platform 	base   # deliberately includes: as something here

COPY src dest
"#;

        let result =
            ensure_dockerfile_has_final_stage_name(dockerfile, "placeholder").expect("stage name");

        assert_eq!(result.last_stage_name, "placeholder");
        assert_eq!(
            result.modified_dockerfile.as_deref(),
            Some(
                r#"
FROM ubuntu:latest as base

RUN some command

 	FROM  --platform=my-platform 	base AS placeholder   # deliberately includes: as something here

COPY src dest
"#
            )
        );
    }

    #[test]
    fn rejects_dockerfiles_without_from_instructions() {
        let error = ensure_dockerfile_has_final_stage_name("RUN some command\n", "placeholder")
            .expect_err("expected parse error");

        assert!(error.contains("Dockerfile contains no FROM instructions"));
    }

    #[test]
    fn extracts_env_forms_and_lowercase_instructions() {
        let dockerfile = "from E\nenv A=B\nenv C = D\nenv E F\narg G\nuser H\n";
        let extracted = extract_dockerfile(dockerfile);

        assert_eq!(extracted.stages.len(), 1);
        let stage = &extracted.stages[0];
        assert_eq!(stage.from.image, "E");
        assert_eq!(stage.instructions[0].instruction, "ENV");
        assert_eq!(stage.instructions[0].name, "A");
        assert_eq!(stage.instructions[0].value.as_deref(), Some("B"));
        assert_eq!(stage.instructions[1].name, "C");
        assert_eq!(stage.instructions[1].value.as_deref(), Some("D"));
        assert_eq!(stage.instructions[2].name, "E");
        assert_eq!(stage.instructions[2].value.as_deref(), Some("F"));
        assert_eq!(stage.instructions[3].instruction, "ARG");
        assert_eq!(stage.instructions[3].name, "G");
        assert_eq!(stage.instructions[4].instruction, "USER");
        assert_eq!(stage.instructions[4].name, "H");
    }

    #[test]
    fn resolves_base_images_from_args_quotes_aliases_and_expressions() {
        let extracted = extract_dockerfile(
            r#"
ARG BASE_IMAGE="image2"
FROM "${BASE_IMAGE}"
"#,
        );
        assert_eq!(
            find_base_image(&extracted, &HashMap::new(), None, &HashMap::new()).as_deref(),
            Some("image2")
        );
        assert_eq!(
            find_base_image(
                &extracted,
                &HashMap::from([("BASE_IMAGE".to_string(), "image3".to_string())]),
                None,
                &HashMap::new(),
            )
            .as_deref(),
            Some("image3")
        );

        let multistage = extract_dockerfile(
            r#"
FROM image1 as stage1
FROM stage3 as stage2
FROM image3 as stage3
FROM image4 as stage4
"#,
        );
        assert_eq!(
            find_base_image(
                &multistage,
                &HashMap::new(),
                Some("stage2"),
                &HashMap::new()
            )
            .as_deref(),
            Some("image3")
        );

        let positive_expression = extract_dockerfile(
            r#"
ARG cloud
FROM ${cloud:+"mcr.microsoft.com/"}azure-cli:latest"
"#,
        );
        assert_eq!(
            find_base_image(
                &positive_expression,
                &HashMap::from([("cloud".to_string(), "true".to_string())]),
                None,
                &HashMap::new(),
            )
            .as_deref(),
            Some("mcr.microsoft.com/azure-cli:latest")
        );
        assert_eq!(
            find_base_image(&positive_expression, &HashMap::new(), None, &HashMap::new(),)
                .as_deref(),
            Some("azure-cli:latest")
        );

        let negative_expression = extract_dockerfile(
            r#"
ARG cloud
FROM "${cloud:-"mcr.microsoft.com/"}azure-cli:latest"
"#,
        );
        assert_eq!(
            find_base_image(
                &negative_expression,
                &HashMap::from([("cloud".to_string(), "ghcr.io/".to_string())]),
                None,
                &HashMap::new(),
            )
            .as_deref(),
            Some("ghcr.io/azure-cli:latest")
        );
        assert_eq!(
            find_base_image(&negative_expression, &HashMap::new(), None, &HashMap::new(),)
                .as_deref(),
            Some("mcr.microsoft.com/azure-cli:latest")
        );
    }

    #[test]
    fn missing_targets_do_not_fall_back_to_final_stage() {
        let dockerfile = extract_dockerfile(
            r#"
FROM debian AS base
USER base-user

FROM ubuntu AS final
USER final-user
"#,
        );

        assert_eq!(
            find_base_image(
                &dockerfile,
                &HashMap::new(),
                Some("missing"),
                &HashMap::new()
            ),
            None
        );
        assert_eq!(
            find_user_statement(
                &dockerfile,
                &HashMap::new(),
                &HashMap::new(),
                &HashMap::new(),
                Some("missing"),
            ),
            None
        );
    }

    #[test]
    fn resolves_user_statements_with_arg_env_and_stage_precedence() {
        let arg_overwritten = extract_dockerfile(
            r#"
FROM debian
ARG IMAGE_USER=user2
USER $IMAGE_USER
"#,
        );
        assert_eq!(
            find_user_statement(
                &arg_overwritten,
                &HashMap::from([("IMAGE_USER".to_string(), "user3".to_string())]),
                &HashMap::new(),
                &HashMap::new(),
                None,
            )
            .as_deref(),
            Some("user3")
        );

        let env_after_arg = extract_dockerfile(
            r#"
FROM debian
ARG USERNAME=user1
ENV USERNAME=user2
USER ${USERNAME}
"#,
        );
        assert_eq!(
            find_user_statement(
                &env_after_arg,
                &HashMap::new(),
                &HashMap::new(),
                &HashMap::new(),
                None,
            )
            .as_deref(),
            Some("user2")
        );

        let inherited_env_not_arg = extract_dockerfile(
            r#"
FROM debian as one
ENV USERNAME=user1
ARG USERNAME=user2

FROM one as two
USER ${USERNAME}
"#,
        );
        assert_eq!(
            find_user_statement(
                &inherited_env_not_arg,
                &HashMap::new(),
                &HashMap::new(),
                &HashMap::new(),
                None,
            )
            .as_deref(),
            Some("user1")
        );

        let base_env = extract_dockerfile(
            r#"
FROM mybase
USER ${USERNAME}
"#,
        );
        assert_eq!(
            find_user_statement(
                &base_env,
                &HashMap::new(),
                &HashMap::from([("USERNAME".to_string(), "user1".to_string())]),
                &HashMap::new(),
                None,
            )
            .as_deref(),
            Some("user1")
        );
    }

    #[test]
    fn detects_build_context_support_from_syntax_directives() {
        assert_eq!(
            supports_build_contexts(&extract_dockerfile("FROM debian")),
            BuildContextSupport::Unsupported
        );
        assert_eq!(
            supports_build_contexts(&extract_dockerfile(
                "# syntax=docker/dockerfile:1.4\nFROM debian"
            )),
            BuildContextSupport::Supported
        );
        assert_eq!(
            supports_build_contexts(&extract_dockerfile(
                "# syntax=docker.io/docker/dockerfile:1.2\nFROM debian"
            )),
            BuildContextSupport::Unsupported
        );
        assert_eq!(
            supports_build_contexts(&extract_dockerfile(
                "# syntax=docker.io/docker/dockerfile:latest\nFROM debian"
            )),
            BuildContextSupport::Supported
        );
        assert_eq!(
            supports_build_contexts(&extract_dockerfile(
                "# syntax=mycompany/myimage:1.4\nFROM debian"
            )),
            BuildContextSupport::Unknown
        );
    }
}
