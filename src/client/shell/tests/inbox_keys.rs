use super::*;
use crate::agent_inbox::ConversationRecord;
use crate::client::endpoint::ClientAgentConversationsProjection;

fn state(config: Config) -> ClientShellState {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&config));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    let attachment = serde_json::json!({
        "terminal_id":"terminal-keys", "pane_id":1, "public_pane_id":"pane_1",
        "workspace_id":"ws_1", "tab_id":"tab_1", "workspace_label":"Project",
        "tab_label":"Task", "workspace_location_id":"workspace-location",
        "tab_location_id":"tab-location", "cwd":"/repo"
    });
    let record: ConversationRecord = serde_json::from_value(serde_json::json!({
        "id":"keys-conversation", "provider":"codex", "source":"hook",
        "session_ref":{"kind":"id", "value":"keys-session"}, "title":"Keyboard task",
        "saved_location":attachment, "attachments":[attachment], "disposition":"inbox",
        "created_at_ms":1, "attention_returned_at_ms":20, "completed_at_ms":20,
        "revision":1, "section":"inbox", "execution":"idle", "queued_human_work":false,
        "blocking_request":false, "failure":false, "capability_limits":[], "resume_state":"none"
    }))
    .unwrap();
    state.set_endpoint_agent_conversations_supported(&ClientEndpointId::Local, true);
    state.set_endpoint_methods_for(
        &ClientEndpointId::Local,
        Some(vec![
            "agent.conversation.open".into(),
            "agent.conversation.settle_and_close".into(),
            "agent.conversation.restore".into(),
            "command.invoke".into(),
        ]),
    );
    state.set_endpoint_agent_conversations(
        &ClientEndpointId::Local,
        None,
        ClientAgentConversationsProjection {
            boot_id: "boot-1".into(),
            revision: 1,
            total_count: 1,
            conversations: vec![record],
            next_offset: None,
            recovery_error: None,
        },
    );
    state
}

#[test]
fn inbox_raw_prefix_opens_actions_and_view_selector() {
    let mut state = state(Config::default());
    let actions = state.handle_input_bytes(b"\x02A");
    assert!(actions.requests.is_empty());
    assert!(
        matches!(state.overlay, Some(ClientShellOverlay::ContextMenu(ref menu))
        if matches!(menu.target, ClientContextMenuTarget::Conversation { .. }))
    );
    state.handle_input_bytes(b"\x1b");
    assert!(state.overlay.is_none());
    let selector = state.handle_input_bytes(b"\x02i");
    assert!(selector.requests.is_empty());
    let Some(ClientShellOverlay::ContextMenu(menu)) = state.overlay.as_ref() else {
        panic!("prefix+i must open the view selector");
    };
    let labels = menu
        .items()
        .into_iter()
        .map(|item| item.label)
        .collect::<Vec<_>>();
    assert!(labels.iter().any(|label| label.contains("Inbox")));
    assert!(labels
        .iter()
        .any(|label| label.contains("All agents by space")));
    assert!(labels
        .iter()
        .any(|label| label.contains("All agents by attention")));
}

#[test]
fn inbox_prefix_fallback_respects_custom_binding_and_prefix() {
    for (binding, input) in [("shift+a", "A"), ("i", "i")] {
        let config = toml::from_str::<Config>(&format!(
            "[keys]\nprefix = \"ctrl+x\"\nhelp = \"prefix+{binding}\"\n"
        ))
        .unwrap();
        let mut state = state(config);
        state.handle_input_bytes(format!("\x18{input}").as_bytes());
        assert!(matches!(state.overlay, Some(ClientShellOverlay::Help(_))));
    }
    let config = toml::from_str::<Config>("[keys]\nprefix = \"ctrl+x\"\n").unwrap();
    let mut state = state(config);
    state.handle_input_bytes(b"\x18i");
    assert!(matches!(
        state.overlay,
        Some(ClientShellOverlay::ContextMenu(_))
    ));
}

#[test]
fn inbox_unprefixed_action_letters_reach_the_terminal() {
    let mut state = state(Config::default());
    let outcome = state.handle_input_bytes(b"sr");
    assert!(outcome.actions.is_empty());
    assert!(state.overlay.is_none());
    let [ClientMessage::ClientShellPaneInput { pane_id, events }] = &outcome.requests[..] else {
        panic!("action letters must reach the focused terminal");
    };
    assert_eq!(pane_id, "pane_1");
    assert!(matches!(
        &events[..],
        [
            crate::protocol::ClientPaneInputEvent::Key {
                code: crate::protocol::ClientKeyCode::Char('s'),
                ..
            },
            crate::protocol::ClientPaneInputEvent::Key {
                code: crate::protocol::ClientKeyCode::Char('r'),
                ..
            },
        ]
    ));
}

#[test]
fn inbox_actions_modal_settle_shortcut_dispatches_close_method() {
    let mut state = state(Config::default());
    state.handle_input_bytes(b"\x02A");
    let outcome = state.handle_input_bytes(b"s");
    assert!(outcome.requests.is_empty());
    assert!(
        matches!(&outcome.actions[..], [ClientShellAction::Endpoint { request, .. }]
        if matches!(request.method, crate::api::schema::Method::AgentConversationSettleAndClose(_)))
    );
}

#[test]
fn inbox_actions_modal_restore_shortcut_uses_restore_method() {
    let mut state = state(Config::default());
    let record = &mut state.endpoints[0]
        .conversations
        .as_mut()
        .unwrap()
        .projection
        .conversations[0];
    record.attachments.clear();
    record.execution = crate::agent_inbox::ExecutionState::Closed;
    record.disposition = crate::agent_inbox::Disposition::Settled;
    record.section = crate::agent_inbox::ConversationSection::Settled;
    state.agent_inbox_selected = Some((ClientEndpointId::Local, "keys-conversation".into()));
    state.handle_input_bytes(b"\x02A");
    let outcome = state.handle_input_bytes(b"r");
    assert!(outcome.requests.is_empty());
    assert!(
        matches!(&outcome.actions[..], [ClientShellAction::Endpoint { request, .. }]
        if matches!(request.method, crate::api::schema::Method::AgentConversationRestore(_)))
    );
}

#[test]
fn inbox_prefix_g_keeps_navigator_search() {
    let mut state = state(Config::default());
    state.handle_input_bytes(b"\x02g");
    assert!(matches!(
        state.overlay,
        Some(ClientShellOverlay::Navigator(_))
    ));
}

#[test]
fn inbox_actions_follow_focused_pane_before_stale_row_selection() {
    let mut state = state(Config::default());
    let records = &mut state.endpoints[0]
        .conversations
        .as_mut()
        .unwrap()
        .projection
        .conversations;
    let mut other = records[0].clone();
    other.id = "previous-selection".into();
    other.attachments[0].public_pane_id = "pane_2".into();
    other.attachments[0].terminal_id = "other-terminal".into();
    records.push(other);
    state.agent_inbox_selected = Some((ClientEndpointId::Local, "previous-selection".into()));
    state.handle_input_bytes(b"\x02A");
    let Some(ClientShellOverlay::ContextMenu(menu)) = state.overlay.as_ref() else {
        panic!("prefix+shift+a must open focused conversation actions");
    };
    assert!(
        matches!(&menu.target, ClientContextMenuTarget::Conversation { record, .. }
        if record.id == "keys-conversation")
    );
}

#[test]
fn inbox_shift_a_accepts_legacy_and_kitty_uppercase_keys() {
    for input in [
        b"\x02A".as_slice(),
        b"\x02\x1b[97;2u".as_slice(),
        b"\x02\x1b[65;2u".as_slice(),
        b"\x02\x1b[97:65;2u".as_slice(),
    ] {
        let mut state = state(Config::default());
        state.handle_input_bytes(input);
        assert!(
            matches!(state.overlay, Some(ClientShellOverlay::ContextMenu(ref menu))
            if matches!(menu.target, ClientContextMenuTarget::Conversation { .. })),
            "input: {input:?}"
        );
    }
}

#[test]
fn inbox_configured_shift_a_binding_wins_for_legacy_and_kitty() {
    for input in [
        b"\x02A".as_slice(),
        b"\x02\x1b[97;2u".as_slice(),
        b"\x02\x1b[65;2u".as_slice(),
        b"\x02\x1b[97:65;2u".as_slice(),
    ] {
        let config: Config = toml::from_str("[keys]\nhelp = \"prefix+shift+a\"\n").unwrap();
        let mut state = state(config);
        state.handle_input_bytes(input);
        assert!(
            matches!(state.overlay, Some(ClientShellOverlay::Help(_))),
            "input: {input:?}"
        );
    }
}

#[test]
fn inbox_lowercase_a_keeps_latch_command_and_has_no_actions_fallback() {
    let mut plain = state(Config::default());
    let outcome = plain.handle_input_bytes(b"\x02a");
    assert!(outcome.actions.is_empty());
    assert!(plain.overlay.is_none());

    let mut state = state(Config::default());
    let mut projection = state.snapshot.as_deref().unwrap().clone();
    projection
        .commands
        .push(crate::protocol::ClientShellCommand {
            command_id: "local.latch.open".into(),
            binding_label: "prefix+a".into(),
            binding_labels: vec!["prefix+a".into()],
            action: crate::protocol::ClientShellCommandAction::PluginAction,
            description: Some("Latch".into()),
        });
    state.set_snapshot(Box::new(projection));
    let outcome = state.handle_input_bytes(b"\x02a");
    assert!(state.overlay.is_none());
    let [ClientShellAction::Endpoint { request, .. }] = &outcome.actions[..] else {
        panic!("prefix+a must still invoke Latch");
    };
    assert!(
        matches!(&request.method, crate::api::schema::Method::CommandInvoke(params)
        if params.command_id == "local.latch.open")
    );
}
