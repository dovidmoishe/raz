use serde_json::Value;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StreamDelta {
    pub session_id: Option<String>,
    pub status: Option<String>,
    pub result: Option<String>,
    pub error: Option<String>,
    pub done: bool,
}

pub fn parse_claude_line(line: &str) -> Option<StreamDelta> {
    let v: Value = serde_json::from_str(line.trim()).ok()?;
    let kind = v.get("type")?.as_str()?;
    let mut delta = StreamDelta {
        session_id: string_field(&v, "session_id"),
        ..StreamDelta::default()
    };
    match kind {
        "system" => {
            if v.get("subtype").and_then(Value::as_str) == Some("init") {
                delta.status = Some("started".into());
            }
        }
        "assistant" => {
            delta.status = assistant_status(&v);
        }
        "result" => {
            delta.result = v
                .get("result")
                .and_then(Value::as_str)
                .map(str::to_string)
                .filter(|s| !s.is_empty());
            delta.done = true;
            if v.get("is_error").and_then(Value::as_bool) == Some(true) {
                delta.error = delta.result.clone().or_else(|| Some("claude reported an error".into()));
            }
        }
        _ => {}
    }
    Some(delta)
}

pub fn parse_codex_line(line: &str) -> Option<StreamDelta> {
    let v: Value = serde_json::from_str(line.trim()).ok()?;
    let kind = v.get("type")?.as_str()?;
    let mut delta = StreamDelta::default();
    match kind {
        "thread.started" => {
            delta.session_id = string_field(&v, "thread_id");
            delta.status = Some("started".into());
        }
        "item.started" => {
            delta.status = item_status(v.get("item"));
        }
        "item.completed" => {
            if let Some(item) = v.get("item") {
                if item.get("type").and_then(Value::as_str) == Some("agent_message") {
                    delta.result = item
                        .get("text")
                        .and_then(Value::as_str)
                        .map(str::to_string);
                }
                delta.status = item_status(Some(item));
            }
        }
        "turn.completed" => delta.done = true,
        "turn.failed" | "error" => {
            delta.done = true;
            delta.error = string_field(&v, "message")
                .or_else(|| {
                    v.get("error")
                        .and_then(|e| string_field(e, "message"))
                })
                .or_else(|| Some("codex reported an error".into()));
        }
        _ => {}
    }
    Some(delta)
}

pub fn find_github_pr(text: &str) -> Option<String> {
    let mut found = None;
    let mut rest = text;
    while let Some(at) = rest.find("https://github.com/") {
        let slice = &rest[at..];
        let url: String = slice
            .chars()
            .take_while(|c| c.is_ascii_graphic() && *c != '>')
            .collect();
        let url = url.trim_end_matches(['.', ',', ';', ')', ']', '"', '\'']);
        if url.contains("/pull/") {
            found = Some(url.to_string());
        }
        rest = &rest[at + 19..];
    }
    found
}

fn string_field(v: &Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn assistant_status(v: &Value) -> Option<String> {
    let blocks = v
        .pointer("/message/content")
        .or_else(|| v.get("content"))
        .and_then(Value::as_array)?;
    for block in blocks {
        match block.get("type").and_then(Value::as_str) {
            Some("tool_use") => {
                if let Some(name) = block.get("name").and_then(Value::as_str) {
                    return Some(name.to_string());
                }
            }
            Some("text") => {
                if let Some(text) = block.get("text").and_then(Value::as_str) {
                    let line = text.lines().find(|l| !l.trim().is_empty())?;
                    return Some(line.trim().to_string());
                }
            }
            _ => {}
        }
    }
    None
}

fn item_status(item: Option<&Value>) -> Option<String> {
    let item = item?;
    match item.get("type").and_then(Value::as_str) {
        Some("command_execution") => item
            .get("command")
            .and_then(Value::as_str)
            .map(|c| clip_cmd(c)),
        Some("agent_message") => Some("working".into()),
        Some(other) => Some(other.replace('_', " ")),
        None => None,
    }
}

fn clip_cmd(cmd: &str) -> String {
    let first = cmd.split_whitespace().next().unwrap_or(cmd);
    if first.len() > 40 {
        format!("{}…", &first[..40])
    } else {
        first.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_session_and_result() {
        let init = parse_claude_line(
            r#"{"type":"system","subtype":"init","session_id":"cla-1"}"#,
        )
        .unwrap();
        assert_eq!(init.session_id.as_deref(), Some("cla-1"));
        let tool = parse_claude_line(
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash"}]}}"#,
        )
        .unwrap();
        assert_eq!(tool.status.as_deref(), Some("Bash"));
        let result = parse_claude_line(
            r#"{"type":"result","session_id":"cla-1","result":"shipped","is_error":false}"#,
        )
        .unwrap();
        assert!(result.done);
        assert_eq!(result.result.as_deref(), Some("shipped"));
    }

    #[test]
    fn codex_thread_and_message() {
        let start = parse_codex_line(
            r#"{"type":"thread.started","thread_id":"thr-9"}"#,
        )
        .unwrap();
        assert_eq!(start.session_id.as_deref(), Some("thr-9"));
        let msg = parse_codex_line(
            r#"{"type":"item.completed","item":{"type":"agent_message","text":"looks good"}}"#,
        )
        .unwrap();
        assert_eq!(msg.result.as_deref(), Some("looks good"));
        let done = parse_codex_line(r#"{"type":"turn.completed"}"#).unwrap();
        assert!(done.done);
    }

    #[test]
    fn finds_last_pull_url() {
        let text = "opened https://github.com/acme/shop/pull/3 and then https://github.com/acme/shop/pull/4.";
        assert_eq!(
            find_github_pr(text).as_deref(),
            Some("https://github.com/acme/shop/pull/4")
        );
        assert!(find_github_pr("no pr here").is_none());
    }
}
