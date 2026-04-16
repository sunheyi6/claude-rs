pub mod openai;
pub mod provider;
pub mod types;

pub use provider::LlmProvider;
pub use types::{
    ChatOptions, ChatResponse, Message, StopReason, StreamChunk, TokenUsage, ToolCall,
    ToolCallDelta, ToolDefinition,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_reexports_normal_message_constructor_available() {
        let msg = Message::user("hello");
        assert!(matches!(msg, Message::User { .. }));
    }

    #[test]
    fn test_reexports_boundary_chat_options_empty_model_allowed() {
        let opts = ChatOptions::new("");
        assert_eq!(opts.model, "");
    }

    #[test]
    fn test_reexports_error_case_not_applicable_enum_variant_works() {
        let reason = StopReason::Other("x".to_string());
        assert!(matches!(reason, StopReason::Other(_)));
    }
}
