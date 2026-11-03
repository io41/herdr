use std::time::{Duration, Instant};

use crate::agent_resume::PersistedAgentSession;
use crate::api::schema::PaneReportAgentSessionEndParams;
use crate::detect::Agent;
use crate::events::AppEvent;
use crate::layout::PaneId;
use crate::terminal::TerminalId;

use super::App;

const CODEX_END_RELEASE_WINDOW: Duration = Duration::from_secs(10);

#[derive(Debug)]
pub(crate) struct PendingCodexSessionEnd {
    pub(crate) conversation_id: String,
    terminal_id: TerminalId,
    session: PersistedAgentSession,
    observed_at: Instant,
    last_human_submission_at_ms: Option<u64>,
}

#[derive(Debug)]
pub(crate) struct PendingCodexManualResume {
    session: PersistedAgentSession,
    user_owned: bool,
    confirmed: bool,
    observed_at: Instant,
}

impl App {
    pub(super) fn observe_codex_session_end(
        &mut self,
        ws_idx: usize,
        pane_id: PaneId,
        params: &PaneReportAgentSessionEndParams,
    ) -> Result<(), &'static str> {
        if params.source != "herdr:codex" || params.agent != "codex" || params.reason != "other" {
            return Err("session end requires an official Codex shutdown report");
        }
        let terminal_id = self.state.workspaces[ws_idx]
            .pane_state(pane_id)
            .ok_or("pane is unavailable")?
            .attached_terminal_id
            .clone();
        let now_ms = super::super::inbox::now_ms();
        let acquired_at_ms = self
            .codex_process_acquisitions
            .get(&terminal_id)
            .ok_or("session end requires a confirmed Codex process acquisition")?;
        if params.reported_at_unix_ms <= *acquired_at_ms
            || params.reported_at_unix_ms > now_ms
            || now_ms.saturating_sub(params.reported_at_unix_ms)
                > CODEX_END_RELEASE_WINDOW.as_millis() as u64
        {
            return Err("session end must be emitted after the current process was acquired");
        }
        let terminal = self
            .state
            .terminals
            .get(&terminal_id)
            .ok_or("terminal is unavailable")?;
        if crate::agent_resume::AgentSessionRef::path(params.transcript_path.clone()).is_none() {
            return Err("session end requires an absolute transcript path");
        }
        let session = terminal
            .persisted_agent_session
            .as_ref()
            .filter(|session| {
                session.source == params.source
                    && session.agent == params.agent
                    && session.session_ref.kind == crate::agent_resume::AgentSessionRefKind::Id
                    && session.session_ref.value == params.agent_session_id
            })
            .cloned()
            .ok_or("session end must confirm the current saved native session")?;
        if terminal.detected_agent != Some(Agent::Codex)
            || !terminal.hook_report_is_newer(&params.source, Some(params.seq))
        {
            return Err("session end is stale or its Codex process is unavailable");
        }
        let records = self.agent_inbox.records();
        let record = records
            .iter()
            .find(|record| {
                record.provider == session.agent
                    && record.source == session.source
                    && record.session_ref.as_ref() == Some(&session.session_ref)
                    && record.attachments.len() == 1
                    && record.attachments[0].terminal_id == terminal_id.to_string()
            })
            .ok_or("session end requires one exact live conversation attachment")?;
        if !self
            .state
            .terminals
            .get_mut(&terminal_id)
            .is_some_and(|terminal| terminal.accept_codex_session_end(&session, params.seq))
        {
            return Err("session end was superseded");
        }
        self.pending_codex_manual_resumes.remove(&terminal_id);
        self.pending_codex_session_ends.insert(
            terminal_id.clone(),
            PendingCodexSessionEnd {
                conversation_id: record.id.clone(),
                terminal_id,
                session,
                observed_at: Instant::now(),
                last_human_submission_at_ms: record.last_human_submission_at_ms,
            },
        );
        Ok(())
    }

    pub(crate) fn observe_codex_lifecycle_before_event(
        &mut self,
        event: &AppEvent,
    ) -> Option<PendingCodexSessionEnd> {
        self.pending_codex_session_ends
            .retain(|terminal_id, _| self.state.terminals.contains_key(terminal_id));
        if let AppEvent::AgentConversationPromptCompleted {
            conversation_id, ..
        } = event
        {
            self.pending_codex_session_ends
                .retain(|_, pending| &pending.conversation_id != conversation_id);
        }
        let pane_id = match event {
            AppEvent::AgentProcessDetected { pane_id, .. }
            | AppEvent::AgentSessionReported { pane_id, .. }
            | AppEvent::StateChanged {
                pane_id,
                process_exited: true,
                ..
            }
            | AppEvent::PaneDied { pane_id, .. } => *pane_id,
            _ => return None,
        };
        let terminal_id = self.find_pane(pane_id)?.1.attached_terminal_id.clone();
        match event {
            AppEvent::AgentProcessDetected {
                agent, observed_at, ..
            } => {
                let elapsed_ms = Instant::now()
                    .saturating_duration_since(*observed_at)
                    .as_millis()
                    .min(u128::from(u64::MAX)) as u64;
                self.codex_process_acquisitions.insert(
                    terminal_id.clone(),
                    super::super::inbox::now_ms().saturating_sub(elapsed_ms),
                );
                self.pending_codex_session_ends.remove(&terminal_id);
                if self
                    .pending_codex_manual_resumes
                    .get(&terminal_id)
                    .is_some_and(|pending| pending.confirmed)
                {
                    self.pending_codex_manual_resumes.remove(&terminal_id);
                }
                let user_owned = *agent == Agent::Codex
                    && self
                        .pending_codex_manual_resumes
                        .get(&terminal_id)
                        .is_none_or(|pending| pending.user_owned)
                    && self
                        .state
                        .terminals
                        .get(&terminal_id)
                        .is_some_and(|terminal| !terminal.managed_agent_launch_pending());
                self.codex_process_origins
                    .insert(terminal_id.clone(), user_owned);
                if !user_owned {
                    self.pending_codex_manual_resumes.remove(&terminal_id);
                }
                None
            }
            AppEvent::AgentSessionReported { .. } => {
                self.pending_codex_session_ends.remove(&terminal_id);
                if self
                    .pending_codex_manual_resumes
                    .get(&terminal_id)
                    .is_some_and(|pending| !pending.confirmed)
                {
                    self.pending_codex_manual_resumes.remove(&terminal_id);
                }
                None
            }
            AppEvent::StateChanged {
                observed_at, agent, ..
            } => {
                self.codex_process_origins.remove(&terminal_id);
                self.codex_process_acquisitions.remove(&terminal_id);
                self.pending_codex_manual_resumes.remove(&terminal_id);
                let pending = self.pending_codex_session_ends.remove(&terminal_id)?;
                (*agent == Some(Agent::Codex)
                    && !(self.server_hangup_probe)()
                    && !self.state.should_quit
                    && *observed_at >= pending.observed_at
                    && self.codex_end_identity_is_current(&pending))
                .then_some(pending)
            }
            AppEvent::PaneDied { exit_reason, .. } => {
                self.codex_process_origins.remove(&terminal_id);
                self.codex_process_acquisitions.remove(&terminal_id);
                self.pending_codex_manual_resumes.remove(&terminal_id);
                let pending = self.pending_codex_session_ends.remove(&terminal_id)?;
                (*exit_reason == crate::platform::ChildExitReason::Exited
                    && !(self.server_hangup_probe)()
                    && !self.state.should_quit
                    && self.codex_end_identity_is_current(&pending))
                .then_some(pending)
            }
            _ => None,
        }
    }

    pub(super) fn observe_confirmed_codex_manual_resume(
        &mut self,
        terminal_id: TerminalId,
        session: PersistedAgentSession,
        user_owned: bool,
    ) {
        self.pending_codex_manual_resumes.insert(
            terminal_id,
            PendingCodexManualResume {
                session,
                user_owned,
                confirmed: false,
                observed_at: Instant::now(),
            },
        );
        self.finish_pending_codex_manual_resumes();
    }

    pub(crate) fn discard_pending_codex_manual_resumes_for(
        &mut self,
        record: &crate::agent_inbox::ConversationRecord,
    ) {
        self.pending_codex_manual_resumes.retain(|_, pending| {
            pending.session.agent != record.provider
                || pending.session.source != record.source
                || Some(&pending.session.session_ref) != record.session_ref.as_ref()
        });
    }

    pub(super) fn finish_pending_codex_manual_resumes(&mut self) {
        let terminals: Vec<_> = self.pending_codex_manual_resumes.keys().cloned().collect();
        for terminal_id in terminals {
            let mut pending = self
                .pending_codex_manual_resumes
                .remove(&terminal_id)
                .expect("selected pending resume");
            let Some(terminal) = self.state.terminals.get(&terminal_id) else {
                continue;
            };
            if terminal.provider_reported_session() != Some(&pending.session)
                || (!pending.confirmed && pending.observed_at.elapsed() > CODEX_END_RELEASE_WINDOW)
            {
                continue;
            }
            let origin = self.codex_process_origins.get(&terminal_id).copied();
            let process_confirmed = origin == Some(true)
                && terminal.detected_agent == Some(Agent::Codex)
                && !terminal.managed_agent_launch_pending();
            if pending.confirmed && !process_confirmed {
                continue;
            }
            if !pending.user_owned || !process_confirmed {
                if origin.is_none() {
                    self.pending_codex_manual_resumes
                        .insert(terminal_id, pending);
                }
                continue;
            }
            pending.confirmed = true;
            if !self
                .reopen_conversation_after_manual_resume(&terminal_id.to_string(), &pending.session)
            {
                self.pending_codex_manual_resumes
                    .insert(terminal_id, pending);
            }
        }
    }

    fn codex_end_identity_is_current(&self, pending: &PendingCodexSessionEnd) -> bool {
        if pending.observed_at.elapsed() > CODEX_END_RELEASE_WINDOW {
            return false;
        }
        self.state
            .terminals
            .get(&pending.terminal_id)
            .is_some_and(|terminal| terminal.provider_reported_session() == Some(&pending.session))
            && self
                .agent_inbox
                .get(&pending.conversation_id)
                .is_some_and(|record| {
                    record.last_human_submission_at_ms == pending.last_human_submission_at_ms
                        && record.attachments.len() == 1
                        && record.attachments[0].terminal_id == pending.terminal_id.to_string()
                })
    }

    pub(crate) fn finish_confirmed_codex_exit(&mut self, pending: Option<PendingCodexSessionEnd>) {
        let Some(pending) = pending else {
            return;
        };
        self.sync_agent_conversations();
        self.settle_conversation_after_confirmed_exit(
            &pending.conversation_id,
            &pending.terminal_id.to_string(),
            &pending.session,
            pending.last_human_submission_at_ms,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_inbox::{Disposition, ExecutionState, InboxRegistry};
    use crate::api::schema::{Method, PaneReportAgentSessionParams, Request};
    use crate::detect::AgentState;

    fn app_with_codex() -> (App, PaneId, TerminalId) {
        app_with_codex_store(None)
    }

    fn app_with_codex_store(path: Option<std::path::PathBuf>) -> (App, PaneId, TerminalId) {
        let (_, rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::new(
            &crate::config::Config::default(),
            crate::app::AppPolicy::TEST,
            None,
            rx,
            crate::api::EventHub::default(),
        );
        app.agent_inbox = InboxRegistry::load(path, "codex-lifecycle-test".into());
        app.state.workspaces = vec![crate::workspace::Workspace::test_new("codex")];
        app.state.ensure_test_terminals();
        app.state.active = Some(0);
        let pane = app.state.workspaces[0].tabs[0].root_pane;
        let terminal = app.find_pane(pane).unwrap().1.attached_terminal_id.clone();
        detect(&mut app, pane);
        start(&mut app, pane, "native", 10, "startup");
        (app, pane, terminal)
    }

    fn call(app: &mut App, method: Method) -> serde_json::Value {
        serde_json::from_str(&app.handle_api_request(Request {
            id: "codex-test".into(),
            method,
        }))
        .unwrap()
    }

    fn detect(app: &mut App, pane_id: PaneId) {
        app.handle_internal_event(AppEvent::AgentProcessDetected {
            pane_id,
            agent: Agent::Codex,
            observed_at: Instant::now(),
        });
    }

    fn report_start(app: &mut App, pane: PaneId, session: &str, seq: u64, source: &str) {
        let pane_id = app.public_pane_id(0, pane).unwrap();
        let result = call(
            app,
            Method::PaneReportAgentSession(PaneReportAgentSessionParams {
                pane_id,
                source: "herdr:codex".into(),
                agent: "codex".into(),
                seq: Some(seq),
                agent_session_id: Some(session.into()),
                agent_session_path: None,
                session_start_source: Some(source.into()),
                resume_argv: None,
            }),
        );
        assert!(result.get("error").is_none(), "{result}");
    }

    fn start(app: &mut App, pane: PaneId, session: &str, seq: u64, source: &str) {
        report_start(app, pane, session, seq, source);
        let terminal_id = app.find_pane(pane).unwrap().1.attached_terminal_id.clone();
        app.state
            .terminals
            .get_mut(&terminal_id)
            .unwrap()
            .set_detected_state(Some(Agent::Codex), AgentState::Idle);
        app.sync_agent_conversations();
    }

    fn end(app: &mut App, pane: PaneId, session: &str, seq: u64) -> serde_json::Value {
        let terminal_id = &app.find_pane(pane).unwrap().1.attached_terminal_id;
        if app
            .codex_process_acquisitions
            .get(terminal_id)
            .is_some_and(|acquired_at| super::super::super::inbox::now_ms() <= *acquired_at)
        {
            std::thread::sleep(Duration::from_millis(1));
        }
        let pane_id = app.public_pane_id(0, pane).unwrap();
        call(
            app,
            Method::PaneReportAgentSessionEnd(PaneReportAgentSessionEndParams {
                pane_id,
                source: "herdr:codex".into(),
                agent: "codex".into(),
                seq,
                agent_session_id: session.into(),
                reason: "other".into(),
                transcript_path: "/tmp/native.jsonl".into(),
                reported_at_unix_ms: super::super::super::inbox::now_ms(),
            }),
        )
    }

    fn release(app: &mut App, pane_id: PaneId) {
        app.handle_internal_event(AppEvent::StateChanged {
            pane_id,
            agent: Some(Agent::Codex),
            state: AgentState::Unknown,
            visible_blocker: false,
            visible_working: false,
            process_exited: true,
            observed_at: Instant::now(),
        });
    }

    #[test]
    fn native_end_requires_release_and_keeps_the_shell_pane() {
        let (mut app, pane, _) = app_with_codex();
        assert!(end(&mut app, pane, "native", 20).get("error").is_none());
        assert_eq!(app.agent_inbox.records()[0].disposition, Disposition::Inbox);
        release(&mut app, pane);
        let record = &app.agent_inbox.records()[0];
        assert_eq!(record.disposition, Disposition::Settled);
        assert_eq!(record.execution, ExecutionState::Closed);
        assert!(record.attachments.is_empty());
        assert!(app.find_pane(pane).is_some());
    }

    #[test]
    fn crash_without_end_and_wrong_session_do_not_settle() {
        let (mut app, pane, _) = app_with_codex();
        assert!(end(&mut app, pane, "different", 20).get("error").is_some());
        release(&mut app, pane);
        assert_eq!(app.agent_inbox.records()[0].disposition, Disposition::Inbox);
        assert!(end(&mut app, pane, "native", 21).get("error").is_some());
    }

    #[test]
    fn stale_duplicate_expired_and_replaced_process_ends_do_not_settle() {
        for replacement in [false, true] {
            let (mut app, pane, terminal) = app_with_codex();
            assert!(end(&mut app, pane, "native", 10).get("error").is_some());
            assert!(end(&mut app, pane, "native", 20).get("error").is_none());
            assert!(end(&mut app, pane, "native", 20).get("error").is_some());
            if replacement {
                detect(&mut app, pane);
            } else {
                app.pending_codex_session_ends
                    .get_mut(&terminal)
                    .unwrap()
                    .observed_at -= Duration::from_secs(11);
            }
            release(&mut app, pane);
            assert_eq!(app.agent_inbox.records()[0].disposition, Disposition::Inbox);
        }
    }

    #[test]
    fn accepted_new_session_start_cancels_an_earlier_end() {
        let (mut app, pane, _) = app_with_codex();
        assert!(end(&mut app, pane, "native", 20).get("error").is_none());
        start(&mut app, pane, "different", 30, "resume");
        release(&mut app, pane);
        assert!(app
            .agent_inbox
            .records()
            .iter()
            .all(|record| record.disposition == Disposition::Inbox));
    }

    #[test]
    fn manual_exact_resume_reopens_but_managed_native_restore_preserves_settlement() {
        for managed in [false, true] {
            for source_first in [false, true] {
                let (mut app, pane, terminal_id) = app_with_codex();
                assert!(end(&mut app, pane, "native", 20).get("error").is_none());
                release(&mut app, pane);
                assert_eq!(
                    app.agent_inbox.records()[0].disposition,
                    Disposition::Settled
                );
                if managed {
                    let terminal = app.state.terminals.get_mut(&terminal_id).unwrap();
                    terminal.set_persisted_agent_session(PersistedAgentSession {
                        source: "herdr:codex".into(),
                        agent: "codex".into(),
                        session_ref: crate::agent_resume::AgentSessionRef::id("native").unwrap(),
                    });
                    terminal.begin_native_resume_launch(Agent::Codex);
                }
                if source_first {
                    report_start(&mut app, pane, "native", 30, "resume");
                    assert_eq!(
                        app.agent_inbox.records()[0].disposition,
                        Disposition::Settled
                    );
                    detect(&mut app, pane);
                } else {
                    detect(&mut app, pane);
                    assert_eq!(
                        app.state.terminals[&terminal_id].effective_known_agent(),
                        Some(Agent::Codex),
                        "resume acquisition must be newer than the previous process exit"
                    );
                    report_start(&mut app, pane, "native", 30, "resume");
                }
                assert_eq!(
                    app.agent_inbox.records()[0].disposition,
                    if managed {
                        Disposition::Settled
                    } else {
                        Disposition::Inbox
                    },
                    "managed={managed}, source_first={source_first}"
                );
            }
        }
    }

    #[test]
    fn restored_native_session_can_end_before_lazy_session_start() {
        let (mut app, pane, terminal_id) = app_with_codex();
        let terminal = app.state.terminals.get_mut(&terminal_id).unwrap();
        let saved = terminal.persisted_agent_session.clone().unwrap();
        terminal.set_persisted_agent_session(saved);
        terminal.begin_native_resume_launch(Agent::Codex);
        detect(&mut app, pane);
        assert!(app.state.terminals[&terminal_id]
            .provider_reported_session()
            .is_none());
        assert_eq!(app.agent_inbox.records()[0].disposition, Disposition::Inbox);
        assert!(end(&mut app, pane, "native", 20).get("error").is_none());
        release(&mut app, pane);
        assert_eq!(
            app.agent_inbox.records()[0].disposition,
            Disposition::Settled
        );
    }

    #[test]
    fn a_different_terminal_generation_cannot_consume_end_evidence() {
        let (mut app, pane, old_id) = app_with_codex();
        assert!(end(&mut app, pane, "native", 20).get("error").is_none());
        let new_id = TerminalId::alloc();
        let mut replacement =
            crate::terminal::TerminalState::new(new_id.clone(), std::env::temp_dir());
        replacement.set_detected_state(Some(Agent::Codex), AgentState::Idle);
        replacement.set_persisted_agent_session(
            app.state.terminals[&old_id]
                .persisted_agent_session
                .clone()
                .unwrap(),
        );
        app.state.terminals.insert(new_id.clone(), replacement);
        app.state.workspaces[0].tabs[0]
            .panes
            .get_mut(&pane)
            .unwrap()
            .attached_terminal_id = new_id;
        app.state.terminals.remove(&old_id);
        app.sync_agent_conversations();
        release(&mut app, pane);
        assert_eq!(app.agent_inbox.records()[0].disposition, Disposition::Inbox);
    }

    #[test]
    fn duplicate_attachments_reject_end_and_cancel_previously_accepted_end() {
        for end_first in [false, true] {
            let (mut app, pane, terminal_id) = app_with_codex();
            if end_first {
                assert!(end(&mut app, pane, "native", 20).get("error").is_none());
            }
            let other = app.state.workspaces[0].test_split(ratatui::layout::Direction::Horizontal);
            app.state.ensure_test_terminals();
            let other_id = app.find_pane(other).unwrap().1.attached_terminal_id.clone();
            let saved = app.state.terminals[&terminal_id]
                .persisted_agent_session
                .clone()
                .unwrap();
            let terminal = app.state.terminals.get_mut(&other_id).unwrap();
            terminal.set_detected_state(Some(Agent::Codex), AgentState::Idle);
            terminal.set_persisted_agent_session(saved);
            app.sync_agent_conversations();
            assert_eq!(app.agent_inbox.records()[0].attachments.len(), 2);
            if !end_first {
                assert!(end(&mut app, pane, "native", 20).get("error").is_some());
            }
            release(&mut app, pane);
            assert_eq!(app.agent_inbox.records()[0].disposition, Disposition::Inbox);
        }
    }

    #[test]
    fn shutdown_hangup_and_quit_intent_prevent_settlement_on_process_release() {
        for hangup in [false, true] {
            let (mut app, pane, _) = app_with_codex();
            assert!(end(&mut app, pane, "native", 20).get("error").is_none());
            if hangup {
                app.server_hangup_probe = || true;
            } else {
                app.state.should_quit = true;
            }
            release(&mut app, pane);
            assert_eq!(app.agent_inbox.records()[0].disposition, Disposition::Inbox);
            assert!(app.pending_codex_session_ends.is_empty());
        }
    }

    #[test]
    fn delayed_old_process_end_cannot_claim_a_replacement_process() {
        let (mut app, pane, _) = app_with_codex();
        let old_emission_ms = super::super::super::inbox::now_ms();
        std::thread::sleep(Duration::from_millis(2));
        app.handle_internal_event(AppEvent::AgentProcessDetected {
            pane_id: pane,
            agent: Agent::Codex,
            observed_at: Instant::now(),
        });
        let pane_id = app.public_pane_id(0, pane).unwrap();
        let response = call(
            &mut app,
            Method::PaneReportAgentSessionEnd(PaneReportAgentSessionEndParams {
                pane_id,
                source: "herdr:codex".into(),
                agent: "codex".into(),
                seq: 20,
                agent_session_id: "native".into(),
                reason: "other".into(),
                transcript_path: "/tmp/native.jsonl".into(),
                reported_at_unix_ms: old_emission_ms,
            }),
        );
        assert!(response.get("error").is_some());
        release(&mut app, pane);
        assert_eq!(app.agent_inbox.records()[0].disposition, Disposition::Inbox);
    }

    #[test]
    fn confirmed_manual_resume_retries_persistence_and_respects_a_new_explicit_settle() {
        for explicit_settle in [false, true] {
            let base =
                std::env::temp_dir().join(format!("herdr-resume-retry-{}", TerminalId::alloc()));
            let store = base.join("store");
            let path = store.join("inbox.json");
            std::fs::create_dir_all(&store).unwrap();
            let (mut app, pane, terminal_id) = app_with_codex_store(Some(path.clone()));
            assert!(end(&mut app, pane, "native", 20).get("error").is_none());
            release(&mut app, pane);
            let settled = app.agent_inbox.records()[0].clone();
            let saved = std::fs::read(&path).unwrap();
            std::fs::remove_dir_all(&store).unwrap();
            std::fs::write(&store, "fixture blocks registry writes").unwrap();
            detect(&mut app, pane);
            report_start(&mut app, pane, "native", 30, "resume");
            assert_eq!(
                app.agent_inbox.get(&settled.id).unwrap().disposition,
                Disposition::Settled
            );
            assert!(app.agent_inbox_error.is_some());
            let pending = app
                .pending_codex_manual_resumes
                .get_mut(&terminal_id)
                .unwrap();
            assert!(pending.confirmed);
            pending.observed_at -= Duration::from_secs(3_600);
            std::fs::remove_file(&store).unwrap();
            std::fs::create_dir_all(&store).unwrap();
            std::fs::write(&path, saved).unwrap();
            if explicit_settle {
                let record = app.agent_inbox.get(&settled.id).unwrap().clone();
                let response = call(
                    &mut app,
                    Method::AgentConversationSettle(
                        crate::api::schema::AgentConversationMutationParams {
                            conversation_id: record.id,
                            expected_revision: record.revision,
                        },
                    ),
                );
                assert!(response.get("error").is_none(), "{response}");
                assert!(app.pending_codex_manual_resumes.is_empty());
                assert_eq!(
                    app.agent_inbox.get(&settled.id).unwrap().settled_at_ms,
                    settled.settled_at_ms
                );
            }
            let expected = if explicit_settle {
                Disposition::Settled
            } else {
                Disposition::Inbox
            };
            app.handle_internal_event(AppEvent::StateChanged {
                pane_id: pane,
                agent: Some(Agent::Codex),
                state: AgentState::Idle,
                visible_blocker: false,
                visible_working: false,
                process_exited: false,
                observed_at: Instant::now(),
            });
            assert_eq!(
                app.agent_inbox.get(&settled.id).unwrap().disposition,
                expected
            );
            assert!(app.pending_codex_manual_resumes.is_empty());
            assert!(app.agent_inbox_error.is_none());
            let restored = InboxRegistry::load(Some(path), "restarted".into());
            assert_eq!(restored.get(&settled.id).unwrap().disposition, expected);
            std::fs::remove_dir_all(base).unwrap();
        }
    }

    #[test]
    fn failed_confirmed_manual_resume_proof_does_not_survive_replacement_or_changed_identity() {
        for replacement in ["process", "session", "terminal"] {
            let base =
                std::env::temp_dir().join(format!("herdr-resume-stale-{}", TerminalId::alloc()));
            let store = base.join("store");
            let path = store.join("inbox.json");
            std::fs::create_dir_all(&store).unwrap();
            let (mut app, pane, terminal_id) = app_with_codex_store(Some(path.clone()));
            assert!(end(&mut app, pane, "native", 20).get("error").is_none());
            release(&mut app, pane);
            std::fs::remove_dir_all(&store).unwrap();
            std::fs::write(&store, "fixture blocks registry writes").unwrap();
            detect(&mut app, pane);
            report_start(&mut app, pane, "native", 30, "resume");
            assert!(app.pending_codex_manual_resumes[&terminal_id].confirmed);
            match replacement {
                "process" => detect(&mut app, pane),
                "session" => report_start(&mut app, pane, "different", 40, "startup"),
                _ => {
                    let new_id = TerminalId::alloc();
                    app.state.terminals.insert(
                        new_id.clone(),
                        crate::terminal::TerminalState::new(new_id.clone(), std::env::temp_dir()),
                    );
                    app.state.workspaces[0].tabs[0]
                        .panes
                        .get_mut(&pane)
                        .unwrap()
                        .attached_terminal_id = new_id;
                    app.state.terminals.remove(&terminal_id);
                    app.finish_pending_codex_manual_resumes();
                }
            }
            assert!(app.pending_codex_manual_resumes.is_empty(), "{replacement}");
            assert!(app
                .agent_inbox
                .records()
                .iter()
                .all(|record| record.disposition == Disposition::Settled));
            std::fs::remove_dir_all(base).unwrap();
        }
    }

    #[test]
    fn unconfirmed_manual_resume_proof_expires_while_waiting_for_a_process() {
        let (mut app, pane, terminal_id) = app_with_codex();
        assert!(end(&mut app, pane, "native", 20).get("error").is_none());
        release(&mut app, pane);
        report_start(&mut app, pane, "native", 30, "resume");
        let pending = app
            .pending_codex_manual_resumes
            .get_mut(&terminal_id)
            .unwrap();
        assert!(!pending.confirmed);
        pending.observed_at -= Duration::from_secs(11);
        detect(&mut app, pane);
        assert!(app.pending_codex_manual_resumes.is_empty());
        assert_eq!(
            app.agent_inbox.records()[0].disposition,
            Disposition::Settled
        );
    }

    #[test]
    fn endpoint_restart_discards_native_end_evidence() {
        let (mut app, pane, _) = app_with_codex();
        assert!(end(&mut app, pane, "native", 20).get("error").is_none());
        let (mut restarted, pane, _) = app_with_codex();
        assert!(restarted.pending_codex_session_ends.is_empty());
        release(&mut restarted, pane);
        assert_eq!(
            restarted.agent_inbox.records()[0].disposition,
            Disposition::Inbox
        );
    }
}
