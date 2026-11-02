//! Server-owned conversation lifecycle integration. Runs on semantic events, never rendering.
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::agent_inbox::{
    ConversationAction, ConversationAttachment, ConversationObservation, ExecutionState, InboxError,
};
use crate::api::schema::{AgentConversationMutationParams, ResponseResult};
use crate::protocol::endpoint::EndpointAgentConversations;

use super::api::responses::{encode_error, encode_success};
use super::App;

pub(super) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}

impl App {
    pub(crate) fn agent_conversations_projection(&self) -> EndpointAgentConversations {
        self.agent_conversations_page(0, usize::MAX)
    }

    fn agent_conversations_page(&self, offset: usize, limit: usize) -> EndpointAgentConversations {
        let mut records = self.agent_inbox.records();
        records.sort_by(|left, right| {
            left.attachments
                .is_empty()
                .cmp(&right.attachments.is_empty())
                .then_with(|| {
                    right
                        .attention_returned_at_ms
                        .cmp(&left.attention_returned_at_ms)
                })
                .then_with(|| left.id.cmp(&right.id))
        });
        let total_count = records.len();
        let mut conversations = Vec::new();
        // Leave room for the JSON envelope, escaping the control payload, and its frame.
        let budget = crate::protocol::MAX_FRAME_SIZE / 3;
        let mut used = 1024;
        for record in records.into_iter().skip(offset).take(limit) {
            let size = serde_json::to_vec(&record).map_or(budget, |data| data.len());
            if used + size > budget {
                break;
            }
            used += size;
            conversations.push(record);
        }
        let consumed = offset.saturating_add(conversations.len());
        let next_offset = (consumed < total_count).then_some(consumed);
        let recovery_error = self
            .agent_inbox
            .recovery_error
            .clone()
            .or_else(|| self.agent_inbox_error.clone());
        EndpointAgentConversations {
            boot_id: self.agent_inbox_boot_id.clone(),
            revision: self.agent_inbox.revision(),
            conversations,
            total_count,
            next_offset,
            recovery_error,
        }
    }

    pub(crate) fn sync_agent_conversations(&mut self) {
        let queued_panes: std::collections::HashSet<_> = self
            .pending_conversation_prompts
            .keys()
            .filter_map(|id| self.agent_inbox.get(id))
            .flat_map(|record| {
                record
                    .attachments
                    .iter()
                    .map(|attachment| attachment.pane_id)
            })
            .collect();
        let mut observations = Vec::new();
        for (ws_idx, workspace) in self.state.workspaces.iter().enumerate() {
            for (tab_idx, tab) in workspace.tabs.iter().enumerate() {
                for (pane_id, pane) in &tab.panes {
                    let Some(terminal) = self.state.terminals.get(&pane.attached_terminal_id)
                    else {
                        continue;
                    };
                    if !terminal.is_agent_terminal() {
                        continue;
                    }
                    let Some(info) = self.agent_info(ws_idx, *pane_id) else {
                        continue;
                    };
                    let provider = info
                        .agent
                        .clone()
                        .or(info.display_agent.clone())
                        .unwrap_or_default();
                    let source = info
                        .agent_session
                        .as_ref()
                        .map(|session| session.source.clone())
                        .unwrap_or_default();
                    let session_ref =
                        info.agent_session
                            .map(|session| crate::agent_resume::AgentSessionRef {
                                kind: session.kind,
                                value: session.value,
                            });
                    let execution = match terminal.state {
                        crate::detect::AgentState::Working => ExecutionState::Working,
                        crate::detect::AgentState::Idle | crate::detect::AgentState::Blocked => {
                            ExecutionState::Idle
                        }
                        crate::detect::AgentState::Unknown => ExecutionState::Unknown,
                    };
                    observations.push(ConversationObservation {
                        provider,
                        source,
                        session_ref,
                        generation: terminal.id.to_string(),
                        attachment: ConversationAttachment {
                            pane_id: pane_id.raw(),
                            terminal_id: terminal.id.to_string(),
                            public_pane_id: info.pane_id.clone(),
                            workspace_label: workspace.display_name_from_terminals(&self.state.terminals),
                            tab_label: workspace.tab_display_name(tab_idx).unwrap_or_default(),
                            workspace_id: workspace.id.clone(),
                            tab_id: self.public_tab_id(ws_idx, tab_idx).unwrap_or_default(),
                            workspace_location_id: workspace.location_id.clone(),
                            tab_location_id: tab.location_id.clone(),
                            cwd: terminal.cwd.to_string_lossy().into_owned(),
                        },
                        title: info.title.or(info.name).unwrap_or_else(|| "Agent conversation".into()),
                        execution,
                        queued_human_work: queued_panes.contains(&pane_id.raw()) || terminal.managed_agent_launch_pending(),
                        blocking_request: terminal.state == crate::detect::AgentState::Blocked,
                        failure: false,
                        capability_limits: vec![
                            "Provider human submissions cannot be distinguished from automatic continuation; use explicit Reopen for terminal input.".into(),
                            "Provider status does not distinguish typed approval, question, failure, or queued human work; blocked status requires attention.".into(),
                        ],
                    });
                }
            }
        }
        let result = self.agent_inbox.reconcile(&observations, now_ms());
        self.record_agent_inbox_result(result);
        self.poll_agent_conversation_resumes();
        self.sync_agent_inbox_deadline();
    }

    fn record_agent_inbox_result(&mut self, result: Result<bool, InboxError>) -> bool {
        match result {
            Ok(changed) => {
                if changed {
                    self.agent_inbox_error = None;
                    self.render_dirty.request_generic();
                    self.render_notify.notify_one();
                }
                changed
            }
            Err(error) => {
                let message = error.to_string();
                let changed = self.agent_inbox_error.as_ref() != Some(&message);
                if changed {
                    tracing::warn!(%message, "conversation lifecycle update failed");
                    self.agent_inbox_error = Some(message);
                    self.render_dirty.request_generic();
                    self.render_notify.notify_one();
                }
                changed
            }
        }
    }

    pub(super) fn sync_agent_inbox_deadline(&mut self) {
        self.agent_inbox_deadline = self.agent_inbox.next_deadline_ms().map(|deadline| {
            let delay = Duration::from_millis(deadline.saturating_sub(now_ms()));
            let delay = if self.agent_inbox_error.is_some() {
                delay.max(Duration::from_secs(1))
            } else {
                delay
            };
            Instant::now() + delay
        });
    }

    pub(crate) fn tick_agent_conversations(&mut self, now: Instant) -> bool {
        if self
            .agent_inbox_deadline
            .is_none_or(|deadline| now < deadline)
        {
            return false;
        }
        let result = self.agent_inbox.tick(now_ms());
        let changed = self.record_agent_inbox_result(result);
        let resumed = self.poll_agent_conversation_resumes();
        self.sync_agent_inbox_deadline();
        changed | resumed
    }

    pub(super) fn conversation_id_for_pane(
        &self,
        pane_id: crate::layout::PaneId,
    ) -> Option<String> {
        self.agent_inbox
            .records()
            .into_iter()
            .find(|record| {
                record
                    .attachments
                    .iter()
                    .any(|attachment| attachment.pane_id == pane_id.raw())
            })
            .map(|record| record.id)
    }

    pub(super) fn finish_conversation_prompt(
        &mut self,
        conversation_id: &str,
        submitted: bool,
    ) -> Result<(), String> {
        if let Some(count) = self.pending_conversation_prompts.get_mut(conversation_id) {
            *count -= 1;
            if *count == 0 {
                self.pending_conversation_prompts.remove(conversation_id);
            }
        }
        let result = if submitted {
            self.agent_inbox
                .human_submission(conversation_id, now_ms())
                .map_err(|error| error.to_string())
        } else {
            Ok(())
        };
        if let Err(error) = &result {
            self.agent_inbox_error = Some(error.clone());
        }
        self.sync_agent_conversations();
        result
    }

    pub(super) fn handle_agent_conversation_list(
        &mut self,
        id: String,
        params: crate::api::schema::AgentConversationListParams,
    ) -> String {
        if !(1..=1000).contains(&params.limit) {
            return encode_error(
                id,
                "invalid_conversation_page",
                "limit must be between 1 and 1000",
            );
        }
        match serde_json::to_value(self.agent_conversations_page(params.offset, params.limit)) {
            Ok(projection) => encode_success(id, ResponseResult::AgentConversations { projection }),
            Err(error) => encode_error(id, "serialization_error", error.to_string()),
        }
    }

    pub(super) fn handle_agent_conversation_open(
        &mut self,
        id: String,
        params: crate::api::schema::AgentConversationOpenParams,
    ) -> String {
        let Some(record) = self.agent_inbox.get(&params.conversation_id) else {
            return encode_error(id, "conversation_not_found", "Conversation not found.");
        };
        if record.revision != params.expected_revision {
            return encode_error(
                id,
                "conversation_revision_conflict",
                "Conversation changed. Refresh and try Open again.",
            );
        }
        let mut attachments: Vec<_> = record.attachments.iter().collect();
        if matches!(
            record.resume_state,
            crate::agent_inbox::ResumeState::Pending | crate::agent_inbox::ResumeState::Uncertain
        ) {
            if let Some(target) = &record.resume_target {
                if !attachments
                    .iter()
                    .any(|attachment| attachment.terminal_id == target.terminal_id)
                {
                    attachments.push(target);
                }
            }
        }
        let attachment = match params.attachment_terminal_id.as_deref() {
            Some(terminal_id) => attachments
                .into_iter()
                .find(|attachment| attachment.terminal_id == terminal_id),
            None if attachments.len() == 1 => attachments.first().copied(),
            None if attachments.len() > 1 => {
                return encode_error(
                    id,
                    "conversation_attachment_required",
                    "Choose an attachment to open this conversation.",
                )
            }
            None => None,
        };
        let Some(attachment) = attachment.filter(|attachment| !attachment.terminal_id.is_empty())
        else {
            return encode_error(
                id,
                "conversation_attachment_unavailable",
                "The selected conversation attachment is unavailable.",
            );
        };
        let conversation_id = record.id.clone();
        let target = self
            .state
            .workspaces
            .iter()
            .enumerate()
            .find_map(|(workspace, ws)| {
                ws.tabs.iter().find_map(|tab| {
                    tab.panes.iter().find_map(|(pane_id, pane)| {
                        (pane.attached_terminal_id.as_str() == attachment.terminal_id
                            && self
                                .state
                                .terminals
                                .contains_key(&pane.attached_terminal_id))
                        .then_some((workspace, *pane_id))
                    })
                })
            });
        let Some((workspace, pane_id)) = target else {
            return encode_error(
                id,
                "conversation_attachment_unavailable",
                "The selected terminal generation is no longer attached.",
            );
        };
        let Some(public_pane_id) = self.public_pane_id(workspace, pane_id) else {
            return encode_error(
                id,
                "conversation_attachment_unavailable",
                "The selected pane is unavailable.",
            );
        };
        self.state.focus_pane_in_workspace(workspace, pane_id);
        self.state.mark_active_tab_seen();
        self.state.mode = crate::app::Mode::Terminal;
        encode_success(
            id,
            ResponseResult::AgentConversationOpened {
                conversation_id,
                pane_id: public_pane_id,
            },
        )
    }

    pub(super) fn handle_agent_conversation_mutation(
        &mut self,
        id: String,
        params: AgentConversationMutationParams,
        action: ConversationAction,
    ) -> String {
        match self.agent_inbox.mutate(
            &params.conversation_id,
            params.expected_revision,
            action,
            now_ms(),
        ) {
            Ok(record) => {
                self.sync_agent_inbox_deadline();
                match serde_json::to_value(record) {
                    Ok(conversation) => {
                        encode_success(id, ResponseResult::AgentConversation { conversation })
                    }
                    Err(error) => encode_error(id, "serialization_error", error.to_string()),
                }
            }
            Err(error) => {
                let code = match &error {
                    InboxError::NotFound => "conversation_not_found",
                    InboxError::Conflict { .. } => "conversation_revision_conflict",
                    InboxError::Ineligible(_) => "conversation_ineligible",
                    InboxError::Persistence(_) | InboxError::PersistenceUncertain(_) => {
                        "conversation_persistence_failed"
                    }
                    InboxError::Recovery(_) => "conversation_registry_invalid",
                };
                encode_error(id, code, error.to_string())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_inbox::{ConversationSection, Disposition};
    use crate::api::schema::{AgentConversationListParams, Method, Request};
    use crate::detect::{Agent, AgentState};

    fn app_with_native_agent() -> (App, crate::layout::PaneId) {
        let (_, api_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::new(
            &crate::config::Config::default(),
            super::super::AppPolicy::TEST,
            None,
            api_rx,
            crate::api::EventHub::default(),
        );
        app.state
            .workspaces
            .push(crate::workspace::Workspace::test_new("inbox"));
        app.state.ensure_test_terminals();
        app.state.active = Some(0);
        let pane_id = app.state.workspaces[0].tabs[0].root_pane;
        let terminal_id = app.state.workspaces[0].tabs[0].panes[&pane_id]
            .attached_terminal_id
            .clone();
        let terminal = app.state.terminals.get_mut(&terminal_id).unwrap();
        terminal.set_detected_state(Some(Agent::Codex), AgentState::Working);
        terminal.set_persisted_agent_session(crate::agent_resume::PersistedAgentSession {
            source: "herdr:codex".into(),
            agent: "codex".into(),
            session_ref: crate::agent_resume::AgentSessionRef::id("native-test").unwrap(),
        });
        app.sync_agent_conversations();
        (app, pane_id)
    }

    fn call(app: &mut App, method: Method) -> serde_json::Value {
        serde_json::from_str(&app.handle_api_request(Request {
            id: "test".into(),
            method,
        }))
        .unwrap()
    }

    #[test]
    fn conversation_open_rejects_a_replaced_terminal_without_changing_focus() {
        let (mut app, pane_id) = app_with_native_agent();
        let record = app.agent_inbox.records().pop().unwrap();
        app.state
            .workspaces
            .push(crate::workspace::Workspace::test_new("other"));
        app.state.ensure_test_terminals();
        app.state.active = Some(1);
        let replacement = crate::terminal::TerminalId::alloc();
        app.state.terminals.insert(
            replacement.clone(),
            crate::terminal::TerminalState::new(replacement.clone(), "/".into()),
        );
        app.state.workspaces[0]
            .pane_state_mut(pane_id)
            .unwrap()
            .attached_terminal_id = replacement;
        let response: serde_json::Value =
            serde_json::from_str(&app.handle_agent_conversation_open(
                "swap".into(),
                crate::api::schema::AgentConversationOpenParams {
                    conversation_id: record.id,
                    expected_revision: record.revision,
                    attachment_terminal_id: None,
                },
            ))
            .unwrap();
        assert_eq!(
            response["error"]["code"],
            "conversation_attachment_unavailable"
        );
        assert_eq!(app.state.active, Some(1));
    }

    #[test]
    fn conversation_open_pending_target_does_not_confirm_or_reopen() {
        let (mut app, pane_id) = app_with_native_agent();
        let terminal_id = app.state.workspaces[0].tabs[0].panes[&pane_id]
            .attached_terminal_id
            .clone();
        app.state
            .terminals
            .get_mut(&terminal_id)
            .unwrap()
            .set_detected_state(Some(Agent::Codex), AgentState::Idle);
        app.sync_agent_conversations();
        let record = app.agent_inbox.records().pop().unwrap();
        let record = app
            .agent_inbox
            .mutate(
                &record.id,
                record.revision,
                ConversationAction::Settle,
                now_ms(),
            )
            .unwrap();
        app.state
            .terminals
            .get_mut(&terminal_id)
            .unwrap()
            .clear_agent_runtime_identity_after_respawn();
        app.sync_agent_conversations();
        let closed_revision = app.agent_inbox.get(&record.id).unwrap().revision;
        let pending = app
            .agent_inbox
            .mutate(
                &record.id,
                closed_revision,
                ConversationAction::BeginResume,
                now_ms(),
            )
            .unwrap();
        let pending = app
            .agent_inbox
            .mutate(
                &pending.id,
                pending.revision,
                ConversationAction::ResumeLaunched(Box::new(record.saved_location)),
                now_ms(),
            )
            .unwrap();
        app.state
            .workspaces
            .push(crate::workspace::Workspace::test_new("other"));
        app.state.ensure_test_terminals();
        app.state.active = Some(1);
        let response = call(
            &mut app,
            Method::AgentConversationOpen(crate::api::schema::AgentConversationOpenParams {
                conversation_id: pending.id.clone(),
                expected_revision: pending.revision,
                attachment_terminal_id: None,
            }),
        );
        assert_eq!(
            response["result"]["pane_id"],
            app.public_pane_id(0, pane_id).unwrap()
        );
        assert_eq!(app.state.active, Some(0));
        let after = app.agent_inbox.get(&pending.id).unwrap();
        assert_eq!(after.disposition, Disposition::Settled);
        assert_eq!(after.resume_state, crate::agent_inbox::ResumeState::Pending);
        assert_eq!(after.revision, pending.revision);
    }

    #[test]
    fn conversation_open_duplicates_require_explicit_terminal_selection() {
        let (mut app, _) = app_with_native_agent();
        let session = app.agent_inbox.records()[0].session_ref.clone().unwrap();
        app.state
            .workspaces
            .push(crate::workspace::Workspace::test_new("duplicate"));
        app.state.ensure_test_terminals();
        let pane_id = app.state.workspaces[1].tabs[0].root_pane;
        let terminal_id = app.state.workspaces[1].tabs[0].panes[&pane_id]
            .attached_terminal_id
            .clone();
        let terminal = app.state.terminals.get_mut(&terminal_id).unwrap();
        terminal.set_detected_state(Some(Agent::Codex), AgentState::Working);
        terminal.set_persisted_agent_session(crate::agent_resume::PersistedAgentSession {
            source: "herdr:codex".into(),
            agent: "codex".into(),
            session_ref: session,
        });
        app.sync_agent_conversations();
        let record = app.agent_inbox.records().pop().unwrap();
        assert_eq!(record.attachments.len(), 2);
        let mut params = crate::api::schema::AgentConversationOpenParams {
            conversation_id: record.id,
            expected_revision: record.revision,
            attachment_terminal_id: None,
        };
        let response = call(&mut app, Method::AgentConversationOpen(params.clone()));
        assert_eq!(
            response["error"]["code"],
            "conversation_attachment_required"
        );
        assert_eq!(app.state.active, Some(0));
        params.attachment_terminal_id = Some(terminal_id.to_string());
        let response = call(&mut app, Method::AgentConversationOpen(params));
        assert_eq!(
            response["result"]["pane_id"],
            app.public_pane_id(1, pane_id).unwrap()
        );
        assert_eq!(app.state.active, Some(1));
    }

    #[test]
    fn focused_completion_keeps_inbox_membership_after_read_acknowledgement() {
        let (mut app, pane_id) = app_with_native_agent();
        assert_eq!(
            app.agent_inbox.records()[0].section,
            ConversationSection::Working
        );
        app.handle_internal_event(crate::events::AppEvent::StateChanged {
            pane_id,
            agent: Some(Agent::Codex),
            state: AgentState::Idle,
            visible_blocker: false,
            visible_working: false,
            process_exited: false,
            observed_at: Instant::now(),
        });
        app.state.workspaces[0]
            .pane_state_mut(pane_id)
            .unwrap()
            .seen = true;
        assert_eq!(
            app.agent_inbox.records()[0].section,
            ConversationSection::Working
        );
        assert!(
            app.agent_inbox_deadline.is_some(),
            "grace remains scheduled without attached clients"
        );
        app.agent_inbox.tick(now_ms() + 2_001).unwrap();
        app.sync_agent_conversations();
        assert_eq!(
            app.agent_inbox.records()[0].section,
            ConversationSection::Inbox
        );
    }

    #[test]
    fn api_settle_conflicts_retries_and_reopen_preserve_terminal_ownership() {
        let (mut app, pane_id) = app_with_native_agent();
        let record = app.agent_inbox.records()[0].clone();
        let rejected = call(
            &mut app,
            Method::AgentConversationSettle(AgentConversationMutationParams {
                conversation_id: record.id.clone(),
                expected_revision: record.revision,
            }),
        );
        assert_eq!(rejected["error"]["code"], "conversation_ineligible");
        let terminal_id = app.state.workspaces[0].tabs[0].panes[&pane_id]
            .attached_terminal_id
            .clone();
        app.state
            .terminals
            .get_mut(&terminal_id)
            .unwrap()
            .set_detected_state(Some(Agent::Codex), AgentState::Idle);
        app.sync_agent_conversations();
        let record = app.agent_inbox.records()[0].clone();
        let params = AgentConversationMutationParams {
            conversation_id: record.id.clone(),
            expected_revision: record.revision,
        };
        let settled = call(&mut app, Method::AgentConversationSettle(params.clone()));
        assert!(settled.get("error").is_none(), "{settled}");
        let retry = call(&mut app, Method::AgentConversationSettle(params.clone()));
        assert_eq!(settled["result"], retry["result"]);
        let stale = call(&mut app, Method::AgentConversationReopen(params));
        assert_eq!(stale["error"]["code"], "conversation_revision_conflict");
        let record = app.agent_inbox.records()[0].clone();
        let reopened = call(
            &mut app,
            Method::AgentConversationReopen(AgentConversationMutationParams {
                conversation_id: record.id,
                expected_revision: record.revision,
            }),
        );
        assert!(reopened.get("error").is_none());
        assert_eq!(app.agent_inbox.records()[0].disposition, Disposition::Inbox);
        assert_eq!(
            app.state.workspaces[0].tabs[0].panes[&pane_id].attached_terminal_id,
            terminal_id
        );
    }

    #[test]
    fn closing_pane_retains_native_history_with_public_location() {
        let (mut app, pane_id) = app_with_native_agent();
        let public_id = app.public_pane_id(0, pane_id).unwrap();
        app.handle_internal_event(crate::events::AppEvent::PaneDied {
            pane_id,
            exit_reason: crate::platform::ChildExitReason::Exited,
        });
        let record = app.agent_inbox.records().pop().unwrap();
        assert!(record.attachments.is_empty());
        assert_eq!(record.execution, ExecutionState::Closed);
        assert_eq!(record.saved_location.public_pane_id, public_id);
        let page = call(
            &mut app,
            Method::AgentConversationList(AgentConversationListParams {
                offset: 0,
                limit: 1,
            }),
        );
        assert_eq!(
            page["result"]["projection"]["conversations"][0]["id"],
            record.id
        );
    }

    #[test]
    fn overdue_completion_retries_wait_after_a_storage_error() {
        let (mut app, _) = app_with_native_agent();
        let record = app.agent_inbox.records().pop().unwrap();
        app.agent_inbox
            .reconcile(
                &[ConversationObservation {
                    provider: record.provider,
                    source: record.source,
                    session_ref: record.session_ref,
                    generation: "test".into(),
                    attachment: record.saved_location,
                    title: record.title,
                    execution: ExecutionState::Idle,
                    queued_human_work: false,
                    blocking_request: false,
                    failure: false,
                    capability_limits: Vec::new(),
                }],
                now_ms().saturating_sub(3_000),
            )
            .unwrap();
        assert!(app.agent_inbox.next_deadline_ms().unwrap() < now_ms());
        app.record_agent_inbox_result(Err(InboxError::Persistence("disk unavailable".into())));
        let before = Instant::now();
        app.sync_agent_inbox_deadline();
        assert!(app.agent_inbox_deadline.unwrap().duration_since(before) >= Duration::from_secs(1));
    }

    #[test]
    fn known_queued_prompt_blocks_settlement_and_verified_submission_reopens() {
        let (mut app, pane_id) = app_with_native_agent();
        let terminal_id = app.state.workspaces[0].tabs[0].panes[&pane_id]
            .attached_terminal_id
            .clone();
        app.state
            .terminals
            .get_mut(&terminal_id)
            .unwrap()
            .set_detected_state(Some(Agent::Codex), AgentState::Idle);
        app.sync_agent_conversations();
        let record = app.agent_inbox.records()[0].clone();
        app.agent_inbox
            .mutate(
                &record.id,
                record.revision,
                ConversationAction::Settle,
                now_ms(),
            )
            .unwrap();
        app.pending_conversation_prompts
            .insert(record.id.clone(), 1);
        app.sync_agent_conversations();
        assert!(app.agent_inbox.records()[0].queued_human_work);
        app.finish_conversation_prompt(&record.id, true).unwrap();
        let record = app.agent_inbox.records().pop().unwrap();
        assert_eq!(record.disposition, Disposition::Inbox);
        assert!(record.last_human_submission_at_ms.is_some());
        assert!(!record.queued_human_work);
    }
}
