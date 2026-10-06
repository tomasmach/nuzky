use rmcp::model::{CallToolResult, ContentBlock};

pub(crate) const MAX_LINE_BYTES: usize = 64 * 1024 * 1024;
pub(crate) const MAX_TOOL_BYTES: usize = MAX_LINE_BYTES - 1024;
// Budget eight wire bytes per pixel for RGBA, PNG overhead, base64 and the JSON envelope.
pub(crate) const MAX_SHEET_PIXELS: u64 = (MAX_TOOL_BYTES / 8) as u64;

pub(crate) fn tool_result(result: CallToolResult) -> CallToolResult {
    match serde_json::to_vec(&result) {
        Ok(bytes) if bytes.len() <= MAX_TOOL_BYTES => result,
        _ => CallToolResult::error(vec![ContentBlock::text(
            "RESULT_TOO_LARGE: use fewer frames, a smaller width or a narrower query",
        )]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oversized_tool_result_is_a_tool_error() {
        let result = tool_result(CallToolResult::success(vec![ContentBlock::text(
            "x".repeat(MAX_TOOL_BYTES),
        )]));
        assert_eq!(result.is_error, Some(true));
        assert!(
            serde_json::to_string(&result)
                .unwrap()
                .contains("RESULT_TOO_LARGE")
        );
    }
}
