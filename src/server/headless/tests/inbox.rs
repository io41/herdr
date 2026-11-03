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
