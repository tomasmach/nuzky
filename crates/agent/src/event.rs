use serde::Serialize;
use serde_json::Value;

/// What the panel shows of a turn, in order. A turn ends with exactly one `Done` or `Error`.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum AgentEvent {
    /// Words of the agent's answer, appended as they stream in.
    Text {
        text: String,
    },
    /// A tool call, first as running (with its arguments once known), then done or failed.
    Tool {
        id: String,
        name: String,
        status: ToolStatus,
        #[serde(skip_serializing_if = "Option::is_none")]
        input: Option<Value>,
    },
    Done {
        stopped: bool,
    },
    Error {
        code: ErrorCode,
        message: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ToolStatus {
    Running,
    Done,
    Failed,
}

/// Prefixes the panel reads to say what to do next.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    NotSignedIn,
    UsageLimit,
    AgentFailed,
}
