use super::*;

#[tokio::test]
async fn conversation_companion_streams_once_to_active_and_inactive_clients() {
    let mut server = test_headless_server();
    server
        .app
        .state
        .workspaces
        .push(crate::workspace::Workspace::test_new("conversation"));
    server.app.state.ensure_test_terminals();
    server.app.state.active = Some(0);
    let pane_id = server.app.state.workspaces[0].tabs[0].root_pane;
    let terminal_id = server.app.state.workspaces[0].tabs[0].panes[&pane_id]
        .attached_terminal_id
        .clone();
    let terminal = server.app.state.terminals.get_mut(&terminal_id).unwrap();
    terminal.set_detected_state(
        Some(crate::detect::Agent::Codex),
        crate::detect::AgentState::Working,
    );
    server.app.sync_agent_conversations();
    let (active, _) = connect_matching_test_shell(&mut server, 91);
    let (inactive, _) = connect_matching_test_shell(&mut server, 92);
    let active_snapshot = client_shell_snapshot(&active);
    let _ = client_shell_snapshot(&inactive);
    server.clients.get_mut(&92).unwrap().shell_surface_active = false;
    server.stream_agent_conversations();
    for receiver in [&active, &inactive] {
        let bytes = receiver.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(bytes.len() <= MAX_FRAME_SIZE);
        let ServerMessage::EndpointControl { kind, data } = read_server_message(bytes) else {
            panic!("expected lifecycle companion")
        };
        assert_eq!(kind, protocol::endpoint::AGENT_CONVERSATIONS_KIND);
        let projection: protocol::endpoint::EndpointAgentConversations =
            serde_json::from_str(&data).unwrap();
        assert_eq!(projection.boot_id, active_snapshot.boot_id);
        assert_eq!(projection.conversations.len(), 1);
        assert_eq!(
            projection.conversations[0].attachments[0].public_pane_id,
            server.app.public_pane_id(0, pane_id).unwrap()
        );
    }
    server.stream_agent_conversations();
    assert!(active.try_recv().is_err());
    assert!(inactive.try_recv().is_err());
    server.app.agent_inbox_error = Some("persistence failed".into());
    server.stream_agent_conversations();
    for receiver in [&active, &inactive] {
        let ServerMessage::EndpointControl { data, .. } =
            read_server_message(receiver.recv_timeout(Duration::from_secs(1)).unwrap())
        else {
            panic!("expected companion")
        };
        assert!(data.contains("persistence failed"));
    }
    server.foreground_client_id = Some(91);
    let focus_before = server.default_shell_target();
    server.handle_client_shell_endpoint_request(
        92,
        server.client_shell_boot_id.clone(),
        Box::new(api::schema::Request {
            id: "inactive-history".into(),
            method: api::schema::Method::AgentConversationList(
                api::schema::AgentConversationListParams {
                    offset: 0,
                    limit: 100,
                },
            ),
        }),
    );
    let response = tokio::time::timeout(Duration::from_secs(1), server.server_event_rx.recv())
        .await
        .unwrap()
        .unwrap();
    server.handle_server_event(response);
    let response = read_server_message(inactive.recv_timeout(Duration::from_secs(1)).unwrap());
    assert!(
        matches!(
            response,
            ServerMessage::ClientShellEndpointResponseChunk { .. }
        ),
        "{response:?}"
    );
    assert_eq!(server.foreground_client_id, Some(91));
    assert_eq!(server.default_shell_target(), focus_before);
    assert!(!server.clients[&92].shell_surface_active);
}

#[tokio::test]
async fn restore_updates_only_requesting_client_location_before_success_response() {
    let mut server = test_headless_server();
    server
        .app
        .state
        .workspaces
        .push(crate::workspace::Workspace::test_new("saved"));
    server
        .app
        .state
        .workspaces
        .push(crate::workspace::Workspace::test_new("foreground"));
    server.app.state.ensure_test_terminals();
    server.app.state.active = Some(1);
    let pane = server.app.state.workspaces[0].tabs[0].root_pane;
    let terminal_id = server.app.state.workspaces[0].tabs[0].panes[&pane]
        .attached_terminal_id
        .clone();
    let terminal = server.app.state.terminals.get_mut(&terminal_id).unwrap();
    terminal.set_detected_state(
        Some(crate::detect::Agent::Codex),
        crate::detect::AgentState::Idle,
    );
    terminal.set_persisted_agent_session(crate::agent_resume::PersistedAgentSession {
        source: "herdr:codex".into(),
        agent: "codex".into(),
        session_ref: crate::agent_resume::AgentSessionRef::id("restore-client-test").unwrap(),
    });
    server.app.sync_agent_conversations();
    let record = server.app.agent_inbox.records().remove(0);
    let record = server
        .app
        .agent_inbox
        .mutate(
            &record.id,
            record.revision,
            crate::agent_inbox::ConversationAction::Settle,
            1,
        )
        .unwrap();
    let (active, _) = connect_matching_test_shell(&mut server, 91);
    let (inactive, _) = connect_matching_test_shell(&mut server, 92);
    let _ = client_shell_snapshot(&active);
    let _ = client_shell_snapshot(&inactive);
    server.clients.get_mut(&92).unwrap().shell_surface_active = false;
    server.foreground_client_id = Some(91);
    let previous = server.shell_target_for_client(91);
    let (respond_to, response_rx) = std::sync::mpsc::channel();
    server.handle_client_conversation_request(
        92,
        api::ApiRequestMessage {
            request: api::schema::Request {
                id: "restore".into(),
                method: api::schema::Method::AgentConversationRestore(
                    api::schema::AgentConversationRestoreParams {
                        conversation_id: record.id,
                        expected_revision: record.revision,
                        location: None,
                        cwd: None,
                    },
                ),
            },
            respond_to,
            response_write_complete: None,
        },
    );
    let response: serde_json::Value =
        serde_json::from_str(&response_rx.try_recv().unwrap()).unwrap();
    assert_eq!(response["result"]["result"]["status"], "uncertain");
    assert_eq!(
        response["result"]["result"]["pane_id"],
        server.app.public_pane_id(0, pane).unwrap()
    );
    assert_eq!(
        server.shell_target_for_client(92).unwrap().workspace_index,
        0
    );
    assert_eq!(server.shell_target_for_client(91), previous);
    assert_eq!(server.foreground_client_id, Some(91));
    assert!(!server.clients[&92].shell_surface_active);
}

#[tokio::test]
async fn inactive_client_can_snooze_and_wake_without_changing_runtime_or_focus() {
    let mut server = test_headless_server();
    server
        .app
        .state
        .workspaces
        .push(crate::workspace::Workspace::test_new("snooze"));
    server.app.state.ensure_test_terminals();
    server.app.state.active = Some(0);
    let pane_id = server.app.state.workspaces[0].tabs[0].root_pane;
    let terminal_id = server.app.state.workspaces[0].tabs[0].panes[&pane_id]
        .attached_terminal_id
        .clone();
    let terminal = server.app.state.terminals.get_mut(&terminal_id).unwrap();
    terminal.set_detected_state(
        Some(crate::detect::Agent::Codex),
        crate::detect::AgentState::Working,
    );
    terminal.set_persisted_agent_session(crate::agent_resume::PersistedAgentSession {
        source: "herdr:codex".into(),
        agent: "codex".into(),
        session_ref: crate::agent_resume::AgentSessionRef::id("snooze-client-test").unwrap(),
    });
    server.app.sync_agent_conversations();
    let original = server.app.agent_inbox.records().remove(0);
    let (receiver, _) = connect_matching_test_shell(&mut server, 92);
    let _ = client_shell_snapshot(&receiver);
    server.clients.get_mut(&92).unwrap().shell_surface_active = false;
    let focus_before = server.default_shell_target();
    let foreground_before = server.foreground_client_id;
    for snooze in [true, false] {
        let record = server.app.agent_inbox.records().remove(0);
        let method = if snooze {
            api::schema::Method::AgentConversationSnooze(
                api::schema::AgentConversationSnoozeParams {
                    conversation_id: record.id,
                    expected_revision: record.revision,
                    duration_ms: 60_000,
                },
            )
        } else {
            api::schema::Method::AgentConversationWake(
                api::schema::AgentConversationMutationParams {
                    conversation_id: record.id,
                    expected_revision: record.revision,
                },
            )
        };
        server.handle_client_shell_endpoint_request(
            92,
            server.client_shell_boot_id.clone(),
            Box::new(api::schema::Request {
                id: format!("snooze-{snooze}"),
                method,
            }),
        );
        let response = tokio::time::timeout(Duration::from_secs(1), server.server_event_rx.recv())
            .await
            .unwrap()
            .unwrap();
        server.handle_server_event(response);
        let ServerMessage::ClientShellEndpointResponseChunk { data, .. } =
            read_server_message(receiver.recv_timeout(Duration::from_secs(1)).unwrap())
        else {
            panic!("expected endpoint response");
        };
        let response: serde_json::Value = serde_json::from_slice(&data).unwrap();
        assert!(response.get("error").is_none(), "{response}");
        let record = server.app.agent_inbox.records().remove(0);
        assert_eq!(record.snoozed_until_ms.is_some(), snooze);
        assert_eq!(record.execution, original.execution);
        assert_eq!(record.attachments, original.attachments);
        assert_eq!(server.foreground_client_id, foreground_before);
        assert_eq!(server.default_shell_target(), focus_before);
        assert!(!server.clients[&92].shell_surface_active);
    }
}

fn headless_with_pending_codex_end() -> (HeadlessServer, crate::layout::PaneId) {
    let mut server = test_headless_server();
    server.app.state.workspaces = vec![crate::workspace::Workspace::test_new("native-end")];
    server.app.state.ensure_test_terminals();
    server.app.state.active = Some(0);
    let pane = server.app.state.workspaces[0].tabs[0].root_pane;
    let terminal_id = server
        .app
        .find_pane(pane)
        .unwrap()
        .1
        .attached_terminal_id
        .clone();
    server
        .app
        .state
        .terminals
        .get_mut(&terminal_id)
        .unwrap()
        .set_persisted_agent_session(crate::agent_resume::PersistedAgentSession {
            source: "herdr:codex".into(),
            agent: "codex".into(),
            session_ref: crate::agent_resume::AgentSessionRef::id("headless-native").unwrap(),
        });
    server.handle_internal_event_with_forwarding(AppEvent::AgentProcessDetected {
        pane_id: pane,
        agent: crate::detect::Agent::Codex,
        observed_at: Instant::now() - Duration::from_millis(5),
    });
    server
        .app
        .state
        .terminals
        .get_mut(&terminal_id)
        .unwrap()
        .set_detected_state(
            Some(crate::detect::Agent::Codex),
            crate::detect::AgentState::Idle,
        );
    server.app.sync_agent_conversations();
    let request: api::schema::Request = serde_json::from_value(serde_json::json!({
        "id": "native-end", "method": "pane.report_agent_session_end",
        "params": {
            "pane_id": server.app.public_pane_id(0, pane).unwrap(),
            "source": "herdr:codex", "agent": "codex", "seq": 20,
            "agent_session_id": "headless-native", "reason": "other",
            "transcript_path": "/tmp/headless-native.jsonl",
            "reported_at_unix_ms": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64,
        },
    })).unwrap();
    let response: serde_json::Value =
        serde_json::from_str(&server.app.handle_api_request(request)).unwrap();
    assert!(response.get("error").is_none(), "{response}");
    assert_eq!(server.app.pending_codex_session_ends.len(), 1);
    (server, pane)
}

#[tokio::test]
async fn native_codex_end_survives_headless_pty_prepublication_and_settles_once() {
    let (mut server, pane_id) = headless_with_pending_codex_end();
    server.handle_internal_event_with_forwarding(AppEvent::PaneDied {
        pane_id,
        exit_reason: crate::platform::ChildExitReason::Exited,
    });
    let record = &server.app.agent_inbox.records()[0];
    assert_eq!(record.disposition, crate::agent_inbox::Disposition::Settled);
    assert_eq!(record.execution, crate::agent_inbox::ExecutionState::Closed);
    assert!(record.attachments.is_empty());
    assert!(server.app.pending_codex_session_ends.is_empty());
    let revision = record.revision;
    server.app.sync_agent_conversations();
    assert_eq!(server.app.agent_inbox.records()[0].revision, revision);
}

#[tokio::test]
async fn server_stop_clears_native_end_proof_before_draining_process_release() {
    let (mut server, pane_id) = headless_with_pending_codex_end();
    server
        .server_stop
        .request(crate::server::shutdown::ShutdownReason::ApiStop { caller: None });
    server.handle_internal_event_with_forwarding(AppEvent::StateChanged {
        pane_id,
        agent: Some(crate::detect::Agent::Codex),
        state: crate::detect::AgentState::Unknown,
        visible_blocker: false,
        visible_working: false,
        process_exited: true,
        observed_at: Instant::now(),
    });
    assert_eq!(
        server.app.agent_inbox.records()[0].disposition,
        crate::agent_inbox::Disposition::Inbox
    );
    assert!(server.app.pending_codex_session_ends.is_empty());
}
