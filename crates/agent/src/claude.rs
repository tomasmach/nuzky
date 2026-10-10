//! Claude Code as `claude -p` with stream-json output: the command and a parser of its lines.

use crate::event::{AgentEvent, ErrorCode, ToolStatus};
use crate::turn::{SYSTEM, Step, TurnRequest};
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::process::Command;

/// Settings Nuzky needs; `outputStyle` doubles as a check that Claude applied them at all,
/// since it silently ignores a settings value it does not accept.
fn settings() -> Value {
    json!({"disableAllHooks": true, "outputStyle": "default", "autoMemoryEnabled": false})
}

pub(crate) fn command(req: &TurnRequest) -> Command {
    let env: Map<String, Value> = req.mcp.env.iter().map(|(k, v)| (k.clone(), Value::String(v.clone()))).collect();
    let mcp = json!({"mcpServers": {"nuzky": {"type": "stdio", "command": req.mcp.command, "args": req.mcp.args, "env": env}}});
    let mut c = Command::new(&req.exe);
    c.args(["-p", "--output-format", "stream-json", "--verbose", "--include-partial-messages"])
        // The user's login, model and gateway stay; the folder's settings, hooks and MCP servers do not.
        .args(["--setting-sources", "user", "--settings", &settings().to_string()])
        .args(["--strict-mcp-config", "--mcp-config", &mcp.to_string()])
        // No shell, files or web: only Nuzky's tools, allowed without asking.
        .args(["--tools", "", "--allowedTools", "mcp__nuzky", "--permission-mode", "dontAsk"])
        .args(["--disable-slash-commands", "--append-system-prompt", SYSTEM])
        .args([if req.resume { "--resume" } else { "--session-id" }, &req.session]);
    c
}

#[derive(Default)]
pub(crate) struct Parser {
    /// Text of the current message already streamed in pieces, so its full copy is skipped.
    streamed: bool,
    /// Tool names by call id; results carry only the id.
    tools: HashMap<String, String>,
}

fn text_of(content: &Value) -> String {
    content.as_array().into_iter().flatten().filter_map(|c| c["text"].as_str()).collect::<Vec<_>>().join("\n")
}

fn tool(id: &str, name: &str, status: ToolStatus, input: Option<Value>) -> Step {
    Step::Event(AgentEvent::Tool { id: id.to_owned(), name: name.to_owned(), status, input })
}

impl Parser {
    pub(crate) fn line(&mut self, line: &str) -> Vec<Step> {
        let Ok(v) = serde_json::from_str::<Value>(line) else { return Vec::new() };
        // Work of a subagent is not the panel's to show; none are available anyway.
        if !v["parent_tool_use_id"].is_null() {
            return Vec::new();
        }
        match v["type"].as_str() {
            Some("system") if v["subtype"] == "init" => init(&v).into_iter().collect(),
            Some("stream_event") => {
                let e = &v["event"];
                match e["type"].as_str() {
                    Some("message_start") => {
                        self.streamed = false;
                        Vec::new()
                    }
                    Some("content_block_start") if e["content_block"]["type"] == "tool_use" => {
                        let (id, name) = (
                            e["content_block"]["id"].as_str().unwrap_or(""),
                            e["content_block"]["name"].as_str().unwrap_or(""),
                        );
                        self.tools.insert(id.to_owned(), name.to_owned());
                        vec![tool(id, name, ToolStatus::Running, None)]
                    }
                    Some("content_block_delta") if e["delta"]["type"] == "text_delta" => {
                        self.streamed = true;
                        vec![Step::Event(AgentEvent::Text {
                            text: e["delta"]["text"].as_str().unwrap_or("").to_owned(),
                        })]
                    }
                    _ => Vec::new(),
                }
            }
            Some("assistant") => {
                let content = &v["message"]["content"];
                if let Some(error) = v["error"].as_str() {
                    let text = text_of(content);
                    return vec![match error {
                        "authentication_failed" | "oauth_org_not_allowed" => Step::Fail(ErrorCode::NotSignedIn, text),
                        "rate_limit" | "billing_error" => Step::Fail(ErrorCode::UsageLimit, text),
                        _ => Step::Fail(ErrorCode::AgentFailed, if text.is_empty() { error.to_owned() } else { text }),
                    }];
                }
                let mut out = Vec::new();
                for c in content.as_array().into_iter().flatten() {
                    match c["type"].as_str() {
                        Some("tool_use") => {
                            let (id, name) = (c["id"].as_str().unwrap_or(""), c["name"].as_str().unwrap_or(""));
                            self.tools.insert(id.to_owned(), name.to_owned());
                            out.push(tool(id, name, ToolStatus::Running, Some(c["input"].clone())));
                        }
                        Some("text") if !self.streamed => out
                            .push(Step::Event(AgentEvent::Text { text: c["text"].as_str().unwrap_or("").to_owned() })),
                        _ => {}
                    }
                }
                out
            }
            Some("user") => v["message"]["content"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|c| c["type"] == "tool_result")
                .map(|c| {
                    let id = c["tool_use_id"].as_str().unwrap_or("");
                    let name = self.tools.get(id).cloned().unwrap_or_default();
                    tool(id, &name, if c["is_error"] == true { ToolStatus::Failed } else { ToolStatus::Done }, None)
                })
                .collect(),
            Some("result") if v["is_error"] == true => {
                let text = v["result"].as_str().unwrap_or("").to_owned();
                vec![Step::ResultError(if text.is_empty() {
                    "Claude Code stopped with an error.".into()
                } else {
                    text
                })]
            }
            _ => Vec::new(),
        }
    }
}

/// Fails closed unless Claude runs with Nuzky's settings and nothing but Nuzky's tools.
fn init(v: &Value) -> Option<Step> {
    let foreign =
        v["tools"].as_array().into_iter().flatten().filter_map(Value::as_str).find(|t| !t.starts_with("mcp__nuzky__"));
    if let Some(tool) = foreign {
        return Some(Step::Fail(
            ErrorCode::AgentFailed,
            format!(
                "Claude Code offered a tool Nuzky did not give it ({tool}), so Nuzky stopped it. Update Claude Code and Nuzky."
            ),
        ));
    }
    let nuzky = v["mcp_servers"].as_array().into_iter().flatten().find(|s| s["name"] == "nuzky");
    if nuzky.is_none_or(|s| s["status"] != "connected") {
        return Some(Step::Fail(
            ErrorCode::AgentFailed,
            "Claude Code could not reach Nuzky's tools. Try again; if it keeps failing, restart Nuzky.".into(),
        ));
    }
    if v.get("output_style").is_some_and(|s| s != "default") {
        return Some(Step::Fail(
            ErrorCode::AgentFailed,
            "Claude Code ignored Nuzky's settings, so Nuzky stopped it. Update Claude Code and try again.".into(),
        ));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn events(lines: &[Value]) -> Vec<Result<AgentEvent, (ErrorCode, String)>> {
        let mut p = Parser::default();
        lines
            .iter()
            .flat_map(|l| p.line(&l.to_string()))
            .map(|s| match s {
                Step::Event(e) => Ok(e),
                Step::Fail(c, m) => Err((c, m)),
                Step::ResultError(m) => Err((ErrorCode::AgentFailed, m)),
            })
            .collect()
    }

    fn init_line(tools: &[&str], style: &str) -> Value {
        json!({"type":"system","subtype":"init","tools":tools,"mcp_servers":[{"name":"nuzky","status":"connected"}],"output_style":style})
    }

    // Shapes from a real claude 2.1.292 turn (docs/research/2026-10-08-acp-spike.md and the panel transport proposals).
    #[test]
    fn a_turn_streams_text_once_and_names_tool_results() {
        let lines = [
            init_line(&["mcp__nuzky__get_state"], "default"),
            json!({"type":"stream_event","parent_tool_use_id":null,"event":{"type":"message_start"}}),
            json!({"type":"stream_event","parent_tool_use_id":null,"event":{"type":"content_block_start","content_block":{"type":"tool_use","id":"t1","name":"mcp__nuzky__get_state","input":{}}}}),
            json!({"type":"assistant","parent_tool_use_id":null,"message":{"content":[{"type":"tool_use","id":"t1","name":"mcp__nuzky__get_state","input":{"range":null}}]}}),
            json!({"type":"user","parent_tool_use_id":null,"message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":[{"type":"text","text":"{}"}]}]}}),
            json!({"type":"stream_event","parent_tool_use_id":null,"event":{"type":"message_start"}}),
            json!({"type":"stream_event","parent_tool_use_id":null,"event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"Hoto"}}}),
            json!({"type":"stream_event","parent_tool_use_id":null,"event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"vo."}}}),
            json!({"type":"assistant","parent_tool_use_id":null,"message":{"content":[{"type":"text","text":"Hotovo."}]}}),
            json!({"type":"result","subtype":"success","is_error":false,"result":"Hotovo."}),
        ];
        let get_state = |status, input| {
            Ok(AgentEvent::Tool { id: "t1".into(), name: "mcp__nuzky__get_state".into(), status, input })
        };
        assert_eq!(
            events(&lines),
            [
                get_state(ToolStatus::Running, None),
                get_state(ToolStatus::Running, Some(json!({"range":null}))),
                get_state(ToolStatus::Done, None),
                Ok(AgentEvent::Text { text: "Hoto".into() }),
                Ok(AgentEvent::Text { text: "vo.".into() }),
            ]
        );
    }

    #[test]
    fn foreign_tools_ignored_settings_and_login_problems_fail_the_turn() {
        assert_eq!(
            events(&[init_line(&["mcp__nuzky__get_state", "Bash"], "default")])[0].as_ref().unwrap_err().0,
            ErrorCode::AgentFailed
        );
        assert_eq!(
            events(&[init_line(&["mcp__nuzky__get_state"], "Proactive")])[0].as_ref().unwrap_err().0,
            ErrorCode::AgentFailed
        );
        let logged_out = json!({"type":"assistant","error":"authentication_failed","parent_tool_use_id":null,"message":{"content":[{"type":"text","text":"Not logged in · Please run /login"}]}});
        assert_eq!(events(&[logged_out]), [Err((ErrorCode::NotSignedIn, "Not logged in · Please run /login".into()))]);
        let limit = json!({"type":"assistant","error":"rate_limit","parent_tool_use_id":null,"message":{"content":[{"type":"text","text":"You've hit your limit · resets 9pm"}]}});
        assert_eq!(events(&[limit]), [Err((ErrorCode::UsageLimit, "You've hit your limit · resets 9pm".into()))]);
    }
}
