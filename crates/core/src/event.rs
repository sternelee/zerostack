use compact_str::CompactString;

#[derive(Debug, Clone)]
pub enum AgentEvent {
    Token(CompactString),
    Reasoning(CompactString),
    ToolCall {
        name: CompactString,
        args: serde_json::Value,
    },
    ToolResult {
        name: CompactString,
        output: CompactString,
    },
    SubagentToolCall {
        name: CompactString,
        args: serde_json::Value,
    },
    Error(CompactString),
    Retrying {
        attempt: usize,
        max: usize,
    },
    CompletionCall {
        input_tokens: u64,
        output_tokens: u64,
        cached_input_tokens: u64,
        cache_creation_input_tokens: u64,
    },
    Done {
        response: CompactString,
        input_tokens: u64,
        output_tokens: u64,
        cached_input_tokens: u64,
        cache_creation_input_tokens: u64,
    },
}

#[derive(Debug, Clone)]
pub enum BtwEvent {
    Done {
        id: u32,
        response: CompactString,
        input_tokens: u64,
        output_tokens: u64,
        cached_input_tokens: u64,
        cache_creation_input_tokens: u64,
    },
    Error {
        id: u32,
        message: CompactString,
    },
}
