use claude_rs_cli::inline_repl::{
    CompletionState, InlineReplState, MAX_COMPLETION_ROWS, visible_width, is_exit_input,
};

#[test]
fn test_visible_width_ascii_only() {
    assert_eq!(visible_width("hello"), 5);
}

#[test]
fn test_visible_width_mixed_cjk() {
    assert_eq!(visible_width("中a"), 3);
    assert_eq!(visible_width("中文"), 4);
}

#[test]
fn test_inline_repl_state_new_defaults() {
    let state = InlineReplState::new();
    assert_eq!(state.input, "");
    assert!(state.queue.is_empty());
    assert!(!state.running);
    assert_eq!(state.output_pos, (0, 0));
}

#[test]
fn test_inline_repl_state_running_toggle() {
    let mut state = InlineReplState::new();
    state.running = true;
    assert!(state.running);
    state.queue.push("test".to_string());
    assert_eq!(state.queue.len(), 1);
}

#[test]
fn test_completion_refresh_prefix_match() {
    let mut c = CompletionState::new();
    c.refresh("/cl");
    assert!(c.active);
    assert_eq!(c.filtered.len(), 1);
    assert_eq!(c.filtered[0].name, "/clear");
}

#[test]
fn test_completion_refresh_substring_match() {
    let mut c = CompletionState::new();
    c.refresh("/uit");
    assert!(c.active);
    assert!(c.filtered.iter().any(|cmd| cmd.name == "/quit"));
}

#[test]
fn test_completion_refresh_no_match_dismisses() {
    let mut c = CompletionState::new();
    c.refresh("/xyz");
    assert!(!c.active);
    assert!(c.filtered.is_empty());
}

#[test]
fn test_completion_navigation_wraps() {
    let mut c = CompletionState::new();
    c.refresh("/");
    assert!(c.filtered.len() > 1);
    c.next();
    assert_eq!(c.selected, 1);
    c.prev();
    assert_eq!(c.selected, 0);
    // wrap prev from 0 -> max-1
    c.prev();
    assert_eq!(c.selected, c.filtered.len().min(MAX_COMPLETION_ROWS) - 1);
}

#[test]
fn test_completion_accept() {
    let mut c = CompletionState::new();
    c.refresh("/qu");
    assert!(c.active);
    let accepted = c.accept();
    assert_eq!(accepted, Some("/quit"));
    assert!(!c.active);
}

#[test]
fn test_is_exit_input_normal_quit_word() {
    assert!(is_exit_input("quit"));
}

#[test]
fn test_is_exit_input_boundary_trimmed_quit_word() {
    assert!(is_exit_input("  quit "));
}

#[test]
fn test_is_exit_input_error_case_uppercase_not_allowed() {
    assert!(!is_exit_input("QUIT"));
}
