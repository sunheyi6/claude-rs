use claude_rs_llm::Message;

/// Very rough token estimation: one token ≈ 4 UTF-8 bytes of English text.
/// This is fast and good enough for compaction heuristics.
pub fn estimate_tokens(msg: &Message) -> usize {
    let text = match msg {
        Message::System { content } => content.as_str(),
        Message::User { content } => content.as_str(),
        Message::Assistant { content } => content.as_str(),
        Message::Tool { content, .. } => content.as_str(),
    };
    text.len() / 4 + 20 // +20 for message overhead
}

/// Total estimated tokens for the whole conversation.
pub fn total_tokens(messages: &[Message]) -> usize {
    messages.iter().map(estimate_tokens).sum()
}

/// Compaction strategy:
/// - Keep the system prompt (first message if it is System).
/// - Keep the most recent `keep_recent` messages.
/// - Replace everything in between with a single summary message.
pub fn compact_messages(messages: &mut Vec<Message>, keep_recent: usize) {
    if messages.len() <= keep_recent + 1 {
        return;
    }

    let has_system = matches!(messages.first(), Some(Message::System { .. }));
    let system = if has_system {
        Some(messages.remove(0))
    } else {
        None
    };

    let keep_recent = keep_recent.min(messages.len());
    let dropped = messages.len() - keep_recent;
    if dropped == 0 {
        if let Some(s) = system {
            messages.insert(0, s);
        }
        return;
    }

    let summary = Message::system(format!(
        "[Context compacted: {} earlier messages were summarized and removed to stay within the context window.]",
        dropped
    ));

    // Drain the old middle messages, keep only the recent ones.
    let recent: Vec<Message> = messages.split_off(messages.len() - keep_recent);
    messages.clear();

    if let Some(s) = system {
        messages.push(s);
    }
    messages.push(summary);
    messages.extend(recent);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compact_keeps_system_and_recent() {
        let mut msgs = vec![
            Message::system("sys"),
            Message::user("1"),
            Message::user("2"),
            Message::user("3"),
            Message::user("4"),
            Message::user("5"),
        ];
        compact_messages(&mut msgs, 2);
        assert_eq!(msgs.len(), 4); // system + summary + 2 recent
        assert!(matches!(msgs[0], Message::System { .. }));
        assert!(matches!(msgs[1], Message::System { .. })); // summary
        assert!(matches!(msgs[2], Message::User { .. }));
        assert!(matches!(msgs[3], Message::User { .. }));
    }
}
