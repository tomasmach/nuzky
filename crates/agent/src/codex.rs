//! Codex as `codex exec --json`: the command and a parser of its events.

use crate::event::{AgentEvent, ErrorCode, ToolStatus};
use crate::turn::{SYSTEM, Step, TurnRequest};
use serde_json::Value;
use std::process::Command;

/// Everything that would give Codex a tool besides Nuzky's. `-c` rather than `--disable`,
/// since Codex refuses to start on a feature name it no longer knows but only warns about a setting.
const OFF: &[&str] = &[
    "sandbox_mode=\"read-only\"",
    "approval_policy=\"never\"",
    "web_search=\"disabled\"",
    "agents.enabled=false",
    "skills.include_instructions=false",
    "features.shell_tool=false",
    "features.apps=false",
    "features.plugins=false",
    "features.hooks=false",
    "features.multi_agent=false",
    "features.image_generation=false",
    "features.view_image=false",
    "features.tool_suggest=false",
    "features.sleep_tool=false",
    "features.goals=false",
    "features.browser_use=false",
    "features.computer_use=false",
    // Current models call tools from JavaScript in `exec`, and Codex reports none of those calls;
    // offered directly, every Nuzky call shows in the panel.
    "features.code_mode.direct_only_tool_namespaces=[\"mcp__nuzky\"]",
];

/// A TOML string, which a JSON string also is.
fn string(s: &str) -> String {
    Value::from(s).to_string()
}

pub(crate) fn command(req: &TurnRequest) -> Command {
    let mut c = Command::new(&req.exe);
    c.arg("exec");
    if req.resume {
        c.arg("resume");
    }
    // The user's login stays; their config.toml, with its MCP servers, plugins and rules, does not.
    c.args(["--json", "--ignore-user-config", "--ignore-rules", "--skip-git-repo-check"]);
    let mut settings: Vec<String> = OFF.iter().map(|s| s.to_string()).collect();
    settings.push(format!("mcp_servers.nuzky.command={}", string(&req.mcp.command.to_string_lossy())));
    settings.push(format!("mcp_servers.nuzky.args={}", Value::from(req.mcp.args.clone())));
    settings.extend(req.mcp.env.iter().map(|(k, v)| format!("mcp_servers.nuzky.env.{k}={}", string(v))));
    settings.push("mcp_servers.nuzky.required=true".into());
    settings.push("mcp_servers.nuzky.default_tools_approval_mode=\"approve\"".into());
    settings.push(format!("developer_instructions={}", string(SYSTEM)));
    for setting in &settings {
        c.args(["-c", setting]);
    }
    if req.resume {
        c.arg(&req.session);
    }
    // The message comes on stdin.
    c.arg("-");
    c
}

pub(crate) struct Parser {
    /// Codex numbers items from item_0 in every process; this keeps a later message's ids apart.
    turn: u32,
    thread: Option<String>,
    /// Whether Codex has kept the conversation: a turn that failed before anything happened cannot be resumed.
    kept: bool,
}

/// What a Codex error means for the user.
fn code(message: &str) -> ErrorCode {
    if message.contains("401 Unauthorized") || message.contains("sign in again") {
        ErrorCode::NotSignedIn
    } else if message.contains("usage limit") || message.contains("spend cap") {
        ErrorCode::UsageLimit
    } else {
        ErrorCode::AgentFailed
    }
}

impl Parser {
    pub(crate) fn new(turn: u32) -> Parser {
        Parser { turn, thread: None, kept: false }
    }

    /// The conversation the next message resumes, once Codex has kept it.
    pub(crate) fn session(&self) -> Option<String> {
        self.thread.clone().filter(|_| self.kept)
    }

    pub(crate) fn line(&mut self, line: &str) -> Vec<Step> {
        let Ok(v) = serde_json::from_str::<Value>(line) else { return Vec::new() };
        match v["type"].as_str() {
            Some("thread.started") => {
                self.thread = v["thread_id"].as_str().map(str::to_owned);
                Vec::new()
            }
            Some("item.started" | "item.updated" | "item.completed") => {
                self.item(&v["item"], v["type"] == "item.completed").into_iter().collect()
            }
            Some("turn.completed") => {
                self.kept = true;
                Vec::new()
            }
            // Codex retries other errors a few times before it gives up with turn.failed. Without
            // any login it would retry for half a minute.
            Some("error") => {
                let message = v["message"].as_str().unwrap_or("");
                if message.contains("Missing bearer") {
                    vec![Step::Fail(ErrorCode::NotSignedIn, message.to_owned())]
                } else {
                    Vec::new()
                }
            }
            Some("turn.failed") => {
                let message = v["error"]["message"].as_str().unwrap_or("").to_owned();
                vec![match code(&message) {
                    ErrorCode::AgentFailed if message.is_empty() => {
                        Step::ResultError("Codex stopped with an error.".into())
                    }
                    ErrorCode::AgentFailed => Step::ResultError(message),
                    code => Step::Fail(code, message),
                }]
            }
            _ => Vec::new(),
        }
    }

    fn item(&mut self, item: &Value, completed: bool) -> Option<Step> {
        match item["type"].as_str() {
            Some("agent_message") if completed => {
                self.kept = true;
                Some(Step::Event(AgentEvent::Text { text: item["text"].as_str().unwrap_or("").to_owned() }))
            }
            Some("mcp_tool_call") if item["server"] == "nuzky" => {
                self.kept |= completed;
                let status = match item["status"].as_str() {
                    Some("completed") => ToolStatus::Done,
                    Some("failed") => ToolStatus::Failed,
                    _ => ToolStatus::Running,
                };
                Some(Step::Event(AgentEvent::Tool {
                    id: format!("{}@{}", item["id"].as_str().unwrap_or(""), self.turn),
                    name: format!("nuzky.{}", item["tool"].as_str().unwrap_or("")),
                    status,
                    input: (status == ToolStatus::Running).then(|| item["arguments"].clone()),
                }))
            }
            Some("agent_message" | "reasoning" | "todo_list" | "error") | None => None,
            // A shell command, a file change, a web search, another agent or another server's tool.
            Some(kind) => {
                let what = item["server"]
                    .as_str()
                    .map_or(kind.to_owned(), |s| format!("{s}.{}", item["tool"].as_str().unwrap_or("")));
                Some(Step::Fail(
                    ErrorCode::AgentFailed,
                    format!(
                        "Codex used a tool Nuzky did not give it ({what}), so Nuzky stopped it. Update Codex and Nuzky."
                    ),
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::turn::McpServer;
    use serde_json::json;
    use std::path::PathBuf;

    type Outcome = Result<AgentEvent, (ErrorCode, String)>;

    fn run(lines: &[&str]) -> (Vec<Outcome>, Option<String>) {
        let mut p = Parser::new(7);
        let steps = lines
            .iter()
            .flat_map(|l| p.line(l))
            .map(|s| match s {
                Step::Event(e) => Ok(e),
                Step::Fail(c, m) => Err((c, m)),
                Step::ResultError(m) => Err((ErrorCode::AgentFailed, m)),
            })
            .collect();
        (steps, p.session())
    }

    // Lines from real `codex exec --json` turns of codex-cli 0.162.1, with Nuzky's settings and a stand-in MCP server.
    #[test]
    fn a_turn_names_nuzky_calls_and_keeps_the_conversation() {
        let (events, session) = run(&[
            r#"{"type":"thread.started","thread_id":"01a12779-e1a0-7391-8130-1242bc8fb459"}"#,
            r#"{"type":"item.completed","item":{"id":"item_0","type":"error","message":"Codex is ignoring 1 unrecognized configuration setting. Check for typos or deprecated settings.\n  session-flags: `features.goals` is ignored."}}"#,
            r#"{"type":"turn.started"}"#,
            r#"{"type":"item.completed","item":{"id":"item_2","type":"agent_message","text":"Zavolám postupně oba nástroje.\n"}}"#,
            r#"{"type":"item.started","item":{"id":"item_3","type":"mcp_tool_call","server":"nuzky","tool":"get_state","arguments":{},"result":null,"error":null,"status":"in_progress"}}"#,
            r#"{"type":"item.completed","item":{"id":"item_3","type":"mcp_tool_call","server":"nuzky","tool":"get_state","arguments":{},"result":{"content":[{"type":"text","text":"{\"duration_us\": 12000000, \"tracks\": 1}"}],"structured_content":{"duration_us":12000000,"tracks":1}},"error":null,"status":"completed"}}"#,
            r#"{"type":"item.started","item":{"id":"item_4","type":"mcp_tool_call","server":"nuzky","tool":"fail","arguments":{},"result":null,"error":null,"status":"in_progress"}}"#,
            r#"{"type":"item.completed","item":{"id":"item_4","type":"mcp_tool_call","server":"nuzky","tool":"fail","arguments":{},"result":{"content":[{"type":"text","text":"INVALID_RUN: no such run"}],"structured_content":null},"error":null,"status":"failed"}}"#,
            r#"{"type":"item.completed","item":{"id":"item_5","type":"agent_message","text":"Hotovo."}}"#,
            r#"{"type":"turn.completed","usage":{"input_tokens":40990,"cached_input_tokens":29312,"cache_write_input_tokens":0,"output_tokens":60,"reasoning_output_tokens":0}}"#,
        ]);
        let tool = |id: &str, name: &str, status, input| {
            Ok(AgentEvent::Tool { id: format!("{id}@7"), name: format!("nuzky.{name}"), status, input })
        };
        assert_eq!(
            events,
            [
                Ok(AgentEvent::Text { text: "Zavolám postupně oba nástroje.\n".into() }),
                tool("item_3", "get_state", ToolStatus::Running, Some(json!({}))),
                tool("item_3", "get_state", ToolStatus::Done, None),
                tool("item_4", "fail", ToolStatus::Running, Some(json!({}))),
                tool("item_4", "fail", ToolStatus::Failed, None),
                Ok(AgentEvent::Text { text: "Hotovo.".into() }),
            ]
        );
        assert_eq!(session.as_deref(), Some("01a12779-e1a0-7391-8130-1242bc8fb459"));
    }

    #[test]
    fn a_turn_stopped_after_its_first_words_can_be_resumed_and_one_that_never_started_cannot() {
        let started = r#"{"type":"thread.started","thread_id":"01a1277b-38cc-7d11-bfe8-468498c08779"}"#;
        let (_, session) = run(&[
            started,
            r#"{"type":"turn.started"}"#,
            r#"{"type":"item.completed","item":{"id":"item_0","type":"agent_message","text":"Zapamatuju si KOSTKA a spustím nástroj slow.\n"}}"#,
            r#"{"type":"item.started","item":{"id":"item_1","type":"mcp_tool_call","server":"nuzky","tool":"slow","arguments":{},"result":null,"error":null,"status":"in_progress"}}"#,
        ]);
        assert_eq!(session.as_deref(), Some("01a1277b-38cc-7d11-bfe8-468498c08779"));
        assert_eq!(run(&[started, r#"{"type":"turn.started"}"#]).1, None);
    }

    #[test]
    fn no_login_fails_at_the_first_retry_and_a_used_up_plan_says_so() {
        let (events, session) = run(&[
            r#"{"type":"thread.started","thread_id":"01a1277b-8952-7f32-92a7-9fb0e9a54be2"}"#,
            r#"{"type":"turn.started"}"#,
            r#"{"type":"error","message":"Reconnecting... 2/5 (unexpected status 401 Unauthorized: Missing bearer or basic authentication in header, url: wss://api.openai.com/v1/responses, cf-ray: a48856e4990cf32a-PRG)"}"#,
        ]);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].as_ref().unwrap_err().0, ErrorCode::NotSignedIn);
        assert_eq!(session, None);
        // A login that no longer works gives up after its retries.
        let expired = r#"{"type":"turn.failed","error":{"message":"Your access token could not be refreshed because your refresh token has expired. Please log out and sign in again."}}"#;
        assert_eq!(run(&[expired]).0[0].as_ref().unwrap_err().0, ErrorCode::NotSignedIn);
        let limit = r#"{"type":"turn.failed","error":{"message":"You’ve hit your usage limit. Upgrade to Pro (https://chatgpt.com/explore/pro), visit https://chatgpt.com/settings/usage to purchase more credits or try again at 9:00 PM."}}"#;
        assert_eq!(run(&[limit]).0[0].as_ref().unwrap_err().0, ErrorCode::UsageLimit);
        let other = r#"{"type":"turn.failed","error":{"message":"stream disconnected before completion"}}"#;
        assert_eq!(run(&[other]).0, [Err((ErrorCode::AgentFailed, "stream disconnected before completion".into()))]);
    }

    #[test]
    fn any_tool_but_nuzkys_fails_the_turn() {
        for item in [
            json!({"id":"item_1","type":"command_execution","command":"bash -lc ls","aggregated_output":"","exit_code":null,"status":"in_progress"}),
            json!({"id":"item_1","type":"file_change","changes":[{"path":"x.txt","kind":"add"}],"status":"completed"}),
            json!({"id":"item_1","type":"mcp_tool_call","server":"ploi","tool":"deploy_site","arguments":{},"result":null,"error":null,"status":"in_progress"}),
        ] {
            let line = json!({"type":"item.started","item":item}).to_string();
            let (events, _) = run(&[&line]);
            assert_eq!(events[0].as_ref().unwrap_err().0, ErrorCode::AgentFailed, "{line}");
        }
    }

    #[test]
    fn the_command_gives_codex_only_nuzkys_tools_and_resumes_by_its_id() {
        let req = TurnRequest {
            agent: crate::AgentId::Codex,
            exe: "codex".into(),
            session: "01a12779-e1a0-7391-8130-1242bc8fb459".into(),
            resume: true,
            prompt: String::new(),
            cwd: PathBuf::new(),
            mcp: McpServer {
                command: "/opt/Nuzky \"app\"".into(),
                args: vec!["mcp".into(), "--project".into(), "/home/u/a b.nuzky".into()],
                env: vec![("XDG_RUNTIME_DIR".into(), "/run/user/1000".into())],
            },
        };
        let args: Vec<String> = command(&req).get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        assert_eq!(args[..3], ["exec", "resume", "--json"]);
        assert_eq!(args[args.len() - 2..], ["01a12779-e1a0-7391-8130-1242bc8fb459", "-"]);
        assert!(args.contains(&"--ignore-user-config".into()) && args.contains(&"features.shell_tool=false".into()));
        // Each setting is TOML that reads back as what Nuzky meant, as Codex parses it.
        let setting = |key: &str| {
            let value = args.iter().find_map(|a| a.strip_prefix(&format!("{key}="))).unwrap();
            serde_json::to_value(&toml::from_str::<toml::Table>(&format!("v = {value}")).unwrap()["v"]).unwrap()
        };
        assert_eq!(setting("mcp_servers.nuzky.command"), json!("/opt/Nuzky \"app\""));
        assert_eq!(setting("mcp_servers.nuzky.args"), json!(["mcp", "--project", "/home/u/a b.nuzky"]));
        assert_eq!(setting("mcp_servers.nuzky.env.XDG_RUNTIME_DIR"), json!("/run/user/1000"));
        assert_eq!(setting("developer_instructions"), json!(SYSTEM));
    }
}
