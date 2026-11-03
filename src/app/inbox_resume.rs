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

fn resolve_restore_destination(
    workspaces: &[Workspace],
    terminals: &std::collections::HashMap<
        crate::terminal::TerminalId,
        crate::terminal::TerminalState,
    >,
    saved: &crate::agent_inbox::ConversationAttachment,
) -> Result<ResumeDestination, String> {
    if !saved.workspace_location_id.is_empty()
        && workspaces
            .iter()
            .filter(|ws| ws.location_id == saved.workspace_location_id)
            .count()
            > 1
    {
        return Err("The saved Space identity is ambiguous. Choose a location.".into());
    }
    let durable_tabs: Vec<_> = workspaces
        .iter()
        .enumerate()
        .flat_map(|(workspace, ws)| {
            ws.tabs
                .iter()
                .enumerate()
                .filter_map(move |(tab, candidate)| {
                    (!saved.tab_location_id.is_empty()
                        && candidate.location_id == saved.tab_location_id)
                        .then_some(ResumeDestination::Tab { workspace, tab })
                })
        })
        .collect();
    if durable_tabs.len() > 1 {
        return Err("The saved tab identity is ambiguous. Choose a location.".into());
    }
    if let Some(destination) = durable_tabs.first() {
        return Ok(*destination);
    }
    let durable_spaces: Vec<_> = workspaces
        .iter()
        .enumerate()
        .filter(|(_, ws)| {
            !saved.workspace_location_id.is_empty() && ws.location_id == saved.workspace_location_id
        })
        .map(|(workspace, _)| workspace)
        .collect();
    if durable_spaces.len() > 1 {
        return Err("The saved Space identity is ambiguous. Choose a location.".into());
    }
    let workspace = if let Some(workspace) = durable_spaces.first() {
        Some(*workspace)
    } else {
        if saved.workspace_label.trim().is_empty() {
            return Err("The saved Space has no recoverable name. Choose a location.".into());
        }
        // Stable migrations can replace durable IDs. Human names are a fallback only when unique.
        let named: Vec<_> = workspaces
            .iter()
            .enumerate()
            .filter(|(_, ws)| ws.display_name_from_terminals(terminals) == saved.workspace_label)
            .map(|(workspace, _)| workspace)
            .collect();
        match named.as_slice() {
            [] => None,
            [workspace] => Some(*workspace),
            _ => return Err("Several Spaces have the saved name. Choose a location.".into()),
        }
    };
    let Some(workspace) = workspace else {
        return Ok(ResumeDestination::NewSpace);
    };
    if saved.tab_label.trim().is_empty() {
        return Err("The saved tab has no recoverable name. Choose a location.".into());
    }
    let named: Vec<_> = workspaces[workspace]
        .tabs
        .iter()
        .enumerate()
        .filter(|(tab, _)| {
            workspaces[workspace].tab_display_name(*tab).as_deref()
                == Some(saved.tab_label.as_str())
        })
        .map(|(tab, _)| tab)
        .collect();
    match named.as_slice() {
        [] => Ok(ResumeDestination::NewTab { workspace }),
        [tab] => Ok(ResumeDestination::Tab {
            workspace,
            tab: *tab,
        }),
        _ => Err("Several tabs in the saved Space have the saved name. Choose a location.".into()),
    }
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
        focus: bool,
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
        if focus {
            self.state.focus_pane_in_workspace(workspace, pane_id);
            self.state.mark_active_tab_seen();
            self.state.mode = crate::app::Mode::Terminal;
        }
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
                                pane_label: terminal.manual_label.clone(),
                                pane_number: tab
                                    .layout
                                    .pane_ids()
                                    .iter()
                                    .position(|id| id == pane_id)
                                    .map(|index| index + 1),
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

    pub(super) fn handle_agent_conversation_restore(
        &mut self,
        id: String,
        params: crate::api::schema::AgentConversationRestoreParams,
    ) -> String {
        self.handle_agent_conversation_resume_inner(
            id,
            crate::api::schema::AgentConversationResumeParams {
                conversation_id: params.conversation_id,
                expected_revision: params.expected_revision,
                location: params.location,
            },
            true,
            params.cwd,
        )
    }

    fn focus_conversation_attachment(
        &mut self,
        attachment: &crate::agent_inbox::ConversationAttachment,
    ) -> Option<String> {
        if attachment.terminal_id.is_empty() {
            return None;
        }
        let terminals = &self.state.terminals;
        let matches: Vec<_> = self
            .state
            .workspaces
            .iter()
            .enumerate()
            .flat_map(|(workspace, ws)| {
                ws.tabs.iter().flat_map(move |tab| {
                    tab.panes.iter().filter_map(move |(pane_id, pane)| {
                        (pane.attached_terminal_id.as_str() == attachment.terminal_id
                            && terminals.contains_key(&pane.attached_terminal_id))
                        .then_some((workspace, *pane_id))
                    })
                })
            })
            .collect();
        let [(workspace, pane_id)] = matches.as_slice() else {
            return None;
        };
        let public = self.public_pane_id(*workspace, *pane_id)?;
        self.state.focus_pane_in_workspace(*workspace, *pane_id);
        self.state.mark_active_tab_seen();
        self.state.mode = crate::app::Mode::Terminal;
        Some(public)
    }

    pub(super) fn handle_agent_conversation_resume(
        &mut self,
        id: String,
        params: crate::api::schema::AgentConversationResumeParams,
    ) -> String {
        self.handle_agent_conversation_resume_inner(id, params, false, None)
    }

    fn handle_agent_conversation_resume_inner(
        &mut self,
        id: String,
        params: crate::api::schema::AgentConversationResumeParams,
        restore: bool,
        cwd_override: Option<String>,
    ) -> String {
        use super::api::responses::encode_error;
        use crate::agent_inbox::{ConversationAction, ResumeState};
        self.poll_agent_conversation_resumes();
        let Some(record) = self.agent_inbox.get(&params.conversation_id).cloned() else {
            return encode_error(id, "conversation_not_found", "Conversation not found.");
        };
        if restore {
            if record.resume_state != ResumeState::None {
                let pane_id = record.resume_target.as_ref().and_then(|target| {
                    let identity_changed = self
                        .parse_pane_id(&target.public_pane_id)
                        .and_then(|(workspace, pane)| self.agent_info(workspace, pane))
                        .and_then(|info| info.agent_session)
                        .is_some_and(|session| {
                            session.source != record.source
                                || session.agent != record.provider
                                || record.session_ref.as_ref().is_none_or(|saved| {
                                    saved.kind != session.kind || saved.value != session.value
                                })
                        });
                    (!identity_changed)
                        .then(|| self.focus_conversation_attachment(target))
                        .flatten()
                });
                return restore_response(id, if record.resume_state == ResumeState::Pending { "pending" } else { "uncertain" }, &record,
                    Some("Awaiting native session confirmation. Continue in the reserved terminal; no additional process was started."), pane_id);
            }
            if record.revision != params.expected_revision {
                return encode_error(
                    id,
                    "conversation_revision_conflict",
                    "Conversation changed. Refresh before restoring it.",
                );
            }
            let mut attachments = self.verified_resume_attachments(&record);
            for attachment in &record.attachments {
                let same_session = self
                    .parse_pane_id(&attachment.public_pane_id)
                    .and_then(|(workspace, pane_id)| self.agent_info(workspace, pane_id))
                    .is_some_and(|info| {
                        info.terminal_id == attachment.terminal_id
                            && info.agent_session.as_ref().is_some_and(|session| {
                                session.source == record.source
                                    && session.agent == record.provider
                                    && record.session_ref.as_ref().is_some_and(|saved| {
                                        saved.kind == session.kind && saved.value == session.value
                                    })
                            })
                    });
                if same_session
                    && !attachments
                        .iter()
                        .any(|current| current.terminal_id == attachment.terminal_id)
                {
                    attachments.push(attachment.clone());
                }
            }
            if attachments.len() > 1 || record.attachments.len() > 1 {
                return encode_error(id, "conversation_attachment_required", "This conversation has multiple live attachments. Choose the attachment to open.");
            }
            if let Some(attachment) = attachments.first() {
                let running = self
                    .terminal_runtimes
                    .iter()
                    .any(|(terminal, _)| terminal.as_str() == attachment.terminal_id);
                if !running {
                    let pane_id = self.focus_conversation_attachment(attachment);
                    return restore_response(id, "uncertain", &record,
                        Some("The existing terminal has no confirmed running native session. No additional process was started."), pane_id);
                }
                return match self.agent_inbox.mutate(
                    &record.id,
                    params.expected_revision,
                    ConversationAction::ResumeSucceeded,
                    super::inbox::now_ms(),
                ) {
                    Ok(record) => match self.focus_conversation_attachment(attachment) {
                        Some(pane_id) => {
                            restore_response(id, "confirmed", &record, None, Some(pane_id))
                        }
                        None => encode_error(
                            id,
                            "conversation_attachment_unavailable",
                            "The exact saved terminal is no longer available.",
                        ),
                    },
                    Err(error) => encode_error(id, "conversation_resume_failed", error.to_string()),
                };
            }
        }
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
        let cwd = match (cwd_override.as_deref(), params.location.as_ref()) {
            (Some(cwd), _) => cwd,
            (None, Some(AgentConversationResumeLocation::Choose { cwd: Some(cwd), .. })) => {
                cwd.as_str()
            }
            _ => record.saved_location.cwd.as_str(),
        };
        let cwd = match validated_resume_cwd(cwd) {
            Ok(cwd) => cwd,
            Err(message) => {
                return resume_response(
                    id,
                    if restore {
                        "cwd_required"
                    } else {
                        "location_required"
                    },
                    &record,
                    Some(&message),
                )
            }
        };
        let resolved = if restore && params.location.is_none() {
            resolve_restore_destination(
                &self.state.workspaces,
                &self.state.terminals,
                &record.saved_location,
            )
        } else {
            resolve_resume_destination(
                &self.state.workspaces,
                &record.saved_location.workspace_location_id,
                &record.saved_location.tab_location_id,
                params.location.as_ref(),
            )
        };
        let destination = match resolved {
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
        match self.launch_conversation_resume(destination, cwd, &plan, restore) {
            Ok((workspace, tab, pane_id)) => {
                if restore {
                    if let Some(label) = &record.saved_location.pane_label {
                        let terminal_id = &self.state.workspaces[workspace].tabs[tab].panes
                            [&pane_id]
                            .attached_terminal_id;
                        if let Some(terminal) = self.state.terminals.get_mut(terminal_id) {
                            terminal.set_manual_label(label.clone());
                        }
                    }
                    if matches!(destination, ResumeDestination::NewSpace) {
                        if !record.saved_location.workspace_label.trim().is_empty() {
                            self.state.workspaces[workspace]
                                .set_custom_name(record.saved_location.workspace_label.clone());
                        }
                        if params.location.is_none()
                            && !record.saved_location.workspace_location_id.is_empty()
                        {
                            self.state.workspaces[workspace].location_id =
                                record.saved_location.workspace_location_id.clone();
                        }
                    }
                    if !matches!(destination, ResumeDestination::Tab { .. }) {
                        if !record.saved_location.tab_label.trim().is_empty() {
                            self.state.workspaces[workspace].tabs[tab]
                                .set_custom_name(record.saved_location.tab_label.clone());
                        }
                        if params.location.is_none()
                            && !record.saved_location.tab_location_id.is_empty()
                        {
                            self.state.workspaces[workspace].tabs[tab].location_id =
                                record.saved_location.tab_location_id.clone();
                        }
                    }
                }
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
                    pane_label: self.state.terminals[&tab.panes[&pane_id].attached_terminal_id]
                        .manual_label
                        .clone(),
                    pane_number: tab
                        .layout
                        .pane_ids()
                        .iter()
                        .position(|id| *id == pane_id)
                        .map(|index| index + 1),
                    workspace_location_id: ws.location_id.clone(),
                    tab_location_id: tab.location_id.clone(),
                    cwd: self.state.terminals[&tab.panes[&pane_id].attached_terminal_id]
                        .cwd
                        .to_string_lossy()
                        .into_owned(),
                };
                let pane_id = restore
                    .then(|| self.focus_conversation_attachment(&target))
                    .flatten();
                match self.agent_inbox.mutate(
                    &record.id,
                    pending.revision,
                    ConversationAction::ResumeLaunched(Box::new(target)),
                    super::inbox::now_ms(),
                ) {
                    Ok(record) if restore => restore_response(id, "pending", &record,
                        Some("Awaiting native session confirmation. Continue in the restored terminal."), pane_id),
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
                        if restore {
                            restore_response(id, "uncertain", &pending, Some("The process started, but its launch target could not be saved. Do not retry until the existing process is reconciled."), pane_id)
                        } else {
                            resume_response(id, "uncertain", &pending, Some("The process started, but its launch target could not be saved. Do not retry until the existing process is reconciled."))
                        }
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

fn restore_response(
    id: String,
    status: &str,
    record: &crate::agent_inbox::ConversationRecord,
    message: Option<&str>,
    pane_id: Option<String>,
) -> String {
    super::api::responses::encode_success(
        id,
        crate::api::schema::ResponseResult::AgentConversationResume {
            result: serde_json::json!({"status": status, "conversation": record, "message": message, "pane_id": pane_id}),
        },
    )
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
    fn restore_uses_unique_human_names_after_stable_location_ids_are_lost() {
        let mut workspace = Workspace::test_new("Saved Space");
        workspace.tabs[0].set_custom_name("Saved tab".into());
        let saved = saved_registry().records().remove(0).saved_location;
        assert_ne!(workspace.location_id, saved.workspace_location_id);
        assert_ne!(workspace.tabs[0].location_id, saved.tab_location_id);
        assert_eq!(
            resolve_restore_destination(&[workspace], &Default::default(), &saved).unwrap(),
            ResumeDestination::Tab {
                workspace: 0,
                tab: 0
            }
        );
    }

    #[test]
    fn restore_reuses_unique_displayed_numeric_tab_name_after_id_migration() {
        let mut saved = saved_registry().records().remove(0).saved_location;
        saved.tab_label = "1".into();
        let workspace = Workspace::test_new("Saved Space");
        assert_eq!(
            resolve_restore_destination(&[workspace], &Default::default(), &saved).unwrap(),
            ResumeDestination::Tab {
                workspace: 0,
                tab: 0
            }
        );
    }

    #[test]
    fn restore_creates_only_the_missing_container_and_refuses_duplicate_names() {
        let saved = saved_registry().records().remove(0).saved_location;
        let workspace = Workspace::test_new("Saved Space");
        assert_eq!(
            resolve_restore_destination(&[workspace], &Default::default(), &saved).unwrap(),
            ResumeDestination::NewTab { workspace: 0 }
        );
        assert_eq!(
            resolve_restore_destination(&[], &Default::default(), &saved).unwrap(),
            ResumeDestination::NewSpace
        );
        let first = Workspace::test_new("Saved Space");
        let second = Workspace::test_new("Saved Space");
        assert!(
            resolve_restore_destination(&[first, second], &Default::default(), &saved).is_err()
        );
        let mut workspace = Workspace::test_new("Saved Space");
        workspace.tabs[0].set_custom_name("Saved tab".into());
        workspace.test_add_tab(Some("Saved tab"));
        assert!(resolve_restore_destination(&[workspace], &Default::default(), &saved).is_err());
    }

    #[test]
    fn restore_prefers_durable_tab_identity_after_move_and_rejects_duplicate_durable_ids() {
        let mut saved = saved_registry().records().remove(0).saved_location;
        let mut workspace = Workspace::test_new("Moved destination");
        workspace.tabs[0].location_id = saved.tab_location_id.clone();
        assert_eq!(
            resolve_restore_destination(&[workspace], &Default::default(), &saved).unwrap(),
            ResumeDestination::Tab {
                workspace: 0,
                tab: 0
            }
        );
        let first = Workspace::test_new("one");
        let mut second = Workspace::test_new("two");
        second.location_id = first.location_id.clone();
        saved.workspace_location_id = first.location_id.clone();
        assert!(
            resolve_restore_destination(&[first, second], &Default::default(), &saved).is_err()
        );
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn restore_launch_focuses_the_new_exact_pane_in_the_existing_tab() {
        let (_, api_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = super::super::App::new(
            &crate::config::Config::default(),
            super::super::AppPolicy::TEST,
            None,
            api_rx,
            crate::api::EventHub::default(),
        );
        app.state
            .workspaces
            .push(Workspace::test_new("Saved Space"));
        app.state.workspaces[0].tabs[0].set_custom_name("Saved tab".into());
        app.state.workspaces.push(Workspace::test_new("other"));
        app.state.ensure_test_terminals();
        app.state.active = Some(1);
        let original = app.state.workspaces[0].tabs[0].root_pane;
        let saved = saved_registry().records().remove(0).saved_location;
        let destination =
            resolve_restore_destination(&app.state.workspaces, &app.state.terminals, &saved)
                .unwrap();
        let plan = crate::agent_resume::AgentResumePlan {
            agent: "codex".into(),
            argv: vec!["/bin/sh".into(), "-c".into(), "sleep 5".into()],
            dedupe_key: "isolated-restore-test".into(),
        };
        let (workspace, tab, pane) = app
            .launch_conversation_resume(destination, std::env::temp_dir(), &plan, true)
            .unwrap();
        assert_eq!((workspace, tab), (0, 0));
        assert_ne!(pane, original);
        assert_eq!(app.state.active, Some(0));
        assert_eq!(app.state.workspaces[0].focused_pane_id(), Some(pane));
        assert_eq!(app.state.workspaces.len(), 2);
        assert_eq!(app.state.workspaces[0].tabs.len(), 1);
        let terminal_id = app.state.workspaces[0].tabs[0].panes[&pane]
            .attached_terminal_id
            .clone();
        let terminal = app.state.terminals.get_mut(&terminal_id).unwrap();
        terminal.set_detected_state(
            Some(crate::detect::Agent::Codex),
            crate::detect::AgentState::Idle,
        );
        terminal
            .set_agent_session_ref(
                "herdr:codex".into(),
                "codex".into(),
                crate::agent_resume::AgentSessionRef::id("existing-live-test"),
                Some(1),
            )
            .unwrap();
        app.sync_agent_conversations();
        let record = app.agent_inbox.records().remove(0);
        let record = app
            .agent_inbox
            .mutate(
                &record.id,
                record.revision,
                crate::agent_inbox::ConversationAction::Settle,
                super::super::inbox::now_ms(),
            )
            .unwrap();
        app.state.active = Some(1);
        let response: serde_json::Value =
            serde_json::from_str(&app.handle_agent_conversation_restore(
                "existing".into(),
                crate::api::schema::AgentConversationRestoreParams {
                    conversation_id: record.id,
                    expected_revision: record.revision,
                    location: None,
                    cwd: None,
                },
            ))
            .unwrap();
        assert_eq!(
            response["result"]["result"]["status"], "confirmed",
            "{response}"
        );
        assert_eq!(
            response["result"]["result"]["pane_id"],
            app.public_pane_id(0, pane).unwrap()
        );
        assert_eq!(app.state.workspaces[0].focused_pane_id(), Some(pane));
        assert_eq!(
            app.state.workspaces[0].tabs[0].panes.len(),
            2,
            "reusing the live native attachment must not launch another pane"
        );
        let mut missing_tab = saved.clone();
        missing_tab.tab_label = "Missing saved tab".into();
        missing_tab.tab_location_id = "deleted-tab".into();
        let destination =
            resolve_restore_destination(&app.state.workspaces, &app.state.terminals, &missing_tab)
                .unwrap();
        assert_eq!(destination, ResumeDestination::NewTab { workspace: 0 });
        let (workspace, tab, pane) = app
            .launch_conversation_resume(destination, std::env::temp_dir(), &plan, true)
            .unwrap();
        assert_eq!((workspace, tab), (0, 1));
        assert_eq!(
            app.state.workspaces.len(),
            2,
            "a missing tab must reuse the existing Space"
        );
        assert_eq!(app.state.workspaces[0].tabs.len(), 2);
        assert_eq!(app.state.workspaces[0].focused_pane_id(), Some(pane));
        for (_, runtime) in app.terminal_runtimes.drain() {
            runtime.shutdown();
        }
    }

    #[test]
    fn restore_pending_target_focuses_without_manufacturing_native_confirmation() {
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
        let record = app.agent_inbox.records().remove(0);
        let record = app
            .agent_inbox
            .mutate(
                &record.id,
                record.revision,
                ConversationAction::SettleAndClose,
                2,
            )
            .unwrap();
        let pending = app
            .agent_inbox
            .mutate(
                &record.id,
                record.revision,
                ConversationAction::BeginResume,
                super::super::inbox::now_ms(),
            )
            .unwrap();
        app.state
            .workspaces
            .push(Workspace::test_new("reserved target"));
        app.state.workspaces.push(Workspace::test_new("other"));
        app.state.ensure_test_terminals();
        app.state.active = Some(1);
        let pane = app.state.workspaces[0].tabs[0].root_pane;
        let mut target = record.saved_location.clone();
        target.terminal_id = app.state.workspaces[0].tabs[0].panes[&pane]
            .attached_terminal_id
            .to_string();
        target.public_pane_id = app.public_pane_id(0, pane).unwrap();
        let pending = app
            .agent_inbox
            .mutate(
                &pending.id,
                pending.revision,
                ConversationAction::ResumeLaunched(Box::new(target.clone())),
                super::super::inbox::now_ms(),
            )
            .unwrap();
        let response: serde_json::Value =
            serde_json::from_str(&app.handle_agent_conversation_restore(
                "pending".into(),
                crate::api::schema::AgentConversationRestoreParams {
                    conversation_id: pending.id.clone(),
                    expected_revision: pending.revision,
                    location: None,
                    cwd: None,
                },
            ))
            .unwrap();
        assert_eq!(response["result"]["result"]["status"], "pending");
        assert_eq!(
            response["result"]["result"]["pane_id"],
            target.public_pane_id
        );
        assert_eq!(app.state.active, Some(0));
        let after = app.agent_inbox.get(&pending.id).unwrap();
        assert_eq!(after.resume_state, ResumeState::Pending);
        assert_eq!(after.session_ref, record.session_ref);
        assert!(after.attachments.is_empty());
        assert!(app
            .state
            .terminals
            .values()
            .all(|terminal| terminal.provider_reported_session().is_none()));
    }

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
                        pane_label: None,
                        pane_number: Some(1),
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
            .launch_conversation_resume(
                ResumeDestination::NewSpace,
                std::env::temp_dir(),
                &plan,
                false,
            )
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
    fn restore_distinguishes_missing_directory_and_accepts_independent_cwd_override() {
        use crate::agent_inbox::{ConversationAction, ConversationObservation, ExecutionState};
        let (_, api_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = super::super::App::new(
            &crate::config::Config::default(),
            super::super::AppPolicy::TEST,
            None,
            api_rx,
            crate::api::EventHub::default(),
        );
        app.agent_inbox = saved_registry();
        let record = app.agent_inbox.records().remove(0);
        let mut attachment = record.saved_location;
        attachment.cwd = "/herdr-does-not-exist/resume".into();
        app.agent_inbox
            .reconcile(
                &[ConversationObservation {
                    provider: record.provider,
                    source: record.source,
                    session_ref: record.session_ref,
                    generation: attachment.terminal_id.clone(),
                    attachment,
                    title: record.title,
                    execution: ExecutionState::Idle,
                    queued_human_work: false,
                    blocking_request: false,
                    failure: false,
                    capability_limits: Vec::new(),
                }],
                2,
            )
            .unwrap();
        let record = app.agent_inbox.records().remove(0);
        let record = app
            .agent_inbox
            .mutate(
                &record.id,
                record.revision,
                ConversationAction::SettleAndClose,
                3,
            )
            .unwrap();
        let params = crate::api::schema::AgentConversationRestoreParams {
            conversation_id: record.id.clone(),
            expected_revision: record.revision,
            location: None,
            cwd: None,
        };
        let response: serde_json::Value = serde_json::from_str(
            &app.handle_agent_conversation_restore("directory".into(), params.clone()),
        )
        .unwrap();
        assert_eq!(response["result"]["result"]["status"], "cwd_required");
        assert!(app.state.workspaces.is_empty());
        assert_eq!(app.terminal_runtimes.len(), 0);
        let legacy: serde_json::Value =
            serde_json::from_str(&app.handle_agent_conversation_resume(
                "legacy".into(),
                crate::api::schema::AgentConversationResumeParams {
                    conversation_id: record.id,
                    expected_revision: record.revision,
                    location: None,
                },
            ))
            .unwrap();
        assert_eq!(
            legacy["result"]["result"]["status"], "location_required",
            "legacy Resume keeps its published result semantics"
        );
        app.state
            .workspaces
            .push(Workspace::test_new("Saved Space"));
        app.state
            .workspaces
            .push(Workspace::test_new("Saved Space"));
        let response: serde_json::Value =
            serde_json::from_str(&app.handle_agent_conversation_restore(
                "destination".into(),
                crate::api::schema::AgentConversationRestoreParams {
                    cwd: Some(std::env::temp_dir().to_string_lossy().into_owned()),
                    ..params
                },
            ))
            .unwrap();
        assert_eq!(
            response["result"]["result"]["status"], "location_required",
            "valid cwd override proceeds to destination resolution"
        );
        assert_eq!(app.terminal_runtimes.len(), 0);
    }

    #[test]
    fn invalid_cwd_is_rejected_without_directory_creation() {
        assert!(validated_resume_cwd(".").is_err());
        assert!(validated_resume_cwd("/herdr-does-not-exist/resume").is_err());
        assert!(validated_resume_cwd(&std::env::temp_dir().to_string_lossy()).is_ok());
    }
}
