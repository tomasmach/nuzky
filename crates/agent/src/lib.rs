//! Runs the user's own installed agent for one turn of Nuzky's AI panel. Claude Code runs as
//! `claude -p`, Codex as `codex exec --json`, each with its built-in tools off and Nuzky's MCP
//! server as its only tools; one process serves one message and the next continues the
//! conversation with the agent's own resume. See docs/AI-ARCHITECTURE.md, "AI panel".

mod claude;
mod codex;
mod discover;
mod event;
mod turn;

pub use discover::{AgentId, AgentInfo, find};
pub use event::{AgentEvent, ErrorCode, ToolStatus};
pub use turn::{McpServer, Turn, TurnRequest};
