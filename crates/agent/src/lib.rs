//! Runs the user's own installed agent for one turn of CapOpen's AI panel. Claude Code runs as
//! `claude -p` with its built-in tools off and CapOpen's MCP server as its only tools; one
//! process serves one message and the next continues the conversation with `--resume`.
//! See docs/AI-ARCHITECTURE.md, "AI panel".

mod claude;
mod discover;
mod event;
mod turn;

pub use discover::{AgentId, AgentInfo, find};
pub use event::{AgentEvent, ErrorCode, ToolStatus};
pub use turn::{McpServer, Turn, TurnRequest};
