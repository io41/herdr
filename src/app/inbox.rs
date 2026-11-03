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

fn conversation_error(id: String, error: InboxError) -> String {
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

fn conversation_title(
    info: &crate::api::schema::AgentInfo,
    pane_label: Option<&str>,
    tab_label: Option<&str>,
) -> String {
    [
        info.title.as_deref(),
        info.name.as_deref(),
        pane_label,
        tab_label.filter(|label| label.parse::<usize>().is_err()),
        info.terminal_title_stripped.as_deref(),
    ]
    .into_iter()
    .flatten()
    .map(str::trim)
    .find(|title| {
        !title.is_empty()
            && !["agent conversation", "codex", "claude", "agent", "terminal"]
                .iter()
                .any(|generic| title.eq_ignore_ascii_case(generic))
    })
    .unwrap_or("Agent conversation")
    .to_string()
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
                for (pane_index, pane_id) in tab.layout.pane_ids().into_iter().enumerate() {
                    let Some(pane) = tab.panes.get(&pane_id) else {
                        continue;
                    };
                    let Some(terminal) = self.state.terminals.get(&pane.attached_terminal_id)
                    else {
                        continue;
                    };
                    if !terminal.is_agent_terminal() {
                        continue;
                    }
                    let Some(info) = self.agent_info(ws_idx, pane_id) else {
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
                    let session_ref = info.agent_session.as_ref().map(|session| {
                        crate::agent_resume::AgentSessionRef {
                            kind: session.kind,
                            value: session.value.clone(),
                        }
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
                            pane_label: terminal.manual_label.clone(),
                            pane_number: Some(pane_index + 1),
                            workspace_location_id: workspace.location_id.clone(),
                            tab_location_id: tab.location_id.clone(),
                            cwd: terminal.cwd.to_string_lossy().into_owned(),
                        },
                        title: conversation_title(&info, terminal.manual_label.as_deref(), workspace.tab_display_name(tab_idx).as_deref()),
                        execution,
                        queued_human_work: queued_panes.contains(&pane_id.raw()) || terminal.managed_agent_launch_pending()
                            || self.lookup_runtime_sender(ws_idx, pane_id).is_some_and(|runtime| runtime.has_pending_human_submission()),
                        blocking_request: terminal.state == crate::detect::AgentState::Blocked,
                        failure: false,
                        capability_limits: vec![
                            "Terminal drafts, unsupported editing input, and mouse button actions forwarded to the agent prevent settlement until a submitted turn completes or the process exits.".into(),
                            "Provider status does not distinguish typed approval, question, failure, or queued human work; blocked status requires attention.".into(),
                            "Snooze wakes on a new blocked-status edge; providers do not supply request event identities or freshness, so repeated requests while status stays blocked cannot wake early.".into(),
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
            let delay = if self.agent_inbox.has_snoozed_conversations() {
                delay.min(Duration::from_secs(60))
            } else {
                delay
            };
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

    pub(crate) fn settle_conversation_after_confirmed_exit(
        &mut self,
        conversation_id: &str,
        terminal_id: &str,
        session: &crate::agent_resume::PersistedAgentSession,
        last_submission: Option<u64>,
    ) {
        let result = self.agent_inbox.settle_after_confirmed_exit(
            conversation_id,
            terminal_id,
            session,
            last_submission,
            now_ms(),
        );
        self.record_agent_inbox_result(result);
        self.sync_agent_inbox_deadline();
    }

    pub(crate) fn reopen_conversation_after_manual_resume(
        &mut self,
        terminal_id: &str,
        session: &crate::agent_resume::PersistedAgentSession,
    ) -> bool {
        let Some(record) = self.agent_inbox.records().into_iter().find(|record| {
            record.provider == session.agent
                && record.source == session.source
                && record.session_ref.as_ref() == Some(&session.session_ref)
                && record.resume_state == crate::agent_inbox::ResumeState::None
                && record
                    .attachments
                    .iter()
                    .any(|attachment| attachment.terminal_id == terminal_id)
        }) else {
            return false;
        };
        if record.disposition == crate::agent_inbox::Disposition::Inbox {
            return true;
        }
        let result = self
            .agent_inbox
            .mutate(
                &record.id,
                record.revision,
                ConversationAction::Reopen,
                now_ms(),
            )
            .map(|_| true);
        let completed = result.is_ok();
        self.record_agent_inbox_result(result);
        self.sync_agent_inbox_deadline();
        completed
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
        if submitted && result.is_ok() {
            self.pending_codex_session_ends
                .retain(|_, pending| pending.conversation_id != conversation_id);
        }
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

    pub(super) fn handle_agent_conversation_settle_and_close(
        &mut self,
        id: String,
        params: AgentConversationMutationParams,
    ) -> String {
        let Some(record) = self.agent_inbox.get(&params.conversation_id).cloned() else {
            return encode_error(id, "conversation_not_found", "Conversation not found.");
        };
        // A retry may acknowledge the closed postcondition, never close a replacement generation.
        if record.disposition == crate::agent_inbox::Disposition::Settled
            && record.execution == ExecutionState::Closed
            && record.attachments.is_empty()
        {
            return self.handle_agent_conversation_mutation(
                id,
                params,
                ConversationAction::SettleAndClose,
            );
        }
        if record.revision != params.expected_revision {
            return encode_error(
                id,
                "conversation_revision_conflict",
                "Conversation changed. Refresh before settling it.",
            );
        }
        let live_native_attachments = self
            .collect_agent_infos()
            .iter()
            .filter(|info| {
                info.agent_session.as_ref().is_some_and(|session| {
                    session.source == record.source
                        && session.agent == record.provider
                        && record.session_ref.as_ref().is_some_and(|saved| {
                            saved.kind == session.kind && saved.value == session.value
                        })
                })
            })
            .count();
        if record.attachments.len() > 1 || live_native_attachments > 1 {
            return encode_error(
                id,
                "conversation_ineligible",
                "This conversation has multiple attachments. Nothing was closed.",
            );
        }
        let target = if let Some(attachment) = record.attachments.first() {
            let matches: Vec<_> = self
                .state
                .workspaces
                .iter()
                .enumerate()
                .flat_map(|(workspace, ws)| {
                    ws.tabs.iter().flat_map(move |tab| {
                        tab.panes.iter().filter_map(move |(pane_id, pane)| {
                            (pane.attached_terminal_id.as_str() == attachment.terminal_id)
                                .then_some((workspace, *pane_id))
                        })
                    })
                })
                .collect();
            let [(workspace, pane_id)] = matches.as_slice() else {
                return encode_error(id, "conversation_attachment_unavailable", "The saved terminal generation is missing or attached more than once. Nothing was closed.");
            };
            let Some(info) = self.agent_info(*workspace, *pane_id) else {
                return encode_error(
                    id,
                    "conversation_attachment_unavailable",
                    "The original agent is no longer attached. Nothing was closed.",
                );
            };
            let same_session = info.agent_session.as_ref().is_some_and(|session| {
                session.source == record.source
                    && session.agent == record.provider
                    && record.session_ref.as_ref().is_some_and(|saved| {
                        saved.kind == session.kind && saved.value == session.value
                    })
            });
            let terminal = &self.state.terminals[&self.state.workspaces[*workspace]
                .pane_state(*pane_id)
                .unwrap()
                .attached_terminal_id];
            if self
                .lookup_runtime_sender(*workspace, *pane_id)
                .is_some_and(|runtime| runtime.has_pending_human_submission())
            {
                return encode_error(id, "conversation_ineligible", "A draft or unconfirmed terminal input remains. Finish and submit the input, then let the turn complete, or wait for the agent process to exit. Nothing was closed.");
            }
            if !same_session
                || terminal.state != crate::detect::AgentState::Idle
                || terminal.managed_agent_launch_pending()
                || self
                    .pending_conversation_prompts
                    .get(&record.id)
                    .is_some_and(|count| *count > 0)
            {
                return encode_error(id, "conversation_ineligible", "The original agent is running, blocked, has queued work, or its native session changed. Nothing was closed.");
            }
            if self
                .state
                .close_pane_would_close_workspace(*workspace, *pane_id)
                && (self.state.workspace_close_indices(*workspace).len() > 1
                    || self.state.confirm_implicit_worktree_group_close(*workspace))
            {
                return encode_error(
                    id,
                    "conversation_ineligible",
                    "Closing this pane would also close a worktree group. Nothing was closed.",
                );
            }
            let Some(public_pane_id) = self.public_pane_id(*workspace, *pane_id) else {
                return encode_error(
                    id,
                    "conversation_attachment_unavailable",
                    "The original pane is unavailable. Nothing was closed.",
                );
            };
            Some(crate::api::schema::PaneTarget {
                pane_id: public_pane_id,
            })
        } else {
            None
        };
        if let Some(target) = &target {
            if let Err(error) = self.checkpoint_session_before_conversation_close() {
                return encode_error(
                    id,
                    "conversation_persistence_failed",
                    format!("The current session could not be saved. Nothing was closed: {error}"),
                );
            }
            let exact_target =
                self.parse_pane_id(&target.pane_id)
                    .is_some_and(|(workspace, pane)| {
                        self.state
                            .terminal_id_for_pane(workspace, pane)
                            .is_some_and(|terminal| {
                                terminal.as_str() == record.attachments[0].terminal_id
                            })
                            && !self
                                .lookup_runtime_sender(workspace, pane)
                                .is_some_and(|runtime| runtime.has_pending_human_submission())
                    });
            if !exact_target {
                return encode_error(
                    id,
                    "conversation_ineligible",
                    "The terminal generation changed or has new human input. Nothing was closed.",
                );
            }
        }
        // App owns the revision and exact terminal generation throughout validation, durable commit, and close.
        let settled = match self.agent_inbox.mutate(
            &params.conversation_id,
            params.expected_revision,
            ConversationAction::SettleAndClose,
            now_ms(),
        ) {
            Ok(record) => record,
            Err(error) => return conversation_error(id, error),
        };
        self.discard_pending_codex_manual_resumes_for(&settled);
        self.sync_agent_inbox_deadline();
        if let Some(target) = target {
            if let Err(error) = self.close_pane(id.clone(), &target) {
                self.sync_agent_conversations();
                return error;
            }
        }
        match serde_json::to_value(settled) {
            Ok(conversation) => {
                encode_success(id, ResponseResult::AgentConversation { conversation })
            }
            Err(error) => encode_error(id, "serialization_error", error.to_string()),
        }
    }

    pub(super) fn handle_agent_conversation_snooze(
        &mut self,
        id: String,
        params: crate::api::schema::AgentConversationSnoozeParams,
    ) -> String {
        if !(crate::agent_inbox::MIN_SNOOZE_DURATION_MS
            ..=crate::agent_inbox::MAX_SNOOZE_DURATION_MS)
            .contains(&params.duration_ms)
        {
            return encode_error(
                id,
                "invalid_snooze_duration",
                "duration_ms must be between 60000 and 2592000000",
            );
        }
        let now = now_ms();
        let Some(wake_at_ms) = now.checked_add(params.duration_ms) else {
            return encode_error(
                id,
                "invalid_snooze_duration",
                "snooze deadline exceeds the supported timestamp range",
            );
        };
        // Use one wall-clock sample for duration validation and the absolute deadline.
        self.apply_agent_conversation_mutation(
            id,
            AgentConversationMutationParams {
                conversation_id: params.conversation_id,
                expected_revision: params.expected_revision,
            },
            ConversationAction::Snooze { wake_at_ms },
            now,
        )
    }

    pub(super) fn handle_agent_conversation_mutation(
        &mut self,
        id: String,
        params: AgentConversationMutationParams,
        action: ConversationAction,
    ) -> String {
        self.apply_agent_conversation_mutation(id, params, action, now_ms())
    }

    fn apply_agent_conversation_mutation(
        &mut self,
        id: String,
        params: AgentConversationMutationParams,
        action: ConversationAction,
        now: u64,
    ) -> String {
        let supersedes_manual_resume = matches!(
            action,
            ConversationAction::Settle
                | ConversationAction::SettleAndClose
                | ConversationAction::Snooze { .. }
        );
        match self.agent_inbox.mutate(
            &params.conversation_id,
            params.expected_revision,
            action,
            now,
        ) {
            Ok(record) => {
                if supersedes_manual_resume {
                    self.discard_pending_codex_manual_resumes_for(&record);
                }
                self.sync_agent_inbox_deadline();
                match serde_json::to_value(record) {
                    Ok(conversation) => {
                        encode_success(id, ResponseResult::AgentConversation { conversation })
                    }
                    Err(error) => encode_error(id, "serialization_error", error.to_string()),
                }
            }
            Err(error) => conversation_error(id, error),
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

    fn idle_app() -> (App, crate::layout::PaneId) {
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
        (app, pane_id)
    }

    fn mutation(
        record: &crate::agent_inbox::ConversationRecord,
    ) -> AgentConversationMutationParams {
        AgentConversationMutationParams {
            conversation_id: record.id.clone(),
            expected_revision: record.revision,
        }
    }

    #[test]
    fn manual_resume_reports_failed_persistence_and_completes_after_retry() {
        let (mut app, pane_id) = idle_app();
        let directory = std::env::temp_dir().join(format!(
            "herdr-manual-resume-retry-{}-{}",
            std::process::id(),
            pane_id.raw()
        ));
        let path = directory.join("conversations.json");
        app.agent_inbox =
            crate::agent_inbox::InboxRegistry::load(Some(path.clone()), "manual-resume".into());
        app.sync_agent_conversations();
        let record = app.agent_inbox.records().remove(0);
        let terminal_id = record.attachments[0].terminal_id.clone();
        let session = crate::agent_resume::PersistedAgentSession {
            source: record.source.clone(),
            agent: record.provider.clone(),
            session_ref: record.session_ref.clone().unwrap(),
        };
        app.agent_inbox
            .mutate(
                &record.id,
                record.revision,
                ConversationAction::Settle,
                now_ms(),
            )
            .unwrap();
        std::fs::remove_file(directory.join("conversations.json.bak")).unwrap();
        std::fs::create_dir(directory.join("conversations.json.bak")).unwrap();
        assert!(!app.reopen_conversation_after_manual_resume(&terminal_id, &session));
        assert!(app.agent_inbox_error.is_some());
        assert_eq!(
            app.agent_inbox.get(&record.id).unwrap().disposition,
            Disposition::Settled
        );
        std::fs::remove_dir(directory.join("conversations.json.bak")).unwrap();
        assert!(app.reopen_conversation_after_manual_resume(&terminal_id, &session));
        assert!(app.agent_inbox_error.is_none());
        assert_eq!(
            app.agent_inbox.get(&record.id).unwrap().disposition,
            Disposition::Inbox
        );
        assert!(app.reopen_conversation_after_manual_resume(&terminal_id, &session));
        assert!(!app.reopen_conversation_after_manual_resume("not-attached", &session));
        let restored = crate::agent_inbox::InboxRegistry::load(Some(path), "after-retry".into());
        assert_eq!(
            restored.get(&record.id).unwrap().disposition,
            Disposition::Inbox
        );
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn snooze_api_validates_duration_and_wake_preserves_the_live_attachment() {
        let (mut app, pane_id) = app_with_native_agent();
        let before = app.agent_inbox.records().remove(0);
        for duration_ms in [0, 59_999, crate::agent_inbox::MAX_SNOOZE_DURATION_MS + 1] {
            let response = call(
                &mut app,
                Method::AgentConversationSnooze(
                    crate::api::schema::AgentConversationSnoozeParams {
                        conversation_id: before.id.clone(),
                        expected_revision: before.revision,
                        duration_ms,
                    },
                ),
            );
            assert_eq!(response["error"]["code"], "invalid_snooze_duration");
            assert_eq!(app.agent_inbox.records()[0], before);
        }
        let earliest = now_ms() + 60_000;
        let response = call(
            &mut app,
            Method::AgentConversationSnooze(crate::api::schema::AgentConversationSnoozeParams {
                conversation_id: before.id.clone(),
                expected_revision: before.revision,
                duration_ms: 60_000,
            }),
        );
        assert!(response.get("error").is_none(), "{response}");
        let snoozed = app.agent_inbox.records().remove(0);
        assert!(snoozed.snoozed_until_ms.unwrap() >= earliest);
        assert!(snoozed.snoozed_until_ms.unwrap() <= now_ms() + 60_000);
        assert_eq!(snoozed.attachments, before.attachments);
        assert_eq!(snoozed.execution, ExecutionState::Working);
        assert!(app.state.workspaces[0].pane_state(pane_id).is_some());
        let response = call(&mut app, Method::AgentConversationWake(mutation(&snoozed)));
        assert!(response.get("error").is_none(), "{response}");
        let awake = app.agent_inbox.records().remove(0);
        assert_eq!(awake.snoozed_until_ms, None);
        assert_eq!(awake.section, ConversationSection::Working);
        assert_eq!(awake.attachments, before.attachments);
        let response = call(&mut app, Method::AgentConversationWake(mutation(&snoozed)));
        assert!(response.get("error").is_none(), "{response}");
        assert_eq!(app.agent_inbox.records()[0], awake);
    }

    #[tokio::test]
    async fn snooze_preserves_active_work_and_unsubmitted_terminal_drafts() {
        for (working, input) in [
            (true, &b"submitted task\r"[..]),
            (false, &b"unsent draft"[..]),
        ] {
            let (mut app, pane_id) = if working {
                app_with_native_agent()
            } else {
                idle_app()
            };
            let (runtime, mut accepted) =
                crate::terminal::TerminalRuntime::test_with_channel(80, 24);
            app.state.insert_test_runtime(pane_id, runtime);
            app.lookup_runtime_sender(0, pane_id)
                .unwrap()
                .try_send_bytes(bytes::Bytes::copy_from_slice(input))
                .unwrap();
            assert_eq!(accepted.try_recv().unwrap().as_ref(), input);
            app.sync_agent_conversations();
            let before = app.agent_inbox.records().remove(0);
            assert!(before.queued_human_work);
            let response = call(
                &mut app,
                Method::AgentConversationSnooze(
                    crate::api::schema::AgentConversationSnoozeParams {
                        conversation_id: before.id.clone(),
                        expected_revision: before.revision,
                        duration_ms: 60_000,
                    },
                ),
            );
            assert!(response.get("error").is_none(), "{response}");
            let snoozed = app.agent_inbox.records().remove(0);
            assert!(snoozed.snoozed_until_ms.is_some());
            assert!(snoozed.queued_human_work);
            assert_eq!(snoozed.attachments, before.attachments);
            assert_eq!(snoozed.execution, before.execution);
            assert!(app
                .lookup_runtime_sender(0, pane_id)
                .unwrap()
                .has_pending_human_submission());
            assert!(accepted.try_recv().is_err(), "snooze must not send input");
            let rejected = call(
                &mut app,
                Method::AgentConversationSettle(mutation(&snoozed)),
            );
            assert_eq!(rejected["error"]["code"], "conversation_ineligible");
            assert_eq!(app.agent_inbox.records()[0], snoozed);
        }
    }

    #[test]
    fn snooze_timer_rechecks_wall_clock_and_releases_overdue_deadlines_without_a_client() {
        let (mut app, _) = app_with_native_agent();
        let record = app.agent_inbox.records().remove(0);
        let now = now_ms();
        let snoozed = app
            .agent_inbox
            .mutate(
                &record.id,
                record.revision,
                ConversationAction::Snooze {
                    wake_at_ms: now + 86_400_000,
                },
                now,
            )
            .unwrap();
        let before = Instant::now();
        app.sync_agent_inbox_deadline();
        let wait = app.agent_inbox_deadline.unwrap().duration_since(before);
        assert!(wait <= Duration::from_secs(61));
        assert!(wait >= Duration::from_secs(59));
        // Model a deadline that became overdue after a forward wall-clock jump.
        let now = now_ms();
        app.agent_inbox
            .mutate(
                &record.id,
                snoozed.revision,
                ConversationAction::Snooze {
                    wake_at_ms: now - 1,
                },
                now - 60_001,
            )
            .unwrap();
        app.sync_agent_inbox_deadline();
        assert!(app.tick_agent_conversations(Instant::now() + Duration::from_secs(1)));
        let record = app.agent_inbox.records().remove(0);
        assert_eq!(record.snoozed_until_ms, None);
        assert_eq!(record.section, ConversationSection::Working);
        assert!(app.agent_inbox_deadline.is_none());
    }

    #[test]
    fn verified_prompt_submission_wakes_a_snoozed_conversation() {
        let (mut app, _) = idle_app();
        let record = app.agent_inbox.records().remove(0);
        let response = call(
            &mut app,
            Method::AgentConversationSnooze(crate::api::schema::AgentConversationSnoozeParams {
                conversation_id: record.id.clone(),
                expected_revision: record.revision,
                duration_ms: 60_000,
            }),
        );
        assert!(response.get("error").is_none(), "{response}");
        app.pending_conversation_prompts
            .insert(record.id.clone(), 1);
        app.sync_agent_conversations();
        assert!(app.agent_inbox.records()[0].snoozed_until_ms.is_some());
        app.finish_conversation_prompt(&record.id, true).unwrap();
        let record = app.agent_inbox.records().remove(0);
        assert_eq!(record.snoozed_until_ms, None);
        assert!(record.last_human_submission_at_ms.is_some());
    }

    #[test]
    fn settle_and_close_persists_native_history_even_when_last_space_disappears() {
        let (mut app, pane_id) = idle_app();
        let directory = std::env::temp_dir().join(format!(
            "herdr-settle-close-{}-{}",
            std::process::id(),
            pane_id.raw()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("conversations.json");
        app.agent_inbox =
            crate::agent_inbox::InboxRegistry::load(Some(path.clone()), "close-test".into());
        app.sync_agent_conversations();
        let before = app.agent_inbox.records().pop().unwrap();
        let params = mutation(&before);
        let response = call(
            &mut app,
            Method::AgentConversationSettleAndClose(params.clone()),
        );
        assert!(response.get("error").is_none(), "{response}");
        assert!(app.state.workspaces.is_empty());
        assert!(app.state.terminals.is_empty());
        let restored = crate::agent_inbox::InboxRegistry::load(Some(path), "restart".into());
        let after = restored.get(&before.id).unwrap();
        assert_eq!(after.disposition, Disposition::Settled);
        assert_eq!(after.execution, ExecutionState::Closed);
        assert!(after.attachments.is_empty());
        assert_eq!(after.session_ref, before.session_ref);
        assert_eq!(after.saved_location, before.saved_location);
        assert_eq!(
            call(&mut app, Method::AgentConversationSettleAndClose(params))["result"],
            response["result"]
        );
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn settle_and_close_checkpoint_failure_keeps_terminal_and_registry_unchanged() {
        let (mut app, pane_id) = idle_app();
        let record = app.agent_inbox.records().pop().unwrap();
        let terminal = app.state.workspaces[0].tabs[0].panes[&pane_id]
            .attached_terminal_id
            .clone();
        app.policy.persist_session = true;
        let writer = app.session_writer.clone();
        let _ = std::thread::spawn(move || {
            let _guard = writer.lock().unwrap();
            panic!("injected checkpoint failure");
        })
        .join();
        let response: serde_json::Value = serde_json::from_str(
            &app.handle_agent_conversation_settle_and_close("checkpoint".into(), mutation(&record)),
        )
        .unwrap();
        assert_eq!(response["error"]["code"], "conversation_persistence_failed");
        assert!(app.state.terminals.contains_key(&terminal));
        assert_eq!(
            app.state.workspaces[0].tabs[0].panes[&pane_id].attached_terminal_id,
            terminal
        );
        assert_eq!(app.agent_inbox.get(&record.id).unwrap(), &record);
    }

    #[test]
    fn settle_and_close_storage_failure_leaves_the_original_terminal_attached() {
        let (mut app, pane_id) = idle_app();
        let directory = std::env::temp_dir().join(format!(
            "herdr-settle-fail-{}-{}",
            std::process::id(),
            pane_id.raw()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("conversations.json");
        app.agent_inbox =
            crate::agent_inbox::InboxRegistry::load(Some(path.clone()), "failure-test".into());
        app.sync_agent_conversations();
        let before = app.agent_inbox.records().pop().unwrap();
        std::fs::create_dir(directory.join("conversations.json.bak")).unwrap();
        let response: serde_json::Value = serde_json::from_str(
            &app.handle_agent_conversation_settle_and_close("persist".into(), mutation(&before)),
        )
        .unwrap();
        assert_eq!(response["error"]["code"], "conversation_persistence_failed");
        assert_eq!(
            app.state.workspaces[0].tabs[0].panes[&pane_id]
                .attached_terminal_id
                .as_str(),
            before.saved_location.terminal_id
        );
        assert_eq!(app.agent_inbox.get(&before.id).unwrap(), &before);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn native_restore_startup_blocks_saved_idle_until_actual_agent_acquisition() {
        let (mut app, pane_id) = idle_app();
        let terminal_id = app.state.workspaces[0].tabs[0].panes[&pane_id]
            .attached_terminal_id
            .clone();
        let (runtime, mut accepted) = crate::terminal::TerminalRuntime::test_with_channel(80, 24);
        app.state.insert_test_runtime(pane_id, runtime);
        app.state
            .terminals
            .get_mut(&terminal_id)
            .unwrap()
            .begin_native_resume_launch(Agent::Codex);
        app.lookup_runtime_sender(0, pane_id)
            .unwrap()
            .try_send_system_bytes(bytes::Bytes::from_static(b"codex resume native-test\r"))
            .unwrap();
        assert_eq!(
            accepted.try_recv().unwrap().as_ref(),
            b"codex resume native-test\r"
        );
        assert!(
            !app.lookup_runtime_sender(0, pane_id)
                .unwrap()
                .has_pending_human_submission(),
            "native startup is not a human turn"
        );
        app.sync_agent_conversations();
        let record = app.agent_inbox.records().pop().unwrap();
        let response = call(
            &mut app,
            Method::AgentConversationSettleAndClose(mutation(&record)),
        );
        assert_eq!(response["error"]["code"], "conversation_ineligible");
        assert!(app.state.terminals.contains_key(&terminal_id));
        app.state
            .terminals
            .get_mut(&terminal_id)
            .unwrap()
            .set_detected_state(Some(Agent::Codex), AgentState::Idle);
        app.sync_agent_conversations();
        let record = app.agent_inbox.records().pop().unwrap();
        assert!(!record.queued_human_work);
        let response = call(
            &mut app,
            Method::AgentConversationSettleAndClose(mutation(&record)),
        );
        assert!(
            response.get("error").is_none(),
            "fresh Idle acquisition needs no artificial user turn: {response}"
        );
        assert!(app.state.workspaces.is_empty());
    }

    #[tokio::test]
    async fn settle_and_close_blocks_accepted_raw_input_until_working_turn_completes() {
        let (mut app, pane_id) = idle_app();
        let (runtime, mut accepted) = crate::terminal::TerminalRuntime::test_with_channel(80, 24);
        app.state.insert_test_runtime(pane_id, runtime);
        app.lookup_runtime_sender(0, pane_id)
            .unwrap()
            .try_send_bytes(bytes::Bytes::from_static(b"\x1b[A"))
            .unwrap();
        assert!(
            !app.lookup_runtime_sender(0, pane_id)
                .unwrap()
                .has_pending_human_submission(),
            "navigation must not manufacture queued work"
        );
        assert_eq!(accepted.try_recv().unwrap().as_ref(), b"\x1b[A");
        app.lookup_runtime_sender(0, pane_id)
            .unwrap()
            .try_send_bytes(bytes::Bytes::from_static(b"new task\r"))
            .unwrap();
        assert_eq!(accepted.try_recv().unwrap().as_ref(), b"new task\r");
        app.sync_agent_conversations();
        let record = app.agent_inbox.records().pop().unwrap();
        assert!(record.queued_human_work);
        let response = call(
            &mut app,
            Method::AgentConversationSettleAndClose(mutation(&record)),
        );
        assert_eq!(response["error"]["code"], "conversation_ineligible");
        assert!(
            app.state.workspaces[0].pane_state(pane_id).is_some(),
            "submitted input must prevent shutdown while detection still says Idle"
        );
        for (state, visible_working) in [(AgentState::Working, true), (AgentState::Idle, false)] {
            app.handle_internal_event(crate::events::AppEvent::StateChanged {
                pane_id,
                agent: Some(Agent::Codex),
                state,
                visible_blocker: false,
                visible_working,
                process_exited: false,
                observed_at: Instant::now(),
            });
        }
        app.sync_agent_conversations();
        let record = app.agent_inbox.records().pop().unwrap();
        assert!(!record.queued_human_work);
        let response = call(
            &mut app,
            Method::AgentConversationSettleAndClose(mutation(&record)),
        );
        assert!(response.get("error").is_none(), "{response}");
        assert!(app.state.workspaces.is_empty());
    }

    #[tokio::test]
    async fn agent_process_release_clears_old_input_for_a_replacement_in_the_same_shell() {
        let (mut app, pane_id) = idle_app();
        let terminal_id = app.state.workspaces[0].tabs[0].panes[&pane_id]
            .attached_terminal_id
            .clone();
        let (runtime, mut accepted) = crate::terminal::TerminalRuntime::test_with_channel(80, 24);
        app.state.insert_test_runtime(pane_id, runtime);
        app.lookup_runtime_sender(0, pane_id)
            .unwrap()
            .try_send_bytes(bytes::Bytes::from_static(b"old task\r"))
            .unwrap();
        assert_eq!(accepted.try_recv().unwrap().as_ref(), b"old task\r");
        assert!(app
            .lookup_runtime_sender(0, pane_id)
            .unwrap()
            .has_pending_human_submission());
        app.handle_internal_event(crate::events::AppEvent::StateChanged {
            pane_id,
            agent: Some(Agent::Codex),
            state: AgentState::Unknown,
            visible_blocker: false,
            visible_working: false,
            process_exited: true,
            observed_at: Instant::now(),
        });
        assert_eq!(
            app.state.workspaces[0].tabs[0].panes[&pane_id].attached_terminal_id,
            terminal_id
        );
        assert!(
            !app.lookup_runtime_sender(0, pane_id)
                .unwrap()
                .has_pending_human_submission(),
            "accepted agent exit clears the old process's input even when the shell survives"
        );
        let terminal = app.state.terminals.get_mut(&terminal_id).unwrap();
        terminal.set_detected_state(Some(Agent::Codex), AgentState::Idle);
        terminal.set_persisted_agent_session(crate::agent_resume::PersistedAgentSession {
            source: "herdr:codex".into(),
            agent: "codex".into(),
            session_ref: crate::agent_resume::AgentSessionRef::id("replacement-native").unwrap(),
        });
        app.sync_agent_conversations();
        let replacement = app
            .agent_inbox
            .records()
            .into_iter()
            .find(|record| {
                record
                    .session_ref
                    .as_ref()
                    .is_some_and(|session| session.value == "replacement-native")
            })
            .unwrap();
        assert!(!replacement.queued_human_work);
        let response = call(
            &mut app,
            Method::AgentConversationSettleAndClose(mutation(&replacement)),
        );
        assert!(response.get("error").is_none(), "{response}");
        assert!(app.state.workspaces.is_empty());
    }

    #[test]
    fn settle_and_close_rechecks_working_blocked_unknown_and_queued_work_before_close() {
        for state in [
            AgentState::Working,
            AgentState::Blocked,
            AgentState::Unknown,
            AgentState::Idle,
        ] {
            let (mut app, pane_id) = idle_app();
            let record = app.agent_inbox.records().pop().unwrap();
            let terminal_id = app.state.workspaces[0].tabs[0].panes[&pane_id]
                .attached_terminal_id
                .clone();
            app.state
                .terminals
                .get_mut(&terminal_id)
                .unwrap()
                .set_detected_state(Some(Agent::Codex), state);
            if state == AgentState::Idle {
                app.pending_conversation_prompts
                    .insert(record.id.clone(), 1);
            }
            let response: serde_json::Value = serde_json::from_str(
                &app.handle_agent_conversation_settle_and_close("race".into(), mutation(&record)),
            )
            .unwrap();
            assert_eq!(
                response["error"]["code"], "conversation_ineligible",
                "{state:?}: {response}"
            );
            assert!(app.state.terminals.contains_key(&terminal_id));
            assert_eq!(app.agent_inbox.get(&record.id).unwrap(), &record);
        }
    }

    #[tokio::test]
    async fn settle_and_close_rejects_replacement_generation_and_duplicate_attachments() {
        let (mut app, pane_id) = idle_app();
        let record = app.agent_inbox.records().pop().unwrap();
        let (old_runtime, _old_receiver) =
            crate::terminal::TerminalRuntime::test_with_channel(80, 24);
        old_runtime
            .try_send_bytes(bytes::Bytes::from_static(b"old generation\r"))
            .unwrap();
        assert!(old_runtime.has_pending_human_submission());
        app.state.insert_test_runtime(pane_id, old_runtime);
        let replacement = crate::terminal::TerminalId::alloc();
        app.state.terminals.insert(
            replacement.clone(),
            crate::terminal::TerminalState::new(replacement.clone(), "/".into()),
        );
        app.state.workspaces[0]
            .pane_state_mut(pane_id)
            .unwrap()
            .attached_terminal_id = replacement.clone();
        let (new_runtime, _new_receiver) =
            crate::terminal::TerminalRuntime::test_with_channel(80, 24);
        assert!(
            !new_runtime.has_pending_human_submission(),
            "a replacement terminal generation must not inherit the old runtime's input latch"
        );
        app.state.insert_test_runtime(pane_id, new_runtime);
        let response: serde_json::Value = serde_json::from_str(
            &app.handle_agent_conversation_settle_and_close("generation".into(), mutation(&record)),
        )
        .unwrap();
        assert_eq!(
            response["error"]["code"],
            "conversation_attachment_unavailable"
        );
        assert!(app.state.terminals.contains_key(&replacement));
        assert_eq!(app.agent_inbox.get(&record.id).unwrap(), &record);

        let (mut app, _) = idle_app();
        app.state
            .workspaces
            .push(crate::workspace::Workspace::test_new("duplicate"));
        app.state.ensure_test_terminals();
        let pane = app.state.workspaces[1].tabs[0].root_pane;
        let terminal = app.state.workspaces[1].tabs[0].panes[&pane]
            .attached_terminal_id
            .clone();
        let state = app.state.terminals.get_mut(&terminal).unwrap();
        state.set_detected_state(Some(Agent::Codex), AgentState::Idle);
        state.set_persisted_agent_session(crate::agent_resume::PersistedAgentSession {
            source: "herdr:codex".into(),
            agent: "codex".into(),
            session_ref: crate::agent_resume::AgentSessionRef::id("native-test").unwrap(),
        });
        let stale_projection = app.agent_inbox.records().pop().unwrap();
        let response: serde_json::Value =
            serde_json::from_str(&app.handle_agent_conversation_settle_and_close(
                "duplicate-recheck".into(),
                mutation(&stale_projection),
            ))
            .unwrap();
        assert_eq!(response["error"]["code"], "conversation_ineligible");
        assert_eq!(
            app.agent_inbox.get(&stale_projection.id).unwrap().revision,
            stale_projection.revision
        );
        app.sync_agent_conversations();
        let record = app.agent_inbox.records().pop().unwrap();
        let response = call(
            &mut app,
            Method::AgentConversationSettleAndClose(mutation(&record)),
        );
        assert_eq!(response["error"]["code"], "conversation_ineligible");
        assert_eq!(app.state.workspaces.len(), 2);
        assert_eq!(app.state.terminals.len(), 2);
    }

    #[test]
    fn settle_and_close_closes_old_nonclosing_settle_but_retry_never_closes_new_attachment() {
        let (mut app, _) = idle_app();
        let original = app.agent_inbox.records().pop().unwrap();
        let response = call(
            &mut app,
            Method::AgentConversationSettle(mutation(&original)),
        );
        assert!(response.get("error").is_none());
        assert_eq!(
            app.state.workspaces.len(),
            1,
            "published settle remains nonclosing"
        );
        let settled = app.agent_inbox.records().pop().unwrap();
        let params = mutation(&settled);
        assert!(call(
            &mut app,
            Method::AgentConversationSettleAndClose(params.clone())
        )
        .get("error")
        .is_none());
        app.state
            .workspaces
            .push(crate::workspace::Workspace::test_new("new attachment"));
        app.state.ensure_test_terminals();
        let pane = app.state.workspaces[0].tabs[0].root_pane;
        let terminal_id = app.state.workspaces[0].tabs[0].panes[&pane]
            .attached_terminal_id
            .clone();
        let terminal = app.state.terminals.get_mut(&terminal_id).unwrap();
        terminal.set_detected_state(Some(Agent::Codex), AgentState::Idle);
        terminal.set_persisted_agent_session(crate::agent_resume::PersistedAgentSession {
            source: "herdr:codex".into(),
            agent: "codex".into(),
            session_ref: original.session_ref.unwrap(),
        });
        let response = call(&mut app, Method::AgentConversationSettleAndClose(params));
        assert_eq!(response["error"]["code"], "conversation_revision_conflict");
        assert!(app.state.terminals.contains_key(&terminal_id));
    }

    #[test]
    fn restore_reuses_and_focuses_the_unique_native_attachment() {
        let (mut app, pane_id) = idle_app();
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
            .workspaces
            .push(crate::workspace::Workspace::test_new("other"));
        app.state.ensure_test_terminals();
        app.state.active = Some(1);
        let response = call(
            &mut app,
            Method::AgentConversationRestore(crate::api::schema::AgentConversationRestoreParams {
                conversation_id: record.id.clone(),
                expected_revision: record.revision,
                location: None,
                cwd: None,
            }),
        );
        assert_eq!(
            response["result"]["result"]["status"], "uncertain",
            "{response}"
        );
        assert_eq!(
            response["result"]["result"]["pane_id"],
            app.public_pane_id(0, pane_id).unwrap()
        );
        assert_eq!(app.state.active, Some(0));
        assert_eq!(app.state.terminals.len(), 2);
        assert_eq!(
            app.agent_inbox.get(&record.id).unwrap().session_ref,
            record.session_ref
        );
    }

    #[test]
    fn live_conversation_titles_use_human_pane_labels_and_display_ordinals() {
        let (mut app, pane_id) = idle_app();
        let terminal_id = app.state.workspaces[0].tabs[0].panes[&pane_id]
            .attached_terminal_id
            .clone();
        app.state
            .terminals
            .get_mut(&terminal_id)
            .unwrap()
            .set_manual_label("Review inbox design".into());
        app.state.workspaces[0].tabs[0].set_custom_name("Implementation".into());
        app.sync_agent_conversations();
        let record = app.agent_inbox.records().pop().unwrap();
        assert_eq!(record.title, "Review inbox design");
        assert_eq!(
            record.saved_location.pane_label.as_deref(),
            Some("Review inbox design")
        );
        assert_eq!(record.saved_location.pane_number, Some(1));
        app.state
            .terminals
            .get_mut(&terminal_id)
            .unwrap()
            .clear_manual_label();
        app.sync_agent_conversations();
        assert_eq!(app.agent_inbox.records()[0].title, "Implementation");
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
