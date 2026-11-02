use super::*;

const MAX_QUEUED_NOTIFICATIONS: usize = 8;
const COMPLETION_EVIDENCE_GRACE: std::time::Duration = std::time::Duration::from_secs(1);
const INBOX_COMPLETION_GRACE: std::time::Duration = std::time::Duration::from_secs(2);
pub(super) const COMPLETION_RECHECK_INTERVAL: std::time::Duration =
    std::time::Duration::from_millis(50);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NotificationValidation {
    Current,
    AwaitingSnapshot,
    Stale,
}

pub(super) struct InboxCompletionEvidence {
    boot_id: Option<String>,
    sequence: Option<u64>,
    projection_revision: Option<u64>,
    attention_after_ms: Option<u64>,
}

fn notification_duration(kind: SemanticNotificationKind) -> std::time::Duration {
    std::time::Duration::from_secs(match kind {
        SemanticNotificationKind::NeedsAttention => 8,
        SemanticNotificationKind::Finished => 5,
        SemanticNotificationKind::UpdateInstalled => 3,
        SemanticNotificationKind::Custom => 5,
    })
}

impl ClientShellState {
    pub(super) fn retire_endpoint_notifications(&mut self, endpoint_id: &ClientEndpointId) {
        self.pending_notifications
            .retain(|pending| &pending.endpoint_id != endpoint_id);
        self.queued_notifications
            .retain(|queued| &queued.endpoint_id != endpoint_id);
        if self
            .visible_notification
            .as_ref()
            .is_some_and(|visible| &visible.endpoint_id == endpoint_id)
        {
            self.visible_notification = None;
            self.promote_queued_notification(std::time::Instant::now());
        }
    }

    fn queue_visible_notification(
        &mut self,
        mut notification: ClientVisibleNotification,
        now: std::time::Instant,
    ) {
        if self.visible_notification.is_none() {
            notification.deadline = now + notification_duration(notification.event.kind);
            self.visible_notification = Some(notification);
            return;
        }
        if self.queued_notifications.len() == MAX_QUEUED_NOTIFICATIONS {
            self.queued_notifications.pop_front();
        }
        self.queued_notifications.push_back(notification);
    }

    fn promote_queued_notification(&mut self, now: std::time::Instant) -> bool {
        let Some(mut notification) = self.queued_notifications.pop_front() else {
            return false;
        };
        notification.deadline = now + notification_duration(notification.event.kind);
        self.visible_notification = Some(notification);
        true
    }

    pub(super) fn focus_visible_notification(&mut self, outcome: &mut ClientShellInput) {
        let Some(notification) = self.visible_notification.as_ref() else {
            return;
        };
        if notification.event.pane_id.is_some()
            && !self.endpoint_is_online(&notification.endpoint_id)
        {
            let label = self.endpoint_label(&notification.endpoint_id).to_owned();
            self.receive_endpoint_unavailable(format!("{label} is unavailable"));
            outcome.repaint = true;
            return;
        }
        let notification = self
            .visible_notification
            .take()
            .expect("checked visible notification");
        self.promote_queued_notification(std::time::Instant::now());
        outcome.repaint = true;
        let Some(pane_id) = notification.event.pane_id else {
            return;
        };
        if notification.endpoint_id == self.active_endpoint_id {
            self.push_endpoint_method(
                crate::api::schema::Method::PaneFocus(crate::api::schema::PaneTarget { pane_id }),
                outcome,
            );
        } else if self.endpoint_is_online(&notification.endpoint_id) {
            outcome.actions.push(ClientShellAction::ActivateEndpoint {
                endpoint_id: notification.endpoint_id,
                target: Some(ClientEndpointFocusTarget::Pane(pane_id)),
            });
        }
    }

    pub(crate) fn receive_notification(
        &mut self,
        endpoint_id: &ClientEndpointId,
        event: SemanticNotification,
        now: std::time::Instant,
    ) -> (Vec<ClientShellNotificationEffect>, bool) {
        let delay = if event.kind == SemanticNotificationKind::Custom {
            0
        } else {
            self.config.toast_delay_seconds
        };
        let inbox_completion = (event.kind == SemanticNotificationKind::Finished)
            .then(|| self.inbox_completion_evidence(endpoint_id, &event))
            .flatten();
        let mut deadline = now
            .checked_add(std::time::Duration::from_secs(delay))
            .unwrap_or(now);
        if inbox_completion.is_some() {
            deadline = deadline.max(now.checked_add(INBOX_COMPLETION_GRACE).unwrap_or(now));
        }
        let expires_at = if inbox_completion.is_some() {
            deadline
                .checked_add(COMPLETION_EVIDENCE_GRACE)
                .unwrap_or(deadline)
        } else {
            now.checked_add(COMPLETION_EVIDENCE_GRACE).unwrap_or(now)
        };
        let cleared_visible = event.pane_id.as_deref().is_some_and(|pane_id| {
            self.visible_notification.as_ref().is_some_and(|visible| {
                visible.endpoint_id == *endpoint_id
                    && visible.event.pane_id.as_deref() == Some(pane_id)
            })
        });
        if let Some(pane_id) = event.pane_id.as_deref() {
            self.pending_notifications.retain(|pending| {
                pending.endpoint_id != *endpoint_id
                    || pending.event.pane_id.as_deref() != Some(pane_id)
            });
            self.queued_notifications.retain(|queued| {
                queued.endpoint_id != *endpoint_id
                    || queued.event.pane_id.as_deref() != Some(pane_id)
            });
            if cleared_visible {
                self.visible_notification = None;
                self.promote_queued_notification(now);
            }
        }
        // Completion evidence is advisory. A Finished effect is valid only while the
        // client-projected pane remains Done, even when delivery is immediate.
        let validate_state = delay > 0 || event.kind == SemanticNotificationKind::Finished;
        self.pending_notifications.push(ClientPendingNotification {
            endpoint_id: endpoint_id.clone(),
            event,
            deadline,
            expires_at,
            validate_state,
            inbox_completion,
        });
        let (effects, repaint) = self.tick_notifications(now);
        (effects, repaint || cleared_visible)
    }

    pub(crate) fn tick_notifications(
        &mut self,
        now: std::time::Instant,
    ) -> (Vec<ClientShellNotificationEffect>, bool) {
        let mut repaint = false;
        if self
            .visible_notification
            .as_ref()
            .is_some_and(|visible| now >= visible.deadline)
        {
            self.visible_notification = None;
            self.promote_queued_notification(now);
            repaint = true;
        }
        if self
            .visible_endpoint_notice
            .as_ref()
            .is_some_and(|visible| now >= visible.deadline)
        {
            self.visible_endpoint_notice = None;
            repaint = true;
        }

        let pending = std::mem::take(&mut self.pending_notifications);
        let mut effects = Vec::new();
        for mut pending in pending {
            if pending.deadline > now {
                self.pending_notifications.push(pending);
                continue;
            }
            if pending.validate_state {
                let validation = if let Some(evidence) = pending.inbox_completion.as_mut() {
                    self.inbox_completion_validation(&pending.endpoint_id, &pending.event, evidence)
                } else {
                    self.notification_validation(&pending.endpoint_id, &pending.event)
                };
                match validation {
                    NotificationValidation::Current => {}
                    NotificationValidation::AwaitingSnapshot if now < pending.expires_at => {
                        let mut pending = pending;
                        pending.deadline = now
                            .checked_add(COMPLETION_RECHECK_INTERVAL)
                            .unwrap_or(now)
                            .min(pending.expires_at);
                        self.pending_notifications.push(pending);
                        continue;
                    }
                    NotificationValidation::AwaitingSnapshot | NotificationValidation::Stale => {
                        continue;
                    }
                }
            }
            let target_active =
                self.notification_target_is_active(&pending.endpoint_id, &pending.event);
            let suppress_external = target_active && self.outer_focused != Some(false);
            if let Some(sound) = pending.event.sound {
                let suppress_sound =
                    pending.event.kind == SemanticNotificationKind::Finished && suppress_external;
                if !suppress_sound {
                    effects.push(ClientShellNotificationEffect::Sound {
                        sound: match sound {
                            SemanticNotificationSound::Done => crate::sound::Sound::Done,
                            SemanticNotificationSound::Request => crate::sound::Sound::Request,
                        },
                        agent: pending.event.agent.clone(),
                    });
                }
            }

            match self.config.toast_delivery {
                crate::config::ToastDelivery::Off => {}
                crate::config::ToastDelivery::Herdr if !target_active => {
                    self.queue_visible_notification(
                        ClientVisibleNotification {
                            endpoint_id: pending.endpoint_id,
                            event: pending.event,
                            deadline: now,
                        },
                        now,
                    );
                    repaint = true;
                }
                crate::config::ToastDelivery::Herdr => {}
                crate::config::ToastDelivery::Terminal if !suppress_external => {
                    effects.push(ClientShellNotificationEffect::Terminal {
                        title: pending.event.title,
                        body: pending.event.body,
                    });
                }
                crate::config::ToastDelivery::System if !suppress_external => {
                    #[cfg(windows)]
                    let target = pending.event.pane_id.as_ref().and_then(|pane_id| {
                        self.endpoint_boot_id(&pending.endpoint_id).map(|boot_id| {
                            ClientSystemNotificationTarget {
                                endpoint_id: pending.endpoint_id.clone(),
                                boot_id: boot_id.to_owned(),
                                pane_id: pane_id.clone(),
                            }
                        })
                    });
                    effects.push(ClientShellNotificationEffect::System {
                        title: pending.event.title,
                        body: pending.event.body,
                        #[cfg(windows)]
                        target,
                    });
                }
                crate::config::ToastDelivery::Terminal | crate::config::ToastDelivery::System => {}
            }
        }
        (effects, repaint)
    }

    #[cfg(windows)]
    pub(crate) fn notification_target_is_current(
        &self,
        endpoint_id: &ClientEndpointId,
        target: &ClientEndpointFocusTarget,
    ) -> bool {
        let ClientEndpointFocusTarget::Notification { pane_id, boot_id } = target else {
            return true;
        };
        self.endpoint_is_online(endpoint_id)
            && self
                .endpoints
                .iter()
                .find(|endpoint| &endpoint.endpoint_id == endpoint_id)
                .and_then(|endpoint| endpoint.snapshot.as_deref())
                .is_some_and(|snapshot| {
                    snapshot.boot_id == *boot_id
                        && snapshot.panes.iter().any(|pane| pane.pane_id == *pane_id)
                })
    }

    #[cfg(windows)]
    pub(crate) fn activate_system_notification(
        &mut self,
        target: ClientSystemNotificationTarget,
    ) -> ClientShellInput {
        let focus = ClientEndpointFocusTarget::Notification {
            pane_id: target.pane_id,
            boot_id: target.boot_id,
        };
        let mut outcome = ClientShellInput::default();
        if self.notification_target_is_current(&target.endpoint_id, &focus) {
            outcome.actions.push(ClientShellAction::ActivateEndpoint {
                endpoint_id: target.endpoint_id,
                target: Some(focus),
            });
        }
        outcome
    }

    fn notification_target_is_active(
        &self,
        endpoint_id: &ClientEndpointId,
        event: &SemanticNotification,
    ) -> bool {
        if endpoint_id != &self.active_endpoint_id {
            return false;
        }
        let Some(snapshot) = self
            .endpoints
            .iter()
            .find(|endpoint| &endpoint.endpoint_id == endpoint_id)
            .and_then(|endpoint| endpoint.snapshot.as_deref())
        else {
            return false;
        };
        if let Some(tab_id) = event.tab_id.as_deref() {
            return snapshot.focused_tab_id.as_deref() == Some(tab_id);
        }
        event.workspace_id.as_deref().is_some_and(|workspace_id| {
            snapshot.focused_workspace_id.as_deref() == Some(workspace_id)
        })
    }

    fn notification_validation(
        &self,
        endpoint_id: &ClientEndpointId,
        event: &SemanticNotification,
    ) -> NotificationValidation {
        let Some(pane_id) = event.pane_id.as_deref() else {
            // Finished notifications carry no independently trustworthy completion state. Without
            // a projected pane to verify as Done, do not emit completion chrome or sound.
            return if event.kind == SemanticNotificationKind::Finished {
                NotificationValidation::Stale
            } else {
                NotificationValidation::Current
            };
        };
        let Some(agent) = self
            .endpoints
            .iter()
            .find(|endpoint| &endpoint.endpoint_id == endpoint_id)
            .and_then(|endpoint| endpoint.snapshot.as_deref())
            .and_then(|snapshot| {
                snapshot
                    .agents
                    .iter()
                    .find(|agent| agent.pane_id == pane_id)
            })
        else {
            return NotificationValidation::AwaitingSnapshot;
        };
        match event.kind {
            SemanticNotificationKind::NeedsAttention
                if agent.agent_status == crate::api::schema::AgentStatus::Blocked =>
            {
                NotificationValidation::Current
            }
            SemanticNotificationKind::Finished
                if agent.agent_status == crate::api::schema::AgentStatus::Done =>
            {
                NotificationValidation::Current
            }
            SemanticNotificationKind::Finished
                if agent.agent_status == crate::api::schema::AgentStatus::Working =>
            {
                NotificationValidation::AwaitingSnapshot
            }
            SemanticNotificationKind::UpdateInstalled | SemanticNotificationKind::Custom => {
                NotificationValidation::Current
            }
            SemanticNotificationKind::NeedsAttention | SemanticNotificationKind::Finished => {
                NotificationValidation::Stale
            }
        }
    }

    fn inbox_completion_evidence(
        &self,
        endpoint_id: &ClientEndpointId,
        event: &SemanticNotification,
    ) -> Option<InboxCompletionEvidence> {
        let endpoint = self
            .endpoints
            .iter()
            .find(|endpoint| &endpoint.endpoint_id == endpoint_id)?;
        if !self.agent_inbox || !endpoint.conversations_supported {
            return None;
        }
        let snapshot = endpoint.snapshot.as_deref();
        let agent = snapshot.and_then(|snapshot| {
            snapshot
                .agents
                .iter()
                .find(|agent| Some(agent.pane_id.as_str()) == event.pane_id.as_deref())
        });
        let cache = endpoint
            .conversations
            .as_ref()
            .filter(|_| super::agent_inbox::cache_current(endpoint));
        let record = cache.and_then(|cache| {
            cache.projection.conversations.iter().find(|record| {
                record.attachments.iter().any(|attachment| {
                    Some(attachment.public_pane_id.as_str()) == event.pane_id.as_deref()
                })
            })
        });
        Some(InboxCompletionEvidence {
            boot_id: snapshot.map(|snapshot| snapshot.boot_id.clone()),
            sequence: agent
                .filter(|agent| agent.agent_status == crate::api::schema::AgentStatus::Done)
                .map(|agent| agent.state_change_seq),
            projection_revision: cache.map(|cache| cache.projection.revision),
            attention_after_ms: record
                .filter(|record| record.section == crate::agent_inbox::ConversationSection::Working)
                .map(|record| record.attention_returned_at_ms),
        })
    }

    fn inbox_completion_validation(
        &self,
        endpoint_id: &ClientEndpointId,
        event: &SemanticNotification,
        evidence: &mut InboxCompletionEvidence,
    ) -> NotificationValidation {
        use crate::agent_inbox::{ConversationSection, Disposition, ExecutionState, ResumeState};

        let Some(endpoint) = self
            .endpoints
            .iter()
            .find(|endpoint| &endpoint.endpoint_id == endpoint_id)
        else {
            return NotificationValidation::Stale;
        };
        if !endpoint.conversations_supported || endpoint.status != ClientEndpointStatus::Online {
            return NotificationValidation::Stale;
        }
        let Some(snapshot) = endpoint.snapshot.as_deref() else {
            return NotificationValidation::AwaitingSnapshot;
        };
        if evidence
            .boot_id
            .as_deref()
            .is_some_and(|boot| boot != snapshot.boot_id)
        {
            return NotificationValidation::Stale;
        }
        let Some(agent) = snapshot
            .agents
            .iter()
            .find(|agent| Some(agent.pane_id.as_str()) == event.pane_id.as_deref())
        else {
            return NotificationValidation::AwaitingSnapshot;
        };
        if evidence
            .sequence
            .is_some_and(|sequence| sequence != agent.state_change_seq)
        {
            return NotificationValidation::Stale;
        }
        if agent.agent_status != crate::api::schema::AgentStatus::Done {
            return if agent.agent_status == crate::api::schema::AgentStatus::Working {
                NotificationValidation::AwaitingSnapshot
            } else {
                NotificationValidation::Stale
            };
        }
        evidence.sequence = Some(agent.state_change_seq);
        if !super::agent_inbox::cache_current(endpoint) {
            return NotificationValidation::AwaitingSnapshot;
        }
        let cache = endpoint
            .conversations
            .as_ref()
            .expect("current conversation cache");
        if cache.projection.recovery_error.is_some() {
            return NotificationValidation::Stale;
        }
        let Some(record) = cache.projection.conversations.iter().find(|record| {
            record
                .attachments
                .iter()
                .any(|attachment| attachment.public_pane_id == agent.pane_id)
        }) else {
            return NotificationValidation::AwaitingSnapshot;
        };
        if record.disposition == Disposition::Settled
            || record.resume_state != ResumeState::None
            || record.queued_human_work
            || record.blocking_request
            || record.failure
        {
            return NotificationValidation::Stale;
        }
        if record.execution == ExecutionState::Working {
            return if evidence
                .projection_revision
                .is_some_and(|revision| revision == cache.projection.revision)
            {
                NotificationValidation::AwaitingSnapshot
            } else {
                NotificationValidation::Stale
            };
        }
        if record.execution != ExecutionState::Idle {
            return NotificationValidation::Stale;
        }
        if record.section == ConversationSection::Working
            || evidence
                .attention_after_ms
                .is_some_and(|previous| record.attention_returned_at_ms <= previous)
        {
            return NotificationValidation::AwaitingSnapshot;
        }
        if record.section != ConversationSection::Inbox {
            return NotificationValidation::Stale;
        }
        NotificationValidation::Current
    }
}

#[cfg(test)]
mod inbox_tests {
    use super::*;
    use crate::agent_inbox::{
        ConversationRecord, ConversationSection, Disposition, ExecutionState, ResumeState,
    };
    use crate::client::endpoint::ClientAgentConversationsProjection;
    use crate::protocol::ClientShellAgent;

    fn record() -> ConversationRecord {
        let attachment = serde_json::json!({
            "pane_id": 1, "public_pane_id": "pane_1", "workspace_id": "ws_1", "tab_id": "tab_1",
            "workspace_location_id": "workspace", "tab_location_id": "tab", "cwd": "/repo"
        });
        serde_json::from_value(serde_json::json!({
            "id": "conversation", "provider": "codex", "source": "test", "session_ref": null,
            "title": "task", "saved_location": attachment, "disposition": "inbox", "created_at_ms": 1,
            "attention_returned_at_ms": 1, "last_human_submission_at_ms": 1, "settled_at_ms": null,
            "revision": 1, "attachments": [attachment], "section": "working", "execution": "idle",
            "queued_human_work": false, "blocking_request": false, "failure": false,
            "capability_limits": [], "resume_state": "none"
        })).unwrap()
    }

    fn project(state: &mut ClientShellState, record: ConversationRecord, revision: u64) {
        let boot_id = state.endpoints[0]
            .snapshot
            .as_ref()
            .unwrap()
            .boot_id
            .clone();
        assert!(state.set_endpoint_agent_conversations(
            &ClientEndpointId::Local,
            None,
            ClientAgentConversationsProjection {
                boot_id,
                revision,
                conversations: vec![record],
                total_count: 1,
                next_offset: None,
                recovery_error: None,
            }
        ));
    }

    fn state() -> ClientShellState {
        let mut config = ClientShellConfig::from_config(&Config::default());
        config.toast_delivery = crate::config::ToastDelivery::Terminal;
        config.toast_delay_seconds = 0;
        let mut state = ClientShellState::new(config);
        state.agent_inbox = true;
        state.set_endpoint_agent_conversations_supported(&ClientEndpointId::Local, true);
        let mut snapshot = super::super::tests::snapshot();
        snapshot.agents.push(ClientShellAgent {
            pane_id: "pane_1".into(),
            workspace_id: "ws_1".into(),
            tab_id: "tab_1".into(),
            name: None,
            display_agent: None,
            agent: Some("codex".into()),
            title: None,
            terminal_title: None,
            terminal_title_stripped: None,
            agent_status: crate::api::schema::AgentStatus::Working,
            state_change_seq: 1,
            state_labels: Vec::new(),
            tokens: Vec::new(),
            focused: false,
        });
        state.set_snapshot(Box::new(snapshot.clone()));
        snapshot.revision += 1;
        snapshot.agents[0].agent_status = crate::api::schema::AgentStatus::Idle;
        snapshot.agents[0].state_change_seq = 2;
        state.set_snapshot(Box::new(snapshot));
        project(&mut state, record(), 10);
        state
    }

    fn event() -> SemanticNotification {
        SemanticNotification {
            kind: SemanticNotificationKind::Finished,
            title: "agent finished".into(),
            body: None,
            sound: Some(SemanticNotificationSound::Done),
            agent: Some("codex".into()),
            workspace_id: Some("ws_1".into()),
            tab_id: Some("background".into()),
            pane_id: Some("pane_1".into()),
            position: None,
        }
    }

    fn returned_record() -> ConversationRecord {
        let mut record = record();
        record.section = ConversationSection::Inbox;
        record.attention_returned_at_ms = 2_001;
        record
    }

    #[test]
    fn inbox_finished_waits_for_grace_and_fresh_attention_without_restore_replay() {
        let mut state = state();
        let now = std::time::Instant::now();
        assert!(state.tick_notifications(now).0.is_empty());
        assert!(state
            .receive_notification(&ClientEndpointId::Local, event(), now)
            .0
            .is_empty());
        assert!(state
            .tick_notifications(now + INBOX_COMPLETION_GRACE)
            .0
            .is_empty());
        assert_eq!(state.pending_notifications.len(), 1);
        project(&mut state, returned_record(), 11);
        let delivered = now + INBOX_COMPLETION_GRACE + COMPLETION_RECHECK_INTERVAL;
        assert_eq!(state.tick_notifications(delivered).0.len(), 2);
        assert!(state
            .tick_notifications(delivered + COMPLETION_RECHECK_INTERVAL)
            .0
            .is_empty());
    }

    #[test]
    fn inbox_finished_handles_event_before_companion_and_after_companion() {
        for event_first in [true, false] {
            let mut state = state();
            let now = std::time::Instant::now();
            if event_first {
                state.endpoints[0].conversations = None;
            } else {
                project(&mut state, returned_record(), 11);
            }
            assert!(state
                .receive_notification(&ClientEndpointId::Local, event(), now)
                .0
                .is_empty());
            if event_first {
                assert!(state
                    .tick_notifications(now + INBOX_COMPLETION_GRACE)
                    .0
                    .is_empty());
                project(&mut state, returned_record(), 11);
            }
            assert_eq!(
                state
                    .tick_notifications(now + INBOX_COMPLETION_GRACE + COMPLETION_RECHECK_INTERVAL)
                    .0
                    .len(),
                2
            );
        }
    }

    #[test]
    fn inbox_finished_cancels_continuation_settlement_and_resume() {
        for reason in ["continuation", "settled", "resume"] {
            let mut state = state();
            let now = std::time::Instant::now();
            assert!(state
                .receive_notification(&ClientEndpointId::Local, event(), now)
                .0
                .is_empty());
            let mut record = returned_record();
            match reason {
                "continuation" => {
                    record.execution = ExecutionState::Working;
                    record.section = ConversationSection::Working;
                    let mut snapshot = state.endpoints[0]
                        .snapshot
                        .as_ref()
                        .unwrap()
                        .as_ref()
                        .clone();
                    snapshot.revision += 1;
                    snapshot.agents[0].agent_status = crate::api::schema::AgentStatus::Working;
                    snapshot.agents[0].state_change_seq += 1;
                    state.set_snapshot(Box::new(snapshot));
                }
                "settled" => {
                    record.disposition = Disposition::Settled;
                    record.section = ConversationSection::Settled;
                }
                _ => record.resume_state = ResumeState::Pending,
            }
            project(&mut state, record, 11);
            assert!(
                state
                    .tick_notifications(now + INBOX_COMPLETION_GRACE + COMPLETION_RECHECK_INTERVAL)
                    .0
                    .is_empty(),
                "{reason}"
            );
            assert!(state.pending_notifications.is_empty(), "{reason}");
            project(&mut state, returned_record(), 12);
            assert!(state
                .tick_notifications(now + std::time::Duration::from_secs(4))
                .0
                .is_empty());
        }
    }
}
