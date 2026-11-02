use std::path::{Path, PathBuf};

use crate::api::schema::AgentConversationResumeLocation;
use crate::workspace::Workspace;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResumeDestination {
    Tab { workspace: usize, tab: usize },
    NewTab { workspace: usize },
    NewSpace,
}

// Public IDs can be reused after restart; only durable IDs recover a saved location.
fn original_destination(
    workspaces: &[Workspace],
    workspace_location_id: &str,
    tab_location_id: &str,
) -> Option<ResumeDestination> {
    if workspace_location_id.is_empty()
        || tab_location_id.is_empty()
        || workspaces
            .iter()
            .filter(|ws| ws.location_id == workspace_location_id)
            .count()
            > 1
    {
        return None;
    }
    let mut matches = workspaces.iter().enumerate().flat_map(|(workspace, ws)| {
        ws.tabs
            .iter()
            .enumerate()
            .filter_map(move |(tab, candidate)| {
                (candidate.location_id == tab_location_id)
                    .then_some(ResumeDestination::Tab { workspace, tab })
            })
    });
    let destination = matches.next()?;
    matches.next().is_none().then_some(destination)
}

fn validated_resume_cwd(cwd: &str) -> Result<PathBuf, String> {
    let path = Path::new(cwd);
    if !path.is_absolute() || !path.is_dir() {
        return Err(
            "The saved working directory is unavailable. Choose an existing absolute directory."
                .into(),
        );
    }
    Ok(path.to_path_buf())
}

fn validate_saved_session(session: &crate::agent_resume::AgentSessionRef) -> Result<(), String> {
    use crate::agent_resume::{AgentSessionRef, AgentSessionRefKind};
    let valid = match session.kind {
        AgentSessionRefKind::Id => {
            AgentSessionRef::id(session.value.clone()).is_some()
                && !session.value.starts_with('-')
                && !session
                    .value
                    .strip_prefix("default:")
                    .is_some_and(|id| id.starts_with('-'))
        }
        AgentSessionRefKind::Path => {
            AgentSessionRef::path(session.value.clone()).is_some()
                && Path::new(&session.value).is_file()
        }
    };
    if valid {
        Ok(())
    } else {
        Err(
            "The saved native session is missing or invalid. No replacement session was started."
                .into(),
        )
    }
}

fn resolve_resume_destination(
    workspaces: &[Workspace],
    workspace_location_id: &str,
    tab_location_id: &str,
    requested: Option<&AgentConversationResumeLocation>,
) -> Result<ResumeDestination, String> {
    match requested {
        None => original_destination(workspaces, workspace_location_id, tab_location_id)
            .ok_or_else(|| "The original tab or Space is missing or ambiguous. Choose Recreate tab, Recreate Space, or Choose location.".into()),
        Some(AgentConversationResumeLocation::RecreateTab) => {
            let matches: Vec<_> = workspaces.iter().enumerate()
                .filter(|(_, ws)| ws.location_id == workspace_location_id)
                .map(|(index, _)| index).collect();
            match matches.as_slice() {
                [workspace] => Ok(ResumeDestination::NewTab { workspace: *workspace }),
                _ => Err("The original Space is missing or ambiguous. Choose Recreate Space or Choose location.".into()),
            }
        },
        Some(AgentConversationResumeLocation::RecreateSpace) => Ok(ResumeDestination::NewSpace),
        Some(AgentConversationResumeLocation::Choose { workspace_id, tab_id, .. }) => {
            let workspace = workspaces.iter().position(|ws| ws.id == *workspace_id)
                .ok_or_else(|| "The selected Space no longer exists.".to_string())?;
            match tab_id {
                Some(tab_id) => workspaces[workspace].tabs.iter()
                    .position(|tab| crate::workspace::public_tab_id_for_number(workspace_id, tab.number) == *tab_id)
                    .map(|tab| ResumeDestination::Tab { workspace, tab })
                    .ok_or_else(|| "The selected tab no longer exists.".into()),
                None => Ok(ResumeDestination::NewTab { workspace }),
            }
        }
    }
}

impl super::App {
    fn launch_conversation_resume(
        &mut self,
        destination: ResumeDestination,
        cwd: PathBuf,
        plan: &crate::agent_resume::AgentResumePlan,
    ) -> std::io::Result<(usize, usize, crate::layout::PaneId)> {
        let (rows, cols) = self.state.new_pane_size(crate::ui::NewPanePlacement::Alone);
        let scrollback = self.state.pane_scrollback_limit_bytes;
        let theme = self.state.host_terminal_theme;
        let appearance = self.pane_host_terminal_appearance();
        let (workspace, tab, pane_id, terminal, runtime) = match destination {
            ResumeDestination::Tab { workspace, tab } => {
                let target = self.state.workspaces[workspace].tabs[tab].root_pane;
                let (rows, cols) = self
                    .state
                    .new_pane_size(crate::ui::NewPanePlacement::Split {
                        ws_idx: workspace,
                        target,
                        direction: ratatui::layout::Direction::Horizontal,
                        ratio: 0.5,
                    });
                let (tab, new_pane) = self.state.workspaces[workspace]
                    .split_pane_argv_command(
                        target,
                        ratatui::layout::Direction::Horizontal,
                        rows,
                        cols,
                        Some(cwd),
                        &plan.argv,
                        Vec::new(),
                        scrollback,
                        theme,
                        appearance,
                        false,
                    )
                    .ok_or_else(|| std::io::Error::other("The original tab disappeared."))??;
                (
                    workspace,
                    tab,
                    new_pane.pane_id,
                    new_pane.terminal,
                    new_pane.runtime,
                )
            }
            ResumeDestination::NewTab { workspace } => {
                let (tab, terminal, runtime) = self.state.workspaces[workspace]
                    .create_tab_argv_command(
                        rows,
                        cols,
                        cwd,
                        &plan.argv,
                        Vec::new(),
                        scrollback,
                        theme,
                        appearance,
                    )?;
                let pane_id = self.state.workspaces[workspace].tabs[tab].root_pane;
                (workspace, tab, pane_id, terminal, runtime)
            }
            ResumeDestination::NewSpace => {
                let (ws, terminal, runtime) = Workspace::new_with_tab(
                    cwd,
                    rows,
                    cols,
                    scrollback,
                    theme,
                    appearance,
                    crate::pane::PaneShellConfig::new("", crate::config::ShellModeConfig::NonLogin),
                    self.event_tx.clone(),
                    self.render_notify.clone(),
                    self.render_dirty.clone(),
                    Some(&plan.argv),
                    Vec::new(),
                )?;
                let pane_id = ws.tabs[0].root_pane;
                self.state.workspaces.push(ws);
                (
                    self.state.workspaces.len() - 1,
                    0,
                    pane_id,
                    terminal,
                    runtime,
                )
            }
        };
        self.terminal_runtimes.insert(terminal.id.clone(), runtime);
        self.state.terminals.insert(terminal.id.clone(), terminal);
        self.state.remove_alias_shadowed_by_new_pane(pane_id);
        self.schedule_session_save();
        match destination {
            ResumeDestination::Tab { .. } => {
                if let Some(pane) = self.pane_info(workspace, pane_id) {
                    self.emit_event(crate::api::schema::EventEnvelope {
                        event: crate::api::schema::EventKind::PaneCreated,
                        data: crate::api::schema::EventData::PaneCreated { pane },
                    });
                }
                self.emit_layout_updated_event(workspace, tab);
            }
            ResumeDestination::NewTab { .. } => self.emit_tab_created_events(workspace, tab),
            ResumeDestination::NewSpace => self.emit_workspace_open_events(workspace),
        }
        Ok((workspace, tab, pane_id))
    }
}

fn reported_same_session(
    terminal: &crate::terminal::TerminalState,
    record: &crate::agent_inbox::ConversationRecord,
) -> bool {
    terminal
        .provider_reported_session()
        .is_some_and(|reported| {
            reported.source == record.source
                && reported.agent == record.provider
                && Some(&reported.session_ref) == record.session_ref.as_ref()
        })
}

impl super::App {
    fn verified_resume_attachments(
        &self,
        record: &crate::agent_inbox::ConversationRecord,
    ) -> Vec<crate::agent_inbox::ConversationAttachment> {
        self.state
            .workspaces
            .iter()
            .flat_map(|workspace| {
                workspace
                    .tabs
                    .iter()
                    .enumerate()
                    .flat_map(move |(tab_index, tab)| {
                        tab.panes.iter().filter_map(move |(pane_id, pane)| {
                            let terminal = self.state.terminals.get(&pane.attached_terminal_id)?;
                            self.terminal_runtimes.get(&pane.attached_terminal_id)?;
                            if !reported_same_session(terminal, record) {
                                return None;
                            }
                            Some(crate::agent_inbox::ConversationAttachment {
                                terminal_id: terminal.id.to_string(),
                                pane_id: pane_id.raw(),
                                public_pane_id: workspace
                                    .public_pane_numbers
                                    .get(pane_id)
                                    .map(|number| {
                                        crate::workspace::public_pane_id_for_number(
                                            &workspace.id,
                                            *number,
                                        )
                                    })
                                    .unwrap_or_default(),
                                workspace_id: workspace.id.clone(),
                                workspace_label: workspace
                                    .display_name_from_terminals(&self.state.terminals),
                                tab_label: workspace
                                    .tab_display_name(tab_index)
                                    .unwrap_or_default(),
                                tab_id: crate::workspace::public_tab_id_for_number(
                                    &workspace.id,
                                    tab.number,
                                ),
                                workspace_location_id: workspace.location_id.clone(),
                                tab_location_id: tab.location_id.clone(),
                                cwd: terminal.cwd.to_string_lossy().into_owned(),
                            })
                        })
                    })
            })
            .collect()
    }

    pub(crate) fn fail_agent_conversation_resume_for_terminal(
        &mut self,
        terminal_id: &str,
    ) -> bool {
        use crate::agent_inbox::{ConversationAction, ResumeState};
        let mut changed = false;
        for record in self.agent_inbox.records() {
            if record.resume_state == ResumeState::None
                || !record.resume_target.as_ref().is_some_and(|target| {
                    !target.terminal_id.is_empty() && target.terminal_id == terminal_id
                })
            {
                continue;
            }
            let action = if self.verified_resume_attachments(&record).is_empty() {
                ConversationAction::ResumeFailed
            } else {
                ConversationAction::ResumeSucceeded
            };
            match self.agent_inbox.mutate(
                &record.id,
                record.revision,
                action,
                super::inbox::now_ms(),
            ) {
                Ok(_) => changed = true,
                Err(error) => self.agent_inbox_error = Some(error.to_string()),
            }
        }
        self.sync_agent_inbox_deadline();
        changed
    }

    pub(crate) fn poll_agent_conversation_resumes(&mut self) -> bool {
        use crate::agent_inbox::{ConversationAction, ResumeState};
        let now = super::inbox::now_ms();
        let mut changed = false;
        for record in self.agent_inbox.records() {
            if record.resume_state == ResumeState::None {
                continue;
            }
            let action = if !self.verified_resume_attachments(&record).is_empty() {
                Some(ConversationAction::ResumeSucceeded)
            } else if record.resume_state == ResumeState::Pending
                && record
                    .resume_started_at_ms
                    .is_some_and(|start| now.saturating_sub(start) >= 30_000)
            {
                Some(ConversationAction::ResumeUncertain)
            } else {
                None
            };
            if let Some(action) = action {
                match self
                    .agent_inbox
                    .mutate(&record.id, record.revision, action, now)
                {
                    Ok(_) => changed = true,
                    Err(error) => self.agent_inbox_error = Some(error.to_string()),
                }
            }
        }
        if changed {
            self.render_dirty.request_generic();
            self.render_notify.notify_one();
        }
        changed
    }

    pub(super) fn handle_agent_conversation_resume(
        &mut self,
        id: String,
        params: crate::api::schema::AgentConversationResumeParams,
    ) -> String {
        use super::api::responses::encode_error;
        use crate::agent_inbox::{ConversationAction, ResumeState};
        self.poll_agent_conversation_resumes();
        let Some(record) = self.agent_inbox.get(&params.conversation_id).cloned() else {
            return encode_error(id, "conversation_not_found", "Conversation not found.");
        };
        if record.resume_state != ResumeState::None {
            let status = if record.resume_state == ResumeState::Pending {
                "pending"
            } else {
                "uncertain"
            };
            let message = if record.provider == "codex" {
                "Awaiting session confirmation. Codex reports it on your next turn. Open the existing resume target; do not retry."
            } else {
                "A resume is already pending or its result is uncertain. Reconcile the existing session before retrying."
            };
            return resume_response(id, status, &record, Some(message));
        }
        if !self.verified_resume_attachments(&record).is_empty() {
            return match self.agent_inbox.mutate(
                &record.id,
                params.expected_revision,
                ConversationAction::ResumeSucceeded,
                super::inbox::now_ms(),
            ) {
                Ok(record) => resume_response(id, "confirmed", &record, None),
                Err(error) => encode_error(id, "conversation_resume_failed", error.to_string()),
            };
        }
        if !record.attachments.is_empty() {
            return resume_response(id, "uncertain", &record,
                Some("An existing attachment has not reported the saved provider session. No additional process was started."));
        }
        let Some(session_ref) = record.session_ref.as_ref() else {
            return resume_response(
                id,
                "failed",
                &record,
                Some("This conversation has no verified native session identity."),
            );
        };
        if let Err(message) = validate_saved_session(session_ref) {
            return resume_response(id, "failed", &record, Some(&message));
        }
        let Some(plan) = crate::agent_resume::plan(&record.source, &record.provider, session_ref)
        else {
            return resume_response(
                id,
                "failed",
                &record,
                Some("This provider does not support exact session resume."),
            );
        };
        let cwd = match params.location.as_ref() {
            Some(AgentConversationResumeLocation::Choose { cwd: Some(cwd), .. }) => cwd.as_str(),
            _ => record.saved_location.cwd.as_str(),
        };
        let cwd = match validated_resume_cwd(cwd) {
            Ok(cwd) => cwd,
            Err(message) => {
                return resume_response(id, "location_required", &record, Some(&message))
            }
        };
        let destination = match resolve_resume_destination(
            &self.state.workspaces,
            &record.saved_location.workspace_location_id,
            &record.saved_location.tab_location_id,
            params.location.as_ref(),
        ) {
            Ok(destination) => destination,
            Err(message) => {
                return resume_response(id, "location_required", &record, Some(&message))
            }
        };
        let pending = match self.agent_inbox.mutate(
            &record.id,
            params.expected_revision,
            ConversationAction::BeginResume,
            super::inbox::now_ms(),
        ) {
            Ok(record) => record,
            Err(error) => return encode_error(id, "conversation_resume_failed", error.to_string()),
        };
        self.sync_agent_inbox_deadline();
        match self.launch_conversation_resume(destination, cwd, &plan) {
            Ok((workspace, tab, pane_id)) => {
                let ws = &self.state.workspaces[workspace];
                let tab_label = ws.tab_display_name(tab).unwrap_or_default();
                let tab = &ws.tabs[tab];
                let target = crate::agent_inbox::ConversationAttachment {
                    terminal_id: tab.panes[&pane_id].attached_terminal_id.to_string(),
                    pane_id: pane_id.raw(),
                    public_pane_id: ws
                        .public_pane_numbers
                        .get(&pane_id)
                        .map(|number| crate::workspace::public_pane_id_for_number(&ws.id, *number))
                        .unwrap_or_default(),
                    workspace_id: ws.id.clone(),
                    workspace_label: ws.display_name_from_terminals(&self.state.terminals),
                    tab_label,
                    tab_id: crate::workspace::public_tab_id_for_number(&ws.id, tab.number),
                    workspace_location_id: ws.location_id.clone(),
                    tab_location_id: tab.location_id.clone(),
                    cwd: self.state.terminals[&tab.panes[&pane_id].attached_terminal_id]
                        .cwd
                        .to_string_lossy()
                        .into_owned(),
                };
                match self.agent_inbox.mutate(
                    &record.id,
                    pending.revision,
                    ConversationAction::ResumeLaunched(Box::new(target)),
                    super::inbox::now_ms(),
                ) {
                    Ok(record) => resume_response(
                        id,
                        "pending",
                        &record,
                        Some(if record.provider == "codex" {
                            "Awaiting session confirmation. Codex reports it on your next turn. Open the resumed terminal to continue."
                        } else {
                            "Waiting for the provider to report the exact saved session."
                        }),
                    ),
                    Err(error) => {
                        self.agent_inbox_error = Some(error.to_string());
                        resume_response(id, "uncertain", &pending, Some("The process started, but its launch target could not be saved. Do not retry until the existing process is reconciled."))
                    }
                }
            }
            Err(error) => match self.agent_inbox.mutate(
                &record.id,
                pending.revision,
                ConversationAction::ResumeFailed,
                super::inbox::now_ms(),
            ) {
                Ok(record) => resume_response(id, "failed", &record, Some(&error.to_string())),
                Err(persist_error) => {
                    self.agent_inbox_error = Some(persist_error.to_string());
                    resume_response(id, "uncertain", &pending, Some("Launch failed and its outcome could not be saved. Reconcile before retrying."))
                }
            },
        }
    }
}

fn resume_response(
    id: String,
    status: &str,
    record: &crate::agent_inbox::ConversationRecord,
    message: Option<&str>,
) -> String {
    super::api::responses::encode_success(
        id,
        crate::api::schema::ResponseResult::AgentConversationResume {
            result: serde_json::json!({"status": status, "conversation": record, "message": message}),
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_location_never_falls_back_to_current_or_reused_public_ids() {
        let original = Workspace::test_new("original");
        let saved_workspace = original.location_id.clone();
        let saved_tab = original.tabs[0].location_id.clone();
        let mut replacement = Workspace::test_new("replacement");
        replacement.id = original.id.clone();
        assert!(
            resolve_resume_destination(&[replacement], &saved_workspace, &saved_tab, None).is_err()
        );
        assert!(original_destination(&[original], "", "").is_none());
    }

    #[test]
    fn tab_move_preserves_original_location_and_missing_space_requires_choice() {
        let mut original = Workspace::test_new("original");
        let saved_workspace = original.location_id.clone();
        let saved_tab = original.tabs[0].location_id.clone();
        let mut destination = Workspace::test_new("destination");
        destination.tabs.push(original.tabs.remove(0));
        assert_eq!(
            original_destination(&[destination], &saved_workspace, &saved_tab),
            Some(ResumeDestination::Tab {
                workspace: 0,
                tab: 1
            })
        );
        assert_eq!(
            resolve_resume_destination(
                &[],
                &saved_workspace,
                &saved_tab,
                Some(&AgentConversationResumeLocation::RecreateSpace)
            )
            .unwrap(),
            ResumeDestination::NewSpace
        );
        assert!(resolve_resume_destination(
            &[],
            &saved_workspace,
            &saved_tab,
            Some(&AgentConversationResumeLocation::RecreateTab)
        )
        .is_err());
    }

    #[test]
    fn duplicate_durable_ids_require_location_choice() {
        let first = Workspace::test_new("one");
        let mut second = Workspace::test_new("two");
        second.location_id = first.location_id.clone();
        second.tabs[0].location_id = first.tabs[0].location_id.clone();
        let saved_workspace = first.location_id.clone();
        let saved_tab = first.tabs[0].location_id.clone();
        let workspaces = [first, second];
        assert!(original_destination(&workspaces, &saved_workspace, &saved_tab).is_none());
        assert!(resolve_resume_destination(
            &workspaces,
            &saved_workspace,
            &saved_tab,
            Some(&AgentConversationResumeLocation::RecreateTab)
        )
        .is_err());
    }

    fn saved_registry() -> crate::agent_inbox::InboxRegistry {
        use crate::agent_inbox::*;
        let mut registry = InboxRegistry::load(None, "test".into());
        registry
            .reconcile(
                &[ConversationObservation {
                    source: "herdr:codex".into(),
                    provider: "codex".into(),
                    session_ref: crate::agent_resume::AgentSessionRef::id("saved-session"),
                    generation: "terminal".into(),
                    title: "Saved".into(),
                    attachment: ConversationAttachment {
                        terminal_id: "saved-terminal".into(),
                        pane_id: 1,
                        public_pane_id: "w1:p1".into(),
                        workspace_id: "w1".into(),
                        workspace_label: "Saved Space".into(),
                        tab_label: "Saved tab".into(),
                        tab_id: "w1:t1".into(),
                        workspace_location_id: "space".into(),
                        tab_location_id: "tab".into(),
                        cwd: "/".into(),
                    },
                    execution: ExecutionState::Idle,
                    queued_human_work: false,
                    blocking_request: false,
                    failure: false,
                    capability_limits: Vec::new(),
                }],
                1,
            )
            .unwrap();
        registry
    }

    #[test]
    fn resume_confirmation_requires_provider_report_and_rejects_wrong_or_stale_session() {
        let record = saved_registry().records().remove(0);
        let mut terminal =
            crate::terminal::TerminalState::new(crate::terminal::TerminalId::alloc(), "/".into());
        terminal.set_managed_agent_launch_session(crate::agent_resume::PersistedAgentSession {
            source: record.source.clone(),
            agent: record.provider.clone(),
            session_ref: record.session_ref.clone().unwrap(),
        });
        assert!(
            !reported_same_session(&terminal, &record),
            "launch metadata is not provider confirmation"
        );
        terminal.set_hook_authority_with_session_ref(
            "herdr:codex".into(),
            "codex".into(),
            crate::detect::AgentState::Idle,
            None,
            crate::agent_resume::AgentSessionRef::id("other-session"),
            Some(2),
        );
        assert!(!reported_same_session(&terminal, &record));
        terminal.set_hook_authority_with_session_ref(
            "herdr:codex".into(),
            "codex".into(),
            crate::detect::AgentState::Idle,
            None,
            record.session_ref.clone(),
            Some(1),
        );
        assert!(
            !reported_same_session(&terminal, &record),
            "stale report cannot confirm resume"
        );
        terminal.set_hook_authority_with_session_ref(
            "herdr:codex".into(),
            "codex".into(),
            crate::detect::AgentState::Idle,
            None,
            record.session_ref.clone(),
            Some(3),
        );
        assert!(reported_same_session(&terminal, &record));
    }

    #[test]
    fn accepted_session_only_report_confirms_but_launch_restore_and_process_exit_do_not() {
        let record = saved_registry().records().remove(0);
        let mut terminal =
            crate::terminal::TerminalState::new(crate::terminal::TerminalId::alloc(), "/".into());
        let session = crate::agent_resume::PersistedAgentSession {
            source: record.source.clone(),
            agent: record.provider.clone(),
            session_ref: record.session_ref.clone().unwrap(),
        };
        terminal.set_persisted_agent_session(session.clone());
        assert!(!reported_same_session(&terminal, &record));
        terminal
            .set_agent_session_ref(
                record.source.clone(),
                record.provider.clone(),
                record.session_ref.clone(),
                Some(1),
            )
            .unwrap();
        assert!(terminal.hook_authority.is_none());
        assert!(reported_same_session(&terminal, &record));
        terminal.set_managed_agent_launch_session(session);
        assert!(!reported_same_session(&terminal, &record));
        terminal
            .set_agent_session_ref(
                record.source.clone(),
                record.provider.clone(),
                record.session_ref.clone(),
                Some(2),
            )
            .unwrap();
        assert!(reported_same_session(&terminal, &record));
        terminal.set_detected_state_with_screen_signals_at(
            Some(crate::detect::Agent::Codex),
            crate::detect::AgentState::Idle,
            false,
            false,
            false,
            true,
            std::time::Instant::now(),
        );
        assert!(!reported_same_session(&terminal, &record));
        terminal.clear_agent_runtime_identity_after_respawn();
        assert!(!reported_same_session(&terminal, &record));
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn uncertain_resume_rebinds_to_exact_report_after_location_and_raw_id_change() {
        use crate::agent_inbox::{ConversationAction, Disposition, ResumeState};
        let (_, api_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = super::super::App::new(
            &crate::config::Config::default(),
            super::super::AppPolicy::TEST,
            None,
            api_rx,
            crate::api::EventHub::default(),
        );
        app.agent_inbox = saved_registry();
        let now = super::super::inbox::now_ms();
        let mut record = app.agent_inbox.records().remove(0);
        record = app
            .agent_inbox
            .mutate(&record.id, record.revision, ConversationAction::Settle, now)
            .unwrap();
        app.agent_inbox.reconcile(&[], now).unwrap();
        record = app.agent_inbox.get(&record.id).unwrap().clone();
        record = app
            .agent_inbox
            .mutate(
                &record.id,
                record.revision,
                ConversationAction::BeginResume,
                now,
            )
            .unwrap();
        let mut old_target = record.saved_location.clone();
        old_target.pane_id = u32::MAX;
        record = app
            .agent_inbox
            .mutate(
                &record.id,
                record.revision,
                ConversationAction::ResumeLaunched(Box::new(old_target)),
                now,
            )
            .unwrap();
        record = app
            .agent_inbox
            .mutate(
                &record.id,
                record.revision,
                ConversationAction::ResumeUncertain,
                now,
            )
            .unwrap();
        let plan = crate::agent_resume::AgentResumePlan {
            agent: "codex".into(),
            argv: vec!["/bin/sh".into(), "-c".into(), "sleep 5".into()],
            dedupe_key: "test".into(),
        };
        let (workspace, tab, pane) = app
            .launch_conversation_resume(ResumeDestination::NewSpace, std::env::temp_dir(), &plan)
            .unwrap();
        let terminal_id = app.state.workspaces[workspace].tabs[tab].panes[&pane]
            .attached_terminal_id
            .clone();
        let terminal = app.state.terminals.get_mut(&terminal_id).unwrap();
        terminal.set_detected_state(
            Some(crate::detect::Agent::Codex),
            crate::detect::AgentState::Idle,
        );
        terminal
            .set_agent_session_ref(
                record.source.clone(),
                record.provider.clone(),
                record.session_ref.clone(),
                Some(1),
            )
            .unwrap();
        assert!(app.poll_agent_conversation_resumes());
        let confirmed = app.agent_inbox.get(&record.id).unwrap();
        assert_eq!(confirmed.resume_state, ResumeState::None);
        assert_eq!(confirmed.disposition, Disposition::Inbox);
        assert!(confirmed.resume_target.is_none());
        for (_, runtime) in app.terminal_runtimes.drain() {
            runtime.shutdown();
        }
    }

    #[tokio::test]
    async fn definitive_exit_only_releases_the_exact_launched_terminal_generation() {
        use crate::agent_inbox::{ConversationAction, Disposition, ResumeState};
        let (_, api_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = super::super::App::new(
            &crate::config::Config::default(),
            super::super::AppPolicy::TEST,
            None,
            api_rx,
            crate::api::EventHub::default(),
        );
        app.agent_inbox = saved_registry();
        let now = super::super::inbox::now_ms();
        let record = app.agent_inbox.records().remove(0);
        let record = app
            .agent_inbox
            .mutate(&record.id, record.revision, ConversationAction::Settle, now)
            .unwrap();
        app.agent_inbox.reconcile(&[], now).unwrap();
        let record = app.agent_inbox.get(&record.id).unwrap().clone();
        let record = app
            .agent_inbox
            .mutate(
                &record.id,
                record.revision,
                ConversationAction::BeginResume,
                now,
            )
            .unwrap();
        let mut target = record.saved_location.clone();
        target.terminal_id = "launched-generation".into();
        let record = app
            .agent_inbox
            .mutate(
                &record.id,
                record.revision,
                ConversationAction::ResumeLaunched(Box::new(target)),
                now,
            )
            .unwrap();
        let record = app
            .agent_inbox
            .mutate(
                &record.id,
                record.revision,
                ConversationAction::ResumeUncertain,
                now,
            )
            .unwrap();
        assert!(!app.fail_agent_conversation_resume_for_terminal("reallocated-generation"));
        assert_eq!(
            app.agent_inbox.get(&record.id).unwrap().resume_state,
            ResumeState::Uncertain
        );
        assert!(app.fail_agent_conversation_resume_for_terminal("launched-generation"));
        let failed = app.agent_inbox.get(&record.id).unwrap();
        assert_eq!(failed.resume_state, ResumeState::None);
        assert_eq!(failed.disposition, Disposition::Settled);
        assert!(failed.resume_target.is_none());
        assert!(failed
            .resume_error
            .as_ref()
            .is_some_and(|error| error.contains("failed")));
    }

    #[tokio::test]
    async fn pending_and_uncertain_resume_retries_never_create_another_pane() {
        use crate::agent_inbox::{ConversationAction, ResumeState};
        let (_, api_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = super::super::App::new(
            &crate::config::Config::default(),
            super::super::AppPolicy::TEST,
            None,
            api_rx,
            crate::api::EventHub::default(),
        );
        app.agent_inbox = saved_registry();
        let now = super::super::inbox::now_ms();
        app.agent_inbox.reconcile(&[], now).unwrap();
        let record = app.agent_inbox.records().remove(0);
        let pending = app
            .agent_inbox
            .mutate(
                &record.id,
                record.revision,
                ConversationAction::BeginResume,
                now,
            )
            .unwrap();
        for expected in [ResumeState::Pending, ResumeState::Uncertain] {
            if expected == ResumeState::Uncertain {
                let current = app.agent_inbox.get(&record.id).unwrap();
                app.agent_inbox
                    .mutate(
                        &record.id,
                        current.revision,
                        ConversationAction::ResumeUncertain,
                        now,
                    )
                    .unwrap();
            }
            let response = app.handle_agent_conversation_resume(
                "retry".into(),
                crate::api::schema::AgentConversationResumeParams {
                    conversation_id: record.id.clone(),
                    expected_revision: pending.revision,
                    location: Some(AgentConversationResumeLocation::RecreateSpace),
                },
            );
            let response: serde_json::Value = serde_json::from_str(&response).unwrap();
            assert_eq!(
                response["result"]["result"]["status"],
                if expected == ResumeState::Pending {
                    "pending"
                } else {
                    "uncertain"
                }
            );
            assert!(app.state.workspaces.is_empty());
            assert_eq!(
                app.agent_inbox.get(&record.id).unwrap().resume_state,
                expected
            );
        }
    }

    #[test]
    fn missing_session_paths_and_option_like_ids_do_not_launch() {
        assert!(validate_saved_session(
            &crate::agent_resume::AgentSessionRef::id("--last").unwrap()
        )
        .is_err());
        assert!(validate_saved_session(
            &crate::agent_resume::AgentSessionRef::path(
                std::env::temp_dir()
                    .join("herdr-missing-session.jsonl")
                    .to_string_lossy(),
            )
            .unwrap()
        )
        .is_err());
        assert!(validate_saved_session(
            &crate::agent_resume::AgentSessionRef::id("native-session").unwrap()
        )
        .is_ok());
    }

    #[test]
    fn invalid_cwd_is_rejected_without_directory_creation() {
        assert!(validated_resume_cwd(".").is_err());
        assert!(validated_resume_cwd("/herdr-does-not-exist/resume").is_err());
        assert!(validated_resume_cwd(&std::env::temp_dir().to_string_lossy()).is_ok());
    }
}
