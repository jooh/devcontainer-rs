//! JSONC parsing helpers for devcontainer configuration files.

use serde_json::Value;

fn strip_jsonc_comments(text: &str) -> String {
    let mut result = String::new();
    let mut chars = text.chars().peekable();
    let mut in_string = false;
    let mut escaped = false;
    let mut in_line_comment = false;
    let mut in_block_comment = false;

    while let Some(current) = chars.next() {
        let next = chars.peek().copied();

        if in_line_comment {
            if current == '\n' {
                in_line_comment = false;
                result.push(current);
            }
            continue;
        }

        if in_block_comment {
            if current == '*' && next == Some('/') {
                chars.next();
                in_block_comment = false;
            }
            continue;
        }

        if in_string {
            result.push(current);
            if escaped {
                escaped = false;
            } else if current == '\\' {
                escaped = true;
            } else if current == '"' {
                in_string = false;
            }
            continue;
        }

        if current == '"' {
            in_string = true;
            result.push(current);
            continue;
        }

        if current == '/' && next == Some('/') {
            chars.next();
            in_line_comment = true;
            continue;
        }

        if current == '/' && next == Some('*') {
            chars.next();
            in_block_comment = true;
            continue;
        }

        result.push(current);
    }

    result
}

fn strip_trailing_commas(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let characters: Vec<char> = text.chars().collect();
    let mut index = 0;
    let mut in_string = false;
    let mut escaped = false;

    while index < characters.len() {
        let current = characters[index];

        if in_string {
            result.push(current);
            if escaped {
                escaped = false;
            } else if current == '\\' {
                escaped = true;
            } else if current == '"' {
                in_string = false;
            }
            index += 1;
            continue;
        }

        if current == '"' {
            in_string = true;
            result.push(current);
            index += 1;
            continue;
        }

        if current == ',' {
            let mut lookahead = index + 1;
            while lookahead < characters.len() && characters[lookahead].is_whitespace() {
                lookahead += 1;
            }

            if lookahead < characters.len()
                && (characters[lookahead] == '}' || characters[lookahead] == ']')
            {
                index += 1;
                continue;
            }
        }

        result.push(current);
        index += 1;
    }

    result
}

pub fn parse_jsonc_value(text: &str) -> Result<Value, String> {
    let sanitized = strip_trailing_commas(&strip_jsonc_comments(text));
    serde_json::from_str(&sanitized).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{parse_jsonc_value, strip_jsonc_comments};

    #[test]
    fn strips_block_comments_and_preserves_comment_like_strings() {
        let parsed = parse_jsonc_value(
            r#"{
                /* block comment */
                "url": "https://example.com/a//b",
                "escaped": "quote: \" // still string",
                "values": [
                    1,
                    2,
                ],
            }"#,
        )
        .expect("jsonc");

        assert_eq!(
            parsed,
            json!({
                "url": "https://example.com/a//b",
                "escaped": "quote: \" // still string",
                "values": [1, 2]
            })
        );
    }

    #[test]
    fn unterminated_block_comment_consumes_the_remainder() {
        assert_eq!(
            strip_jsonc_comments(r#"{"keep": true} /* unfinished"#),
            r#"{"keep": true} "#
        );
    }
}
