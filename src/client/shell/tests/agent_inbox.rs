use super::*;
use crate::agent_inbox::{ConversationRecord, ConversationSection, Disposition, ExecutionState};
use crate::client::endpoint::ClientAgentConversationsProjection;

fn record(id: &str, section: ConversationSection, attached: bool) -> ConversationRecord {
    let attachment = serde_json::json!({"terminal_id":"test-terminal","pane_id":999,"public_pane_id":"pane_1","workspace_id":"ws_1","tab_id":"tab_1","workspace_label":"Project","tab_label":"Task","workspace_location_id":"workspace-location","tab_location_id":"tab-location","cwd":"/repo"});
    serde_json::from_value(serde_json::json!({
        "id":id,"provider":"codex","source":"hook","session_ref":{"kind":"id","value":id},"title":id,
        "saved_location":attachment,"disposition":if section == ConversationSection::Settled { "settled" } else { "inbox" },
        "created_at_ms":1,"attention_returned_at_ms":20,"completed_at_ms":20,"last_human_submission_at_ms":10,"settled_at_ms":30,"revision":1,
        "attachments":if attached { vec![attachment] } else { vec![] },"section":section,
        "execution":if attached { "idle" } else { "closed" },"queued_human_work":false,"blocking_request":false,"failure":false,"capability_limits":[],"resume_state":"none"
    })).unwrap()
}

fn projection(
    boot: &str,
    revision: u64,
    records: Vec<ConversationRecord>,
) -> ClientAgentConversationsProjection {
    ClientAgentConversationsProjection {
        boot_id: boot.into(),
        revision,
        total_count: records.len(),
        conversations: records,
        next_offset: None,
        recovery_error: None,
    }
}

fn inbox(records: Vec<ConversationRecord>) -> ClientShellState {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    state.set_endpoint_agent_conversations_supported(&ClientEndpointId::Local, true);
    state.set_endpoint_methods_for(
        &ClientEndpointId::Local,
        Some(
            ["list", "settle", "reopen", "resume", "open"]
                .map(|action| format!("agent.conversation.{action}"))
                .to_vec(),
        ),
    );
    state.endpoints[0]
        .methods
        .as_mut()
        .unwrap()
        .insert("pane.focus".into());
    assert!(state.set_endpoint_agent_conversations(
        &ClientEndpointId::Local,
        None,
        projection("boot-1", 1, records)
    ));
    state.agent_inbox = true;
    state
}

fn press(state: &mut ClientShellState, code: KeyCode) -> ClientShellInput {
    let mut outcome = ClientShellInput::default();
    state.handle_key(
        crate::input::TerminalKey::new(code, KeyModifiers::empty()),
        &mut outcome,
    );
    outcome
}

#[test]
fn inbox_defaults_off_and_closed_settled_conversations_remain_searchable() {
    let state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    assert!(!state.agent_inbox);
    assert_eq!(state.agent_inbox_expanded, [true, false, false]);
    let mut state = inbox(vec![record(
        "closed-history",
        ConversationSection::Settled,
        false,
    )]);
    assert!(state.visible_conversations().is_empty());
    state.open_navigator_overlay();
    if let Some(ClientShellOverlay::Navigator(navigator)) = state.overlay.as_mut() {
        navigator.query = TextEditor::new("closed-history", false);
    }
    let ClientShellOverlay::Navigator(navigator) = state.overlay.as_ref().unwrap() else {
        panic!()
    };
    let rows =
        render::client_navigator_rows(&state.endpoints, &state.active_endpoint_id, navigator);
    assert!(rows.iter().any(|row| matches!(&row.target, ClientNavigatorTarget::Conversation { conversation_id, .. } if conversation_id == "closed-history")));
    assert_eq!(
        state.endpoints[0]
            .conversations
            .as_ref()
            .unwrap()
            .projection
            .conversations[0]
            .disposition,
        Disposition::Settled
    );
}

#[test]
fn inbox_open_acknowledges_read_without_settling_and_uses_exact_public_pane() {
    let mut state = inbox(vec![record("done", ConversationSection::Inbox, true)]);
    let mut outcome = ClientShellInput::default();
    assert!(state
        .handle_conversation_navigation(crate::input::KeybindAction::FocusAgent(0), &mut outcome));
    assert!(outcome.actions.iter().any(|action| matches!(action, ClientShellAction::Endpoint { request, .. } if matches!(&request.method, crate::api::schema::Method::AgentConversationOpen(target) if target.attachment_terminal_id.as_deref() == Some("test-terminal")))));
    assert!(!state.conversation_read.is_empty());
    assert_eq!(state.visible_conversations().len(), 1);
    assert_eq!(
        state.endpoints[0]
            .conversations
            .as_ref()
            .unwrap()
            .projection
            .conversations[0]
            .disposition,
        Disposition::Inbox
    );
}

#[test]
fn inbox_projection_drives_mouse_numbers_next_and_skips_collapsed_headers() {
    let mut state = inbox(vec![
        record("newest", ConversationSection::Inbox, false),
        record("working", ConversationSection::Working, true),
        record("settled", ConversationSection::Settled, false),
    ]);
    let records = &mut state.endpoints[0]
        .conversations
        .as_mut()
        .unwrap()
        .projection
        .conversations;
    records[1].execution = ExecutionState::Working;
    let mut remote = super::super::endpoints::local_endpoint();
    remote.endpoint_id = ClientEndpointId::Ssh(
        crate::client::endpoint::ProfileId::parse("0123456789abcdef0123456789abcdef").unwrap(),
    );
    remote.label = "Build".into();
    remote.snapshot = Some(Box::new(snapshot()));
    remote.conversations_supported = true;
    let mut remote_record = record("older", ConversationSection::Inbox, false);
    remote_record.attention_returned_at_ms = 5;
    remote.conversations = Some(super::super::agent_inbox::CachedConversations {
        generation: None,
        projection: projection("boot-1", 1, vec![remote_record]),
    });
    state.endpoints.push(remote);
    let rows = state.visible_conversations();
    assert_eq!(
        rows.iter().map(|row| row.id.as_str()).collect::<Vec<_>>(),
        ["newest", "older"]
    );
    let mut buffer = Buffer::empty(Rect::new(0, 0, 45, 18));
    state.hits.agent_body = Rect::new(0, 3, 45, 15);
    super::super::agent_inbox::render_conversation_panel(
        &mut buffer,
        &state.endpoints,
        true,
        state.agent_inbox_expanded,
        None,
        &state.conversation_read,
        false,
        &state.config,
        &mut state.agent_scroll,
        &mut state.hits,
    );
    assert_eq!(state.hits.agent_inbox_sections.len(), 3);
    let hit = state.hits.conversations[0].clone();
    let mut outcome = ClientShellInput::default();
    assert!(state.handle_conversation_click((hit.0.x, hit.0.y), false, &mut outcome));
    assert_eq!(
        state
            .agent_inbox_selected
            .as_ref()
            .map(|row| row.1.as_str()),
        Some("newest")
    );
    state.overlay = None;
    state.handle_conversation_navigation(crate::input::KeybindAction::NextAgent, &mut outcome);
    assert_eq!(
        state
            .agent_inbox_selected
            .as_ref()
            .map(|row| row.1.as_str()),
        Some("older")
    );
    state.overlay = None;
    state.handle_conversation_navigation(crate::input::KeybindAction::FocusAgent(0), &mut outcome);
    assert_eq!(
        state
            .agent_inbox_selected
            .as_ref()
            .map(|row| row.1.as_str()),
        Some("newest")
    );
    assert!(state
        .hits
        .agent_inbox_sections
        .iter()
        .all(|(header, _)| !state
            .hits
            .conversations
            .iter()
            .any(|(row, _, _)| header == row)));
}

#[test]
fn inbox_movement_preserves_terminal_focus_and_selects_next_visible_conversation() {
    let mut first = record("first", ConversationSection::Inbox, false);
    first.attention_returned_at_ms = 30;
    let mut second = record("second", ConversationSection::Inbox, false);
    second.attention_returned_at_ms = 20;
    let mut third = record("third", ConversationSection::Inbox, false);
    third.attention_returned_at_ms = 10;
    let mut state = inbox(vec![first.clone(), second.clone(), third.clone()]);
    state.agent_inbox_selected = Some((ClientEndpointId::Local, "second".into()));
    second.section = ConversationSection::Settled;
    second.disposition = Disposition::Settled;
    assert!(state.set_endpoint_agent_conversations(
        &ClientEndpointId::Local,
        None,
        projection("boot-1", 2, vec![first, second, third])
    ));
    assert_eq!(
        state
            .agent_inbox_selected
            .as_ref()
            .map(|row| row.1.as_str()),
        Some("third")
    );
    assert_eq!(
        state.snapshot.as_ref().unwrap().focused_pane_id.as_deref(),
        Some("pane_1")
    );
    assert!(state.take_conversation_actions().is_empty());
}

#[test]
fn inbox_keyboard_menu_switches_mode_and_expands_working() {
    let mut state = inbox(vec![]);
    let actions = super::super::global_menu::global_menu_items(state.snapshot.as_ref().unwrap());
    let index = actions
        .iter()
        .position(|(_, action)| {
            *action == super::super::global_menu::ClientGlobalMenuAction::ToggleAgentInbox
        })
        .unwrap();
    state.toggle_global_menu();
    if let Some(ClientShellOverlay::GlobalMenu(menu)) = state.overlay.as_mut() {
        menu.highlighted = index;
    }
    press(&mut state, KeyCode::Enter);
    assert!(!state.agent_inbox);
    let index = actions
        .iter()
        .position(|(_, action)| {
            *action == super::super::global_menu::ClientGlobalMenuAction::ToggleInboxSection(1)
        })
        .unwrap();
    state.toggle_global_menu();
    if let Some(ClientShellOverlay::GlobalMenu(menu)) = state.overlay.as_mut() {
        menu.highlighted = index;
    }
    press(&mut state, KeyCode::Enter);
    assert_eq!(state.agent_inbox_expanded, [true, true, false]);
}

#[test]
fn inbox_multiple_attachments_require_a_picker_and_closed_click_never_resumes() {
    let mut multiple = record("multiple", ConversationSection::Inbox, true);
    let mut second = multiple.attachments[0].clone();
    second.public_pane_id = "pane_2".into();
    multiple.attachments.push(second);
    let mut state = inbox(vec![
        multiple.clone(),
        record("closed", ConversationSection::Inbox, false),
    ]);
    let mut outcome = ClientShellInput::default();
    state.open_conversation(
        ClientEndpointId::Local,
        "multiple".into(),
        false,
        &mut outcome,
    );
    assert!(outcome.actions.is_empty());
    let Some(ClientShellOverlay::ContextMenu(menu)) = state.overlay.as_ref() else {
        panic!()
    };
    assert_eq!(
        menu.items()
            .iter()
            .filter(|item| matches!(item.action, ClientContextMenuAction::ConversationOpen(_)))
            .count(),
        2
    );
    state.open_conversation(
        ClientEndpointId::Local,
        "closed".into(),
        false,
        &mut outcome,
    );
    assert!(outcome.actions.is_empty());
    let Some(ClientShellOverlay::ContextMenu(menu)) = state.overlay.as_ref() else {
        panic!()
    };
    assert!(menu
        .items()
        .iter()
        .any(|item| item.action == ClientContextMenuAction::ConversationResume));
}

#[test]
fn inbox_stale_generations_and_boots_cannot_replace_cache_or_enable_actions() {
    let mut state = inbox(vec![record(
        "retained",
        ConversationSection::Settled,
        false,
    )]);
    state.endpoints[0].snapshot_generation = Some(2);
    state.endpoints[0]
        .conversations
        .as_mut()
        .unwrap()
        .generation = Some(2);
    assert!(!state.set_endpoint_agent_conversations(
        &ClientEndpointId::Local,
        Some(1),
        projection("boot-1", 9, vec![])
    ));
    assert!(!state.set_endpoint_agent_conversations(
        &ClientEndpointId::Local,
        Some(2),
        projection("old-boot", 9, vec![])
    ));
    state.set_endpoint_status(&ClientEndpointId::Local, ClientEndpointStatus::Reconnecting);
    assert_eq!(state.visible_conversations()[0].id, "retained");
    let mut outcome = ClientShellInput::default();
    state.open_conversation(
        ClientEndpointId::Local,
        "retained".into(),
        true,
        &mut outcome,
    );
    let Some(ClientShellOverlay::ContextMenu(menu)) = state.overlay.as_ref() else {
        panic!()
    };
    assert!(menu.items().is_empty());
    assert!(outcome.actions.is_empty());
}

#[test]
fn inbox_pages_merge_only_matching_projection_and_restart_changed_revision_at_zero() {
    let mut state = inbox(vec![]);
    let mut first = projection(
        "boot-1",
        2,
        vec![record("one", ConversationSection::Inbox, false)],
    );
    first.total_count = 2;
    first.next_offset = Some(1);
    state.set_endpoint_agent_conversations(&ClientEndpointId::Local, None, first);
    let action = state.take_conversation_actions().pop().unwrap();
    let ClientShellAction::Endpoint { request, .. } = action else {
        panic!()
    };
    let page = projection(
        "boot-1",
        2,
        vec![record("two", ConversationSection::Settled, false)],
    );
    let (changed, actions) = state.handle_endpoint_result(
        "boot-1",
        &request.id,
        Ok(crate::api::schema::ResponseResult::AgentConversations {
            projection: serde_json::to_value(page).unwrap(),
        }),
    );
    assert!(changed);
    assert!(actions.is_empty());
    assert_eq!(
        state.endpoints[0]
            .conversations
            .as_ref()
            .unwrap()
            .projection
            .conversations
            .len(),
        2
    );
    let mut first = projection(
        "boot-1",
        3,
        vec![record("three", ConversationSection::Inbox, false)],
    );
    first.total_count = 3;
    first.next_offset = Some(1);
    state.set_endpoint_agent_conversations(&ClientEndpointId::Local, None, first);
    let ClientShellAction::Endpoint { request, .. } =
        state.take_conversation_actions().pop().unwrap()
    else {
        panic!()
    };
    let mut changed_page = projection(
        "boot-1",
        4,
        vec![record("wrong-offset", ConversationSection::Inbox, false)],
    );
    changed_page.next_offset = Some(2);
    changed_page.total_count = 3;
    let (_, actions) = state.handle_endpoint_result(
        "boot-1",
        &request.id,
        Ok(crate::api::schema::ResponseResult::AgentConversations {
            projection: serde_json::to_value(changed_page).unwrap(),
        }),
    );
    assert!(actions.iter().any(|action| matches!(action,ClientShellAction::Endpoint{request,..} if matches!(&request.method,crate::api::schema::Method::AgentConversationList(params) if params.offset==0))));
    assert!(state.endpoints[0]
        .conversations
        .as_ref()
        .unwrap()
        .projection
        .conversations
        .is_empty());
}

#[test]
fn inbox_rejects_invalid_page_atomically_and_keeps_recovery_visible() {
    let mut state = inbox(vec![]);
    let mut first = projection(
        "boot-1",
        2,
        vec![record("retained", ConversationSection::Settled, false)],
    );
    first.next_offset = Some(1);
    first.total_count = 3;
    state.set_endpoint_agent_conversations(&ClientEndpointId::Local, None, first);
    let ClientShellAction::Endpoint { request, .. } =
        state.take_conversation_actions().pop().unwrap()
    else {
        panic!()
    };
    let mut invalid = record("invalid", ConversationSection::Inbox, false);
    invalid.session_ref.as_mut().unwrap().value = String::new();
    let mut page = projection(
        "boot-1",
        2,
        vec![record("valid", ConversationSection::Inbox, false), invalid],
    );
    page.total_count = 3;
    state.handle_endpoint_result(
        "boot-1",
        &request.id,
        Ok(crate::api::schema::ResponseResult::AgentConversations {
            projection: serde_json::to_value(page).unwrap(),
        }),
    );
    let cache = state.endpoints[0].conversations.as_ref().unwrap();
    assert_eq!(cache.projection.conversations.len(), 1);
    assert!(cache.projection.recovery_error.is_some());
    assert_eq!(state.visible_conversations()[0].id, "retained");
}

#[test]
fn inbox_settle_reopen_and_resume_are_explicit_distinct_endpoint_actions() {
    let done = record("closed", ConversationSection::Inbox, false);
    let mut state = inbox(vec![done.clone()]);
    let mut outcome = ClientShellInput::default();
    state.activate_conversation_action(
        ClientEndpointId::Local,
        done.clone(),
        ClientContextMenuAction::ConversationSettle,
        &mut outcome,
    );
    assert_eq!(outcome.actions.len(), 1);
    assert!(
        matches!(&outcome.actions[0],ClientShellAction::Endpoint{request,..} if matches!(request.method,crate::api::schema::Method::AgentConversationSettle(_)))
    );
    assert!(outcome.requests.is_empty());
    assert_eq!(
        state.endpoints[0]
            .conversations
            .as_ref()
            .unwrap()
            .projection
            .conversations[0]
            .disposition,
        Disposition::Inbox
    );
    let mut settled = done.clone();
    settled.disposition = Disposition::Settled;
    outcome.actions.clear();
    state.activate_conversation_action(
        ClientEndpointId::Local,
        settled,
        ClientContextMenuAction::ConversationReopen,
        &mut outcome,
    );
    assert!(
        matches!(&outcome.actions[0],ClientShellAction::Endpoint{request,..} if matches!(request.method,crate::api::schema::Method::AgentConversationReopen(_)))
    );
    outcome.actions.clear();
    state.activate_conversation_action(
        ClientEndpointId::Local,
        done,
        ClientContextMenuAction::ConversationResume,
        &mut outcome,
    );
    assert!(
        matches!(&outcome.actions[0],ClientShellAction::Endpoint{request,..} if matches!(&request.method,crate::api::schema::Method::AgentConversationResume(params) if params.location.is_none()))
    );
}

#[test]
fn inbox_render_scaling() {
    let measure = |count: usize| {
        let mut records = Vec::new();
        let mut metadata = snapshot();
        let mut view = surface();
        metadata.panes.clear();
        view.panes.clear();
        for index in 0..count {
            let pane_id = format!("pane_{}", index + 1);
            let mut conversation = record(
                &format!("conversation-{index}"),
                ConversationSection::Inbox,
                true,
            );
            conversation.attachments[0].public_pane_id = pane_id.clone();
            records.push(conversation);
            let mut pane = snapshot().panes.remove(0);
            pane.pane_id = pane_id.clone();
            pane.focused = index == 0;
            metadata.panes.push(pane);
            let mut pane = surface().panes.remove(0);
            pane.pane_id = pane_id;
            pane.focused = index == 0;
            view.panes.push(pane);
        }
        let mut state = inbox(records);
        state.set_snapshot(Box::new(metadata));
        state.set_pane_surface(view);
        for _ in 0..20 {
            std::hint::black_box(state.compose(120, 40).unwrap());
        }
        let start = std::time::Instant::now();
        for _ in 0..300 {
            std::hint::black_box(state.compose(120, 40).unwrap());
        }
        start.elapsed().as_nanos() / 300
    };
    let one = measure(1);
    let fifteen = measure(15);
    eprintln!("agent inbox enabled, fixed 120x40, 300 frames: 1 pane {one} ns/frame; 15 panes {fifteen} ns/frame; ratio {:.2}",fifteen as f64/one.max(1) as f64);
}

#[test]
fn inbox_keyboard_opens_single_live_conversation_actions_and_settles_explicitly() {
    let mut state = inbox(vec![record("live", ConversationSection::Inbox, true)]);
    let mut outcome = ClientShellInput::default();
    state.handle_conversation_navigation(crate::input::KeybindAction::FocusAgent(0), &mut outcome);
    state.toggle_global_menu();
    let items = super::super::global_menu::global_menu_items(state.snapshot.as_ref().unwrap());
    let index = items
        .iter()
        .position(|(_, action)| {
            *action == super::super::global_menu::ClientGlobalMenuAction::ConversationActions
        })
        .unwrap();
    if let Some(ClientShellOverlay::GlobalMenu(menu)) = state.overlay.as_mut() {
        menu.highlighted = index;
    }
    let outcome = press(&mut state, KeyCode::Enter);
    assert!(outcome.actions.is_empty());
    let Some(ClientShellOverlay::ContextMenu(menu)) = state.overlay.as_mut() else {
        panic!()
    };
    menu.highlighted = menu
        .items()
        .iter()
        .position(|item| item.action == ClientContextMenuAction::ConversationSettle)
        .unwrap();
    let outcome = press(&mut state, KeyCode::Enter);
    assert_eq!(outcome.actions.len(), 1);
    assert!(
        matches!(&outcome.actions[0],ClientShellAction::Endpoint{request,..} if matches!(request.method,crate::api::schema::Method::AgentConversationSettle(_)))
    );
}

#[test]
fn inbox_downgrade_retains_recovery_and_unmatched_live_agents_without_duplicate_native_rows() {
    let retained = record("retained", ConversationSection::Inbox, true);
    let mut state = inbox(vec![retained]);
    let mut metadata = snapshot();
    metadata.agents.push(ClientShellAgent {
        pane_id: "pane_1".into(),
        workspace_id: "ws_1".into(),
        tab_id: "tab_1".into(),
        name: Some("retained-live".into()),
        display_agent: None,
        agent: Some("codex".into()),
        title: None,
        terminal_title: None,
        terminal_title_stripped: None,
        agent_status: AgentStatus::Idle,
        state_change_seq: 1,
        state_labels: vec![],
        tokens: vec![],
        focused: false,
    });
    let mut added = metadata.agents[0].clone();
    added.pane_id = "new-live-pane".into();
    added.name = Some("new-live".into());
    metadata.agents.push(added);
    state.set_snapshot(Box::new(metadata));
    state.set_endpoint_agent_conversations_supported(&ClientEndpointId::Local, false);
    let rows = state.visible_conversations();
    assert_eq!(rows.len(), 2);
    assert!(rows
        .iter()
        .any(|row| row.id == "retained" && row.unavailable));
    assert!(rows.iter().any(|row| row.id == "pane:new-live-pane"));
}

#[test]
fn inbox_closed_remote_details_render_without_active_terminal_surface() {
    let mut state = inbox(vec![record(
        "closed-remote",
        ConversationSection::Inbox,
        false,
    )]);
    let mut remote = state.endpoints[0].clone();
    let remote_id = ClientEndpointId::Ssh(
        crate::client::endpoint::ProfileId::parse("0123456789abcdef0123456789abcdef").unwrap(),
    );
    remote.endpoint_id = remote_id.clone();
    remote.label = "Build".into();
    state.endpoints[0].snapshot = None;
    state.endpoints[0].conversations = None;
    state.endpoints[0].status = ClientEndpointStatus::Reconnecting;
    state.endpoints.push(remote);
    state.snapshot = None;
    state.pane_surface = None;
    state.compose(100, 30).unwrap();
    let mut outcome = ClientShellInput::default();
    state.open_conversation(remote_id, "closed-remote".into(), true, &mut outcome);
    let frame = state.compose(100, 30).unwrap();
    assert!(!state.hits.context_menu_rows.is_empty());
    assert!(frame
        .cells
        .iter()
        .map(|cell| cell.symbol.as_str())
        .collect::<String>()
        .contains("closed-remote"));
}

#[test]
fn inbox_numbered_navigation_reveals_rows_below_short_viewport() {
    let mut state = inbox(
        (0..12)
            .map(|index| {
                record(
                    &format!("row-{index:02}"),
                    ConversationSection::Inbox,
                    false,
                )
            })
            .collect(),
    );
    state.hits.agent_body = Rect::new(0, 3, 35, 6);
    let mut outcome = ClientShellInput::default();
    state.handle_conversation_navigation(crate::input::KeybindAction::FocusAgent(11), &mut outcome);
    assert!(state.agent_scroll > 0);
    let selected = state.agent_inbox_selected.clone();
    let mut buffer = Buffer::empty(Rect::new(0, 0, 35, 9));
    super::super::agent_inbox::render_conversation_panel(
        &mut buffer,
        &state.endpoints,
        true,
        state.agent_inbox_expanded,
        selected.as_ref(),
        &state.conversation_read,
        false,
        &state.config,
        &mut state.agent_scroll,
        &mut state.hits,
    );
    assert!(state
        .hits
        .conversations
        .iter()
        .any(|(_, endpoint, id)| Some(&(endpoint.clone(), id.clone())) == selected.as_ref()));
}

#[test]
fn inbox_short_context_menu_keeps_keyboard_recovery_action_visible() {
    let mut closed = record("closed", ConversationSection::Inbox, false);
    closed.capability_limits = (0..25)
        .map(|index| format!("Capability limit {index}"))
        .collect();
    let mut state = inbox(vec![closed]);
    let mut outcome = ClientShellInput::default();
    state.open_conversation(ClientEndpointId::Local, "closed".into(), true, &mut outcome);
    let Some(ClientShellOverlay::ContextMenu(menu)) = state.overlay.as_mut() else {
        panic!()
    };
    let last = menu.items().len() - 1;
    menu.highlighted = last;
    let mut buffer = Buffer::empty(Rect::new(0, 0, 40, 8));
    let rendered = render::render_context_menu(&mut buffer, menu, &state.config.palette).unwrap();
    assert!(rendered.menu_rows.iter().any(|(_, index)| *index == last));
    assert!(rendered
        .menu_rows
        .iter()
        .all(|(rect, _)| rect.bottom() <= 8));
}

#[test]
fn inbox_pending_resume_keeps_native_row_and_suppresses_only_exact_provisional_generation() {
    let mut original = record("original-native", ConversationSection::Settled, false);
    original.resume_state = crate::agent_inbox::ResumeState::Uncertain;
    original.resume_target = Some(original.saved_location.clone());
    let mut provisional = record("provisional-target", ConversationSection::Inbox, true);
    provisional.session_ref = None;
    let mut other_provider = provisional.clone();
    other_provider.id = "conflicting-provisional".into();
    other_provider.provider = "claude".into();
    other_provider.source.clear();
    let conflict = record("conflicting-native", ConversationSection::Inbox, true);
    let mut state = inbox(vec![
        original.clone(),
        provisional.clone(),
        conflict,
        other_provider,
    ]);
    let rows = state.visible_conversations();
    assert!(rows
        .iter()
        .any(|row| row.id == "original-native"
            && row.detail.contains("Awaiting session confirmation")));
    assert!(rows.iter().any(|row| row.id == "conflicting-native"));
    assert!(rows.iter().any(|row| row.id == "conflicting-provisional"));
    assert!(!rows.iter().any(|row| row.id == "provisional-target"));
    state.open_navigator_overlay();
    let Some(ClientShellOverlay::Navigator(navigator)) = state.overlay.as_ref() else {
        panic!()
    };
    let rows =
        render::client_navigator_rows(&state.endpoints, &state.active_endpoint_id, navigator);
    assert!(!rows.iter().any(|row| matches!(&row.target, ClientNavigatorTarget::Conversation { conversation_id, .. } if conversation_id == "provisional-target")));
    let mut outcome = ClientShellInput::default();
    state.open_conversation(
        ClientEndpointId::Local,
        original.id.clone(),
        true,
        &mut outcome,
    );
    let Some(ClientShellOverlay::ContextMenu(menu)) = state.overlay.as_ref() else {
        panic!()
    };
    assert!(menu
        .items()
        .iter()
        .any(|item| item.action == ClientContextMenuAction::ConversationOpenResumeTarget));
    assert!(!menu
        .items()
        .iter()
        .any(|item| item.action == ClientContextMenuAction::ConversationResume));
    assert!(
        super::super::agent_inbox::conversation_details(&menu.target)
            .iter()
            .any(|line| line.contains("Awaiting session confirmation"))
    );

    provisional.attachments[0].terminal_id = "replacement-terminal".into();
    state.set_endpoint_agent_conversations(
        &ClientEndpointId::Local,
        None,
        projection("boot-1", 2, vec![original, provisional]),
    );
    assert!(state
        .visible_conversations()
        .iter()
        .any(|row| row.id == "provisional-target"));
}

#[test]
fn inbox_pending_open_uses_server_generation_guard_and_activates_only_after_success() {
    let mut original = record("original", ConversationSection::Inbox, false);
    original.resume_state = crate::agent_inbox::ResumeState::Pending;
    original.resume_target = Some(original.saved_location.clone());
    let mut state = inbox(vec![original.clone()]);
    let remote_id = ClientEndpointId::Ssh(
        crate::client::endpoint::ProfileId::parse("0123456789abcdef0123456789abcdef").unwrap(),
    );
    let mut remote = state.endpoints[0].clone();
    remote.endpoint_id = remote_id.clone();
    state.endpoints.push(remote);
    let mut outcome = ClientShellInput::default();
    state.open_conversation(remote_id.clone(), original.id.clone(), false, &mut outcome);
    assert_eq!(outcome.actions.len(), 1);
    let ClientShellAction::Endpoint {
        endpoint_id,
        request,
        ..
    } = &outcome.actions[0]
    else {
        panic!()
    };
    assert_eq!(endpoint_id, &remote_id);
    assert!(
        matches!(&request.method, crate::api::schema::Method::AgentConversationOpen(params) if params.attachment_terminal_id.as_deref() == Some("test-terminal"))
    );
    let request_id = request.id.clone();
    let (_, actions) = state.handle_endpoint_result(
        "boot-1",
        &request_id,
        Err(ClientShellEndpointError {
            code: Some("stale_target".into()),
            message: "The terminal generation was replaced".into(),
        }),
    );
    assert!(actions.is_empty());
    assert_eq!(state.active_endpoint_id, ClientEndpointId::Local);
    outcome.actions.clear();
    state.open_conversation(remote_id.clone(), original.id.clone(), false, &mut outcome);
    let ClientShellAction::Endpoint { request, .. } = &outcome.actions[0] else {
        panic!()
    };
    let (_, actions) = state.handle_endpoint_result(
        "boot-1",
        &request.id,
        Ok(
            crate::api::schema::ResponseResult::AgentConversationOpened {
                conversation_id: original.id,
                pane_id: "pane_1".into(),
            },
        ),
    );
    assert!(
        matches!(&actions[0], ClientShellAction::ActivateEndpoint { endpoint_id, target: None } if endpoint_id == &remote_id)
    );
    let record = &state.endpoints[1]
        .conversations
        .as_ref()
        .unwrap()
        .projection
        .conversations[0];
    assert_eq!(
        record.resume_state,
        crate::agent_inbox::ResumeState::Pending
    );
    assert!(record.attachments.is_empty());
}

#[test]
fn inbox_global_menu_keeps_highlighted_action_visible_in_short_terminals() {
    let state = inbox(Vec::new());
    let last =
        super::super::global_menu::global_menu_items(state.snapshot.as_deref().unwrap()).len() - 1;
    let menu = ClientGlobalMenuOverlay { highlighted: last };
    let mut buffer = Buffer::empty(Rect::new(0, 0, 40, 8));
    let rendered = render::render_global_menu(
        &mut buffer,
        Rect::new(0, 7, 8, 1),
        &menu,
        state.snapshot.as_deref().unwrap(),
        &state.config.palette,
    )
    .unwrap();
    assert!(rendered.menu_rows.iter().any(|(_, index)| *index == last));
    assert!(rendered
        .menu_rows
        .iter()
        .all(|(rect, _)| rect.bottom() <= 8));
}

#[test]
fn inbox_same_revision_recovery_errors_are_visible_and_can_clear() {
    let saved = record("saved", ConversationSection::Settled, false);
    let mut state = inbox(vec![saved.clone()]);
    let mut error = projection("boot-1", 1, vec![saved.clone()]);
    error.recovery_error = Some("History could not be saved".into());
    assert!(state.set_endpoint_agent_conversations(&ClientEndpointId::Local, None, error));
    assert_eq!(state.visible_conversations().len(), 1);
    assert!(state.set_endpoint_agent_conversations(
        &ClientEndpointId::Local,
        None,
        projection("boot-1", 1, vec![saved]),
    ));
    assert!(state.visible_conversations().is_empty());
}

#[test]
fn inbox_definitive_resume_failure_reports_without_reopening_or_replaying_on_restore() {
    let mut pending = record("saved", ConversationSection::Settled, false);
    pending.resume_state = crate::agent_inbox::ResumeState::Pending;
    let mut state = inbox(vec![pending.clone()]);
    let mut failed = pending;
    failed.resume_state = crate::agent_inbox::ResumeState::None;
    failed.resume_error = Some("The agent exited before confirming its session".into());
    state.set_endpoint_agent_conversations(
        &ClientEndpointId::Local,
        None,
        projection("boot-1", 2, vec![failed.clone()]),
    );
    assert_eq!(
        state.visible_endpoint_notice.as_ref().unwrap().title,
        "Resume failed"
    );
    assert!(state.visible_conversations().is_empty());
    let restored = inbox(vec![failed]);
    assert!(restored.visible_endpoint_notice.is_none());
}
