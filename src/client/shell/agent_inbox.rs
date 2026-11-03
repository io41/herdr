//! Client presentation of endpoint-owned conversations. Reading never changes disposition.
use super::super::endpoint::ClientAgentConversationsProjection;
use super::*;
use crate::agent_inbox::{
    ConversationRecord, ConversationSection, Disposition, ExecutionState, ResumeState,
};

#[derive(Clone, Debug)]
pub(super) struct CachedConversations {
    pub(super) generation: Option<u64>,
    pub(super) projection: ClientAgentConversationsProjection,
}

#[derive(Clone)]
pub(super) struct ConversationRow {
    pub(super) endpoint_id: ClientEndpointId,
    pub(super) id: String,
    pub(super) label: String,
    pub(super) detail: String,
    pub(super) status: String,
    pub(super) provider: String,
    pub(super) origin: Option<String>,
    pub(super) section: usize,
    pub(super) order: u64,
    pub(super) unavailable: bool,
    pub(super) attention: u64,
}

pub(super) fn cache_current(endpoint: &ClientShellEndpoint) -> bool {
    endpoint.conversations.as_ref().is_some_and(|cache| {
        endpoint.snapshot.as_deref().is_some_and(|snapshot| {
            endpoint.conversations_supported
                && snapshot.boot_id == cache.projection.boot_id
                && endpoint.snapshot_generation == cache.generation
        })
    })
}

pub(super) fn record_status(record: &ConversationRecord, unavailable: bool) -> &'static str {
    if unavailable {
        "Unavailable"
    } else if record.blocking_request {
        "Needs input"
    } else if record.failure {
        "Failed"
    } else if record.resume_state != ResumeState::None {
        "Confirm session"
    } else {
        match record.execution {
            ExecutionState::Working => "Working",
            ExecutionState::Idle => {
                if record.completed_at_ms.is_some() {
                    "Done"
                } else {
                    "Idle"
                }
            }
            ExecutionState::Unknown => "Unknown",
            ExecutionState::Unavailable => "Unavailable",
            ExecutionState::Closed => "Closed",
        }
    }
}

pub(super) fn conversation_title(record: &ConversationRecord) -> String {
    if !record.title.trim().is_empty()
        && !record
            .title
            .trim()
            .eq_ignore_ascii_case("Agent conversation")
    {
        return record.title.clone();
    }
    let location = record.attachments.first().unwrap_or(&record.saved_location);
    location
        .pane_label
        .clone()
        .filter(|label| !label.trim().is_empty())
        .unwrap_or_else(|| {
            if !location.tab_label.trim().is_empty() && location.tab_label.parse::<usize>().is_err()
            {
                location.tab_label.clone()
            } else {
                format!(
                    "{} / Pane {}",
                    location_label(&location.workspace_label, &location.workspace_id),
                    location.pane_number.unwrap_or(location.pane_id as usize)
                )
            }
        })
}

pub(super) fn conversation_breadcrumb(record: &ConversationRecord) -> String {
    let location = record.attachments.first().unwrap_or(&record.saved_location);
    let pane = location
        .pane_label
        .clone()
        .filter(|label| !label.trim().is_empty())
        .or_else(|| location.pane_number.map(|number| format!("Pane {number}")))
        .unwrap_or_else(|| format!("Pane {}", location.pane_id));
    format!(
        "{}{} / {} / {}",
        if record.attachments.is_empty() {
            "Last known: "
        } else {
            ""
        },
        location_label(&location.workspace_label, &location.workspace_id),
        location_label(&location.tab_label, &location.tab_id),
        pane
    )
}

pub(super) fn row_headline(row: &ConversationRow, width: usize, unread: bool) -> String {
    let status_width = usize::from(super::render::display_width(&row.status));
    let title_width = width.saturating_sub(status_width + 3);
    let title = crate::ui::truncate_end(
        &format!("{}{}", if unread { "● " } else { "" }, row.label),
        title_width,
    );
    if title_width == 0 {
        row.status.clone()
    } else {
        format!("{title} · {}", row.status)
    }
}

pub(super) fn row_detail(row: &ConversationRow, width: usize) -> String {
    let location = row.detail.as_str();
    let provider = row.provider.as_str();
    let origin = row.origin.as_deref();
    let full = format!(
        "{provider} · {location}{}",
        origin
            .map(|origin| format!(" · {origin}"))
            .unwrap_or_default()
    );
    if usize::from(super::render::display_width(&full)) <= width {
        return full;
    }
    let (prefix, location) = location
        .strip_prefix("Last known: ")
        .map_or(("", location), |location| ("Last: ", location));
    let parts: Vec<_> = location.split(" / ").collect();
    let mut remaining = width.saturating_sub(
        usize::from(super::render::display_width(provider))
            + 3
            + usize::from(super::render::display_width(prefix))
            + parts.len().saturating_sub(1),
    );
    let compact = parts
        .iter()
        .enumerate()
        .map(|(index, part)| {
            let text = crate::ui::truncate_end(
                &part.replace("Pane ", "#"),
                remaining / (parts.len() - index),
            );
            remaining = remaining.saturating_sub(usize::from(super::render::display_width(&text)));
            text
        })
        .collect::<Vec<_>>()
        .join("/");
    format!("{provider} · {prefix}{compact}")
}

pub(super) fn conversation_rows(endpoints: &[ClientShellEndpoint]) -> Vec<ConversationRow> {
    let mut rows = Vec::new();
    for endpoint in endpoints {
        let unavailable = endpoint.status != ClientEndpointStatus::Online;
        if let Some(cache) = &endpoint.conversations {
            let unavailable = unavailable
                || !cache_current(endpoint)
                || cache.projection.recovery_error.is_some();
            let reserved = reserved_resume_terminals(&cache.projection.conversations);
            for record in &cache.projection.conversations {
                if reserved_provisional(record, &reserved) {
                    continue;
                }
                let section = if unavailable
                    || record.blocking_request
                    || record.failure
                    || record.resume_state != ResumeState::None
                    || matches!(
                        record.execution,
                        ExecutionState::Unknown | ExecutionState::Unavailable
                    ) {
                    0
                } else {
                    match record.section {
                        ConversationSection::Inbox => 0,
                        ConversationSection::Working => 1,
                        ConversationSection::Settled => 2,
                    }
                };
                rows.push(ConversationRow {
                    endpoint_id: endpoint.endpoint_id.clone(),
                    id: record.id.clone(),
                    label: conversation_title(record),
                    detail: conversation_breadcrumb(record),
                    status: record_status(record, unavailable).into(),
                    provider: record.provider.clone(),
                    origin: (endpoints.len() > 1).then(|| endpoint.label.clone()),
                    section,
                    order: match section {
                        1 => record.last_human_submission_at_ms.unwrap_or(0),
                        2 => record.settled_at_ms.unwrap_or(0),
                        _ => record.attention_returned_at_ms,
                    },
                    unavailable,
                    attention: record.attention_returned_at_ms,
                });
            }
        }
        if let Some(snapshot) = endpoint.snapshot.as_deref() {
            let represented = endpoint
                .conversations
                .as_ref()
                .map(|cache| {
                    cache
                        .projection
                        .conversations
                        .iter()
                        .flat_map(|record| {
                            record
                                .attachments
                                .iter()
                                .map(|attachment| attachment.public_pane_id.as_str())
                        })
                        .collect::<HashSet<_>>()
                })
                .unwrap_or_default();
            for agent in snapshot
                .agents
                .iter()
                .filter(|agent| !represented.contains(agent.pane_id.as_str()))
            {
                let workspace = snapshot
                    .workspaces
                    .iter()
                    .find(|workspace| workspace.workspace_id == agent.workspace_id);
                let tab = snapshot.tabs.iter().find(|tab| tab.tab_id == agent.tab_id);
                let pane = snapshot
                    .panes
                    .iter()
                    .find(|pane| pane.pane_id == agent.pane_id);
                let title = agent
                    .name
                    .as_ref()
                    .or(agent.title.as_ref())
                    .filter(|title| {
                        !title.trim().is_empty()
                            && !title.trim().eq_ignore_ascii_case("Agent conversation")
                    })
                    .or_else(|| pane.and_then(|pane| pane.label.as_ref()))
                    .cloned()
                    .or_else(|| tab.map(|tab| tab.label.clone()))
                    .or_else(|| agent.terminal_title_stripped.clone())
                    .unwrap_or_else(|| agent.pane_id.clone());
                let breadcrumb = format!(
                    "{} / {} / {}",
                    workspace.map_or(agent.workspace_id.as_str(), |workspace| workspace
                        .label
                        .as_str()),
                    tab.map_or(agent.tab_id.as_str(), |tab| tab.label.as_str()),
                    pane.and_then(|pane| pane.label.as_deref())
                        .unwrap_or(agent.pane_id.as_str())
                );
                rows.push(ConversationRow {
                    endpoint_id: endpoint.endpoint_id.clone(),
                    id: format!("pane:{}", agent.pane_id),
                    label: title,
                    detail: breadcrumb,
                    status: "Unavailable".into(),
                    provider: agent
                        .display_agent
                        .clone()
                        .unwrap_or_else(|| "agent".into()),
                    origin: (endpoints.len() > 1).then(|| endpoint.label.clone()),
                    section: 0,
                    order: 0,
                    unavailable,
                    attention: 0,
                });
            }
        }
    }
    rows.sort_by(|left, right| {
        left.section
            .cmp(&right.section)
            .then_with(|| right.order.cmp(&left.order))
            .then_with(|| left.endpoint_id.cmp(&right.endpoint_id))
            .then_with(|| left.id.cmp(&right.id))
    });
    rows
}

impl ClientShellState {
    pub(crate) fn set_endpoint_agent_conversations_supported(
        &mut self,
        endpoint_id: &ClientEndpointId,
        supported: bool,
    ) {
        if let Some(endpoint) = self
            .endpoints
            .iter_mut()
            .find(|endpoint| &endpoint.endpoint_id == endpoint_id)
        {
            endpoint.conversations_supported = supported;
        }
    }

    pub(crate) fn set_endpoint_agent_conversations(
        &mut self,
        endpoint_id: &ClientEndpointId,
        generation: Option<u64>,
        projection: ClientAgentConversationsProjection,
    ) -> bool {
        let old = self.visible_conversations();
        let Some(endpoint) = self
            .endpoints
            .iter_mut()
            .find(|endpoint| &endpoint.endpoint_id == endpoint_id)
        else {
            return false;
        };
        if endpoint.snapshot_generation.is_some() && endpoint.snapshot_generation != generation {
            return false;
        }
        if endpoint
            .snapshot
            .as_deref()
            .is_some_and(|snapshot| snapshot.boot_id != projection.boot_id)
        {
            return false;
        }
        if endpoint.conversations.as_ref().is_some_and(|cache| {
            cache.generation == generation
                && cache.projection.boot_id == projection.boot_id
                && cache.projection.revision > projection.revision
        }) {
            return false;
        }
        if endpoint.conversations.as_ref().is_some_and(|cache| {
            cache.generation == generation
                && cache.projection.boot_id == projection.boot_id
                && cache.projection.revision == projection.revision
                && cache.projection.conversations.len() >= projection.conversations.len()
                && cache.projection.recovery_error == projection.recovery_error
        }) {
            return false;
        }
        let failures = endpoint
            .conversations
            .as_ref()
            .map(|cache| {
                projection
                    .conversations
                    .iter()
                    .filter_map(|record| {
                        let error = record.resume_error.as_ref()?;
                        let previous = cache
                            .projection
                            .conversations
                            .iter()
                            .find(|previous| previous.id == record.id)?;
                        (previous.resume_state != ResumeState::None
                            && previous.resume_error.as_ref() != Some(error))
                        .then(|| {
                            (
                                record.id.clone(),
                                format!("{}: {}. {error}", endpoint.label, record.title),
                            )
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        endpoint.conversations = Some(CachedConversations {
            generation,
            projection,
        });
        self.queue_conversation_page(endpoint_id);
        self.reconcile_conversation_selection(&old);
        if self.agent_inbox {
            for (id, message) in failures {
                self.push_endpoint_notice(
                    ClientEndpointNoticeKind::Rejected,
                    format!("conversation_resume_failed:{id}"),
                    "Resume failed",
                    message,
                );
            }
        }
        true
    }

    pub(super) fn visible_conversations(&self) -> Vec<ConversationRow> {
        conversation_rows(&self.endpoints)
            .into_iter()
            .filter(|row| self.agent_inbox_expanded[row.section])
            .collect()
    }
}

pub(super) fn render_conversation_panel(
    buffer: &mut Buffer,
    endpoints: &[ClientShellEndpoint],
    enabled: bool,
    expanded: [bool; 3],
    selected: Option<&(ClientEndpointId, String)>,
    read: &HashMap<String, u64>,
    collapsed: bool,
    config: &ClientShellConfig,
    scroll: &mut usize,
    hits: &mut ShellHitMap,
) {
    let body = hits.agent_body;
    if body.is_empty() || collapsed {
        return;
    }
    let palette = &config.palette;
    let header = Rect::new(body.x, body.y.saturating_sub(2), body.width, 1);
    hits.agent_inbox_toggle = header;
    hits.agent_sort_toggle = Rect::default();
    for x in header.x..header.right() {
        buffer[(x, header.y)]
            .set_symbol(" ")
            .set_style(Style::default().bg(palette.panel_bg));
    }
    let title = if enabled {
        " Inbox ▾"
    } else {
        match config.agent_panel_sort {
            crate::config::AgentPanelSortConfig::Spaces => " All agents by space ▾",
            crate::config::AgentPanelSortConfig::Priority => " All agents by attention ▾",
        }
    };
    super::render::put_text(
        buffer,
        header.x,
        header.y,
        header.width,
        title,
        Style::default().fg(palette.accent),
    );
    if !enabled {
        return;
    }
    hits.agents.clear();
    hits.endpoint_agents.clear();
    hits.agent_scrollbar = Rect::default();
    hits.agent_scroll_metrics = None;
    for y in body.y..body.bottom() {
        for x in body.x..body.right() {
            buffer[(x, y)]
                .set_symbol(" ")
                .set_style(Style::default().bg(palette.panel_bg));
        }
    }
    let rows = conversation_rows(endpoints);
    let mut lines = Vec::new();
    for endpoint in endpoints {
        if let Some(cache) = &endpoint.conversations {
            if cache.projection.next_offset.is_some() {
                lines.push((
                    None,
                    format!(
                        " {}: loading {}/{} conversations",
                        endpoint.label,
                        cache.projection.conversations.len(),
                        cache.projection.total_count
                    ),
                    None,
                ));
            }
            if let Some(error) = &cache.projection.recovery_error {
                lines.push((None, format!(" {}: {error}", endpoint.label), None));
            }
        }
    }
    for (section, title) in ["Inbox", "Working", "Settled"].into_iter().enumerate() {
        let count = rows.iter().filter(|row| row.section == section).count();
        lines.push((
            Some(section),
            format!(
                " {} {title} ({count})",
                if expanded[section] { "▾" } else { "▸" }
            ),
            None,
        ));
        if expanded[section] {
            for row in rows.iter().filter(|row| row.section == section) {
                lines.push((
                    None,
                    row_headline(
                        row,
                        body.width as usize,
                        row.attention
                            > read
                                .get(&read_key(&row.endpoint_id, &row.id))
                                .copied()
                                .unwrap_or(0),
                    ),
                    Some(row),
                ));
                lines.push((None, row_detail(row, body.width as usize), Some(row)));
            }
        }
    }
    *scroll = (*scroll).min(lines.len().saturating_sub(body.height as usize));
    hits.agent_max_scroll = lines.len().saturating_sub(body.height as usize);
    for (index, (section, label, row)) in lines
        .into_iter()
        .skip(*scroll)
        .take(body.height as usize)
        .enumerate()
    {
        let rect = Rect::new(body.x, body.y + index as u16, body.width, 1);
        let selected = row.is_some_and(|row| {
            selected.is_some_and(|selected| selected.0 == row.endpoint_id && selected.1 == row.id)
        });
        let style = Style::default()
            .fg(if row.is_some_and(|row| row.unavailable) {
                palette.overlay0
            } else {
                palette.text
            })
            .bg(if selected {
                palette.active_row_bg
            } else {
                palette.panel_bg
            });
        buffer.set_style(rect, style);
        super::render::put_text(buffer, rect.x, rect.y, rect.width, &label, style);
        if let Some(section) = section {
            hits.agent_inbox_sections.push((rect, section));
        }
        if let Some(row) = row {
            hits.conversations
                .push((rect, row.endpoint_id.clone(), row.id.clone()));
        }
    }
}

impl ClientShellState {
    pub(super) fn open_agent_view_selector(&mut self, outcome: &mut ClientShellInput) {
        let selected = if self.agent_inbox {
            0
        } else if self.config.agent_panel_sort == crate::config::AgentPanelSortConfig::Spaces {
            1
        } else {
            2
        };
        self.overlay = Some(ClientShellOverlay::ContextMenu(ClientContextMenuOverlay {
            target: ClientContextMenuTarget::AgentViews {
                selected,
                expanded: self.agent_inbox_expanded,
            },
            x: self.hits.agent_body.x,
            y: self.hits.agent_body.y.saturating_sub(2),
            highlighted: selected,
        }));
        outcome.repaint = true;
    }

    pub(super) fn open_selected_conversation_actions(&mut self, outcome: &mut ClientShellInput) {
        let focused = self
            .endpoints
            .iter()
            .find(|endpoint| endpoint.endpoint_id == self.active_endpoint_id)
            .and_then(|endpoint| {
                let pane = endpoint.snapshot.as_deref()?.focused_pane_id.as_ref()?;
                let record = endpoint
                    .conversations
                    .as_ref()?
                    .projection
                    .conversations
                    .iter()
                    .find(|record| {
                        record
                            .attachments
                            .iter()
                            .any(|attachment| &attachment.public_pane_id == pane)
                            || record
                                .resume_target
                                .as_ref()
                                .is_some_and(|target| &target.public_pane_id == pane)
                    })?;
                Some((endpoint.endpoint_id.clone(), record.id.clone()))
            });
        if let Some((endpoint_id, id)) = focused.or_else(|| self.agent_inbox_selected.clone()) {
            self.open_conversation(endpoint_id, id, true, outcome);
        } else {
            self.set_endpoint_error("Focus or select a conversation to open its actions");
            outcome.repaint = true;
        }
    }

    pub(super) fn handle_conversation_navigation(
        &mut self,
        action: crate::input::KeybindAction,
        outcome: &mut ClientShellInput,
    ) -> bool {
        use crate::input::KeybindAction;
        if !self.agent_inbox
            || !matches!(
                action,
                KeybindAction::FocusAgent(_)
                    | KeybindAction::NextAgent
                    | KeybindAction::PreviousAgent
            )
        {
            return false;
        }
        let rows = self.visible_conversations();
        if rows.is_empty() {
            return true;
        }
        let current = rows.iter().position(|row| {
            self.agent_inbox_selected
                .as_ref()
                .is_some_and(|selected| selected.0 == row.endpoint_id && selected.1 == row.id)
        });
        let index = match action {
            KeybindAction::FocusAgent(index) if index < rows.len() => index,
            KeybindAction::FocusAgent(_) => return true,
            KeybindAction::PreviousAgent => current.map_or(rows.len() - 1, |index| {
                (index + rows.len() - 1) % rows.len()
            }),
            _ => current.map_or(0, |index| (index + 1) % rows.len()),
        };
        let row = &rows[index];
        self.open_conversation(row.endpoint_id.clone(), row.id.clone(), false, outcome);
        true
    }

    pub(super) fn open_conversation(
        &mut self,
        endpoint_id: ClientEndpointId,
        id: String,
        details: bool,
        outcome: &mut ClientShellInput,
    ) {
        self.agent_inbox_selected = Some((endpoint_id.clone(), id.clone()));
        self.reveal_selected_conversation();
        self.mark_conversation_read(&endpoint_id, &id, outcome);
        let Some(endpoint) = self
            .endpoints
            .iter()
            .find(|endpoint| endpoint.endpoint_id == endpoint_id)
        else {
            return;
        };
        let record = endpoint.conversations.as_ref().and_then(|cache| {
            cache
                .projection
                .conversations
                .iter()
                .find(|record| record.id == id)
        });
        if let Some(record) = record {
            let available =
                endpoint.status == ClientEndpointStatus::Online && cache_current(endpoint);
            if !details
                && available
                && endpoint
                    .methods
                    .as_ref()
                    .is_some_and(|methods| methods.contains("agent.conversation.open"))
            {
                let target = if record.attachments.len() == 1 {
                    record.attachments.first()
                } else if record.attachments.is_empty() && record.resume_state != ResumeState::None
                {
                    record.resume_target.as_ref()
                } else {
                    None
                };
                if let Some(target) = target.filter(|target| !target.terminal_id.is_empty()) {
                    let method = crate::api::schema::Method::AgentConversationOpen(
                        crate::api::schema::AgentConversationOpenParams {
                            conversation_id: record.id.clone(),
                            expected_revision: record.revision,
                            attachment_terminal_id: Some(target.terminal_id.clone()),
                        },
                    );
                    self.push_conversation_method(endpoint_id, method, outcome);
                    return;
                }
            }
            self.overlay = Some(ClientShellOverlay::ContextMenu(ClientContextMenuOverlay {
                target: ClientContextMenuTarget::Conversation {
                    endpoint_id,
                    record: Box::new(record.clone()),
                    available,
                    methods: endpoint.methods.clone().unwrap_or_default(),
                },
                x: self.hits.agent_body.right(),
                y: self.hits.agent_body.y,
                highlighted: 0,
            }));
        } else if let Some(pane) = id.strip_prefix("pane:") {
            self.focus_or_activate(
                endpoint_id,
                ClientEndpointFocusTarget::Pane(pane.into()),
                outcome,
            );
        }
        outcome.repaint = true;
    }
}

impl ClientShellState {
    pub(super) fn handle_conversation_click(
        &mut self,
        point: (u16, u16),
        details: bool,
        outcome: &mut ClientShellInput,
    ) -> bool {
        if !self.config.mouse_capture {
            return false;
        }
        if !details && super::contains(self.hits.agent_inbox_toggle, point) {
            self.open_agent_view_selector(outcome);
            return true;
        }
        if !self.agent_inbox {
            return false;
        }
        if !details {
            if let Some((_, section)) = self
                .hits
                .agent_inbox_sections
                .iter()
                .find(|(rect, _)| super::contains(*rect, point))
            {
                let section = *section;
                let old = self.visible_conversations();
                self.agent_inbox_expanded[section] = !self.agent_inbox_expanded[section];
                self.reconcile_conversation_selection(&old);
                self.persist_chrome_preferences(outcome);
                outcome.repaint = true;
                return true;
            }
        }
        if let Some((_, endpoint_id, id)) = self
            .hits
            .conversations
            .iter()
            .find(|(rect, _, _)| super::contains(*rect, point))
            .cloned()
        {
            self.open_conversation(endpoint_id, id, details, outcome);
            return true;
        }
        false
    }

    pub(super) fn activate_conversation_action(
        &mut self,
        endpoint_id: ClientEndpointId,
        record: ConversationRecord,
        action: ClientContextMenuAction,
        outcome: &mut ClientShellInput,
    ) {
        use crate::api::schema::{
            AgentConversationMutationParams, AgentConversationRestoreParams, Method,
        };
        let method = match action {
            ClientContextMenuAction::ConversationOpen(index) => {
                let Some(attachment) = record.attachments.get(index) else {
                    return;
                };
                Method::AgentConversationOpen(crate::api::schema::AgentConversationOpenParams {
                    conversation_id: record.id,
                    expected_revision: record.revision,
                    attachment_terminal_id: Some(attachment.terminal_id.clone()),
                })
            }
            ClientContextMenuAction::ConversationOpenResumeTarget => {
                let Some(target) = record.resume_target.as_ref().filter(|target| {
                    record.resume_state != ResumeState::None && !target.terminal_id.is_empty()
                }) else {
                    return;
                };
                Method::AgentConversationOpen(crate::api::schema::AgentConversationOpenParams {
                    conversation_id: record.id,
                    expected_revision: record.revision,
                    attachment_terminal_id: Some(target.terminal_id.clone()),
                })
            }
            ClientContextMenuAction::ConversationSettle => {
                Method::AgentConversationSettleAndClose(AgentConversationMutationParams {
                    conversation_id: record.id,
                    expected_revision: record.revision,
                })
            }
            ClientContextMenuAction::ConversationChooseLocation => {
                self.overlay = Some(ClientShellOverlay::Rename(ClientRenameOverlay {
                    title: "restore working directory",
                    input: TextEditor::new(&record.saved_location.cwd, false),
                    target: ClientRenameTarget::ConversationCwd {
                        endpoint_id,
                        record: Box::new(record),
                    },
                }));
                outcome.repaint = true;
                return;
            }
            ClientContextMenuAction::ConversationResume => {
                Method::AgentConversationRestore(AgentConversationRestoreParams {
                    conversation_id: record.id,
                    expected_revision: record.revision,
                    cwd: None,
                    location: None,
                })
            }
            _ => return,
        };
        self.push_conversation_method(endpoint_id, method, outcome);
    }

    pub(super) fn choose_conversation_location(
        &mut self,
        endpoint_id: ClientEndpointId,
        record: Box<ConversationRecord>,
        cwd: String,
        outcome: &mut ClientShellInput,
    ) {
        if cwd.is_empty() {
            self.set_endpoint_error("Enter a working directory to restore this conversation");
            outcome.repaint = true;
            return;
        }
        self.push_conversation_method(
            endpoint_id,
            crate::api::schema::Method::AgentConversationRestore(
                crate::api::schema::AgentConversationRestoreParams {
                    conversation_id: record.id,
                    expected_revision: record.revision,
                    location: None,
                    cwd: Some(cwd),
                },
            ),
            outcome,
        );
    }

    fn show_conversation_locations(
        &mut self,
        endpoint_id: ClientEndpointId,
        record: Box<ConversationRecord>,
        cwd: String,
        outcome: &mut ClientShellInput,
    ) {
        if cwd.is_empty() {
            self.set_endpoint_error("Enter a working directory before choosing a location");
            outcome.repaint = true;
            return;
        }
        let locations = self
            .endpoints
            .iter()
            .find(|endpoint| endpoint.endpoint_id == endpoint_id)
            .and_then(|endpoint| endpoint.snapshot.as_deref())
            .map(|snapshot| {
                snapshot
                    .tabs
                    .iter()
                    .map(|tab| {
                        let workspace = snapshot
                            .workspaces
                            .iter()
                            .find(|workspace| workspace.workspace_id == tab.workspace_id);
                        (
                            tab.workspace_id.clone(),
                            tab.tab_id.clone(),
                            format!(
                                "{} / {}",
                                workspace.map_or(tab.workspace_id.as_str(), |workspace| workspace
                                    .label
                                    .as_str()),
                                tab.label
                            ),
                        )
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        self.overlay = Some(ClientShellOverlay::ContextMenu(ClientContextMenuOverlay {
            target: ClientContextMenuTarget::ConversationLocations {
                endpoint_id,
                record,
                cwd,
                locations,
            },
            x: self.hits.agent_body.right(),
            y: self.hits.agent_body.y,
            highlighted: 0,
        }));
        outcome.repaint = true;
    }

    pub(super) fn push_conversation_method(
        &mut self,
        endpoint_id: ClientEndpointId,
        method: crate::api::schema::Method,
        outcome: &mut ClientShellInput,
    ) {
        let name = crate::api::api_method_name(&method).to_owned();
        let Some(endpoint) = self
            .endpoints
            .iter()
            .find(|endpoint| endpoint.endpoint_id == endpoint_id)
        else {
            return;
        };
        if endpoint.status != ClientEndpointStatus::Online || !cache_current(endpoint) {
            outcome.repaint |= self.receive_endpoint_unavailable(format!(
                "{} conversation data is unavailable. Reconnect before retrying.",
                endpoint.label
            ));
            return;
        }
        if !endpoint
            .methods
            .as_ref()
            .is_some_and(|methods| methods.contains(&name))
        {
            outcome.repaint |= self.push_endpoint_notice(
                ClientEndpointNoticeKind::Unsupported,
                name.clone(),
                "Action unavailable",
                format!("This endpoint does not support {name}."),
            );
            return;
        }
        let Some(snapshot) = endpoint.snapshot.as_deref() else {
            return;
        };
        let boot_id = snapshot.boot_id.clone();
        let request_id = format!("client-shell:{}", self.next_request_id);
        self.next_request_id = self.next_request_id.saturating_add(1);
        self.pending_requests.insert(
            request_id.clone(),
            PendingEndpointRequest {
                boot_id: boot_id.clone(),
                method_name: name,
                confirmation_workspace_id: None,
                kind: PendingEndpointKind::Conversation {
                    endpoint_id: endpoint_id.clone(),
                },
            },
        );
        outcome.actions.push(ClientShellAction::Endpoint {
            endpoint_id,
            boot_id,
            request: Box::new(crate::api::schema::Request {
                id: request_id,
                method,
            }),
        });
    }

    pub(super) fn complete_conversation_method(
        &mut self,
        endpoint_id: ClientEndpointId,
        boot_id: &str,
        result: Result<crate::api::schema::ResponseResult, ClientShellEndpointError>,
    ) -> (bool, Vec<ClientShellAction>) {
        let Some(endpoint) = self
            .endpoints
            .iter_mut()
            .find(|endpoint| endpoint.endpoint_id == endpoint_id)
        else {
            return (false, Vec::new());
        };
        if endpoint
            .snapshot
            .as_deref()
            .is_none_or(|snapshot| snapshot.boot_id != boot_id)
        {
            return (false, Vec::new());
        }
        match result {
            Ok(crate::api::schema::ResponseResult::AgentConversationOpened { .. }) => {
                if self.active_endpoint_id != endpoint_id {
                    return (
                        true,
                        vec![ClientShellAction::ActivateEndpoint {
                            endpoint_id,
                            target: None,
                        }],
                    );
                }
            }
            Ok(crate::api::schema::ResponseResult::AgentConversation { conversation }) => {
                if let Ok(record) = serde_json::from_value::<ConversationRecord>(conversation) {
                    if crate::agent_inbox::validate_record(&record).is_err() {
                        self.set_endpoint_error("Endpoint returned an invalid conversation record");
                        return (true, Vec::new());
                    }
                    if let Some(cache) = endpoint.conversations.as_mut() {
                        if let Some(previous) = cache
                            .projection
                            .conversations
                            .iter_mut()
                            .find(|previous| previous.id == record.id)
                        {
                            if previous.revision <= record.revision {
                                *previous = record;
                            }
                        }
                    }
                }
            }
            Ok(crate::api::schema::ResponseResult::AgentConversationResume { result }) => {
                let record = result
                    .get("conversation")
                    .cloned()
                    .and_then(|value| serde_json::from_value::<ConversationRecord>(value).ok())
                    .filter(|record| crate::agent_inbox::validate_record(record).is_ok());
                if let (Some(cache), Some(record)) =
                    (endpoint.conversations.as_mut(), record.as_ref())
                {
                    if let Some(previous) = cache
                        .projection
                        .conversations
                        .iter_mut()
                        .find(|previous| previous.id == record.id)
                    {
                        if previous.revision <= record.revision {
                            *previous = record.clone();
                        }
                    }
                }
                if let Some(message) = result.get("message").and_then(|value| value.as_str()) {
                    self.push_endpoint_notice(
                        ClientEndpointNoticeKind::Rejected,
                        "conversation_resume_result",
                        "Restore conversation",
                        message,
                    );
                }
                if result
                    .get("pane_id")
                    .and_then(|value| value.as_str())
                    .is_some()
                    && self.active_endpoint_id != endpoint_id
                {
                    return (
                        true,
                        vec![ClientShellAction::ActivateEndpoint {
                            endpoint_id,
                            target: None,
                        }],
                    );
                }
                if result.get("status").and_then(|value| value.as_str()) == Some("cwd_required") {
                    if let Some(record) = record.clone() {
                        let mut outcome = ClientShellInput::default();
                        self.activate_conversation_action(
                            endpoint_id.clone(),
                            record,
                            ClientContextMenuAction::ConversationChooseLocation,
                            &mut outcome,
                        );
                    }
                }
                if result.get("status").and_then(|value| value.as_str())
                    == Some("location_required")
                {
                    if let Some(record) = record {
                        let mut outcome = ClientShellInput::default();
                        self.show_conversation_locations(
                            endpoint_id,
                            Box::new(record.clone()),
                            record.saved_location.cwd,
                            &mut outcome,
                        );
                    }
                }
            }
            Ok(_) => {}
            Err(error) => {
                self.push_endpoint_notice(
                    ClientEndpointNoticeKind::Rejected,
                    format!("conversation:{:?}", endpoint_id),
                    "Conversation action rejected",
                    error.message,
                );
            }
        }
        (true, Vec::new())
    }
}

pub(super) fn conversation_menu_items(
    record: &ConversationRecord,
    available: bool,
    methods: &HashSet<String>,
) -> Vec<ClientContextMenuItem> {
    use ClientContextMenuAction as Action;
    let mut items = Vec::new();
    for (index, attachment) in record
        .attachments
        .iter()
        .enumerate()
        .filter(|(_, attachment)| !attachment.terminal_id.is_empty())
    {
        items.push(ClientContextMenuItem {
            label: if record.attachments.len() == 1 {
                "Open  Enter".into()
            } else {
                format!(
                    "Open {} / {}",
                    location_label(&attachment.tab_label, &attachment.tab_id),
                    attachment.pane_label.clone().unwrap_or_else(|| format!(
                        "Pane {}",
                        attachment
                            .pane_number
                            .unwrap_or(attachment.pane_id as usize)
                    ))
                )
            },
            action: if available && methods.contains("agent.conversation.open") {
                Action::ConversationOpen(index)
            } else {
                Action::Disabled
            },
        });
    }
    if record.resume_state != ResumeState::None
        && record
            .resume_target
            .as_ref()
            .is_some_and(|target| !target.terminal_id.is_empty())
    {
        items.push(ClientContextMenuItem {
            label: "Open restored session".into(),
            action: if available {
                Action::ConversationOpenResumeTarget
            } else {
                Action::Disabled
            },
        });
    }
    if record.disposition == Disposition::Settled || record.attachments.is_empty() {
        let reason = if !available {
            Some("endpoint unavailable")
        } else if !methods.contains("agent.conversation.restore") {
            Some("update endpoint")
        } else if record.session_ref.is_none() {
            Some("session identity unavailable")
        } else if record.resume_state != ResumeState::None {
            Some("awaiting session confirmation")
        } else {
            None
        };
        items.push(ClientContextMenuItem {
            label: match reason {
                Some(reason) => format!("Restore  r · {reason}"),
                None => "Restore  r".into(),
            },
            action: if reason.is_none() {
                Action::ConversationResume
            } else {
                Action::Disabled
            },
        });
    } else {
        let reason = if !available {
            Some("endpoint unavailable")
        } else if !methods.contains("agent.conversation.settle_and_close") {
            Some("update endpoint")
        } else if record.session_ref.is_none() {
            Some("session identity unavailable")
        } else if record.resume_state != ResumeState::None {
            Some("awaiting session confirmation")
        } else if record.execution == ExecutionState::Working || record.queued_human_work {
            Some("working")
        } else if record.blocking_request {
            Some("needs input")
        } else if record.failure {
            Some("resolve failure first")
        } else if !matches!(
            record.execution,
            ExecutionState::Idle | ExecutionState::Closed
        ) {
            Some("state unknown")
        } else {
            None
        };
        items.push(ClientContextMenuItem {
            label: match reason {
                Some(reason) => format!("Settle  s · {reason}"),
                None => "Settle  s".into(),
            },
            action: if reason.is_none() {
                Action::ConversationSettle
            } else {
                Action::Disabled
            },
        });
    }
    items
}

pub(super) fn conversation_details(target: &ClientContextMenuTarget) -> Vec<String> {
    match target {
        ClientContextMenuTarget::Conversation {
            record, available, ..
        } => {
            let mut lines = vec![
                conversation_title(record),
                format!(
                    "{} · {}",
                    record_status(record, !available),
                    record.provider
                ),
                conversation_breadcrumb(record),
            ];
            if let Some(error) = &record.resume_error {
                lines.push(error.clone());
            }
            lines
        }
        ClientContextMenuTarget::ConversationLocations { cwd, locations, .. } => {
            let mut lines = vec![
                format!("Resume directory: {cwd}"),
                "Choose the destination tab explicitly.".into(),
            ];
            if locations.is_empty() {
                lines.push("No tabs available. Recreate the original Space instead.".into());
            }
            lines
        }
        _ => Vec::new(),
    }
}

fn read_key(endpoint_id: &ClientEndpointId, id: &str) -> String {
    match endpoint_id {
        ClientEndpointId::Local => format!("local:{id}"),
        ClientEndpointId::Ssh(profile) => format!("ssh:{profile}:{id}"),
    }
}

impl ClientShellState {
    fn mark_conversation_read(
        &mut self,
        endpoint_id: &ClientEndpointId,
        id: &str,
        outcome: &mut ClientShellInput,
    ) {
        let attention = self
            .endpoints
            .iter()
            .find(|endpoint| &endpoint.endpoint_id == endpoint_id)
            .and_then(|endpoint| endpoint.conversations.as_ref())
            .and_then(|cache| {
                cache
                    .projection
                    .conversations
                    .iter()
                    .find(|record| record.id == id)
            })
            .map(|record| record.attention_returned_at_ms);
        if let Some(attention) = attention {
            let previous = self
                .conversation_read
                .entry(read_key(endpoint_id, id))
                .or_default();
            if *previous < attention {
                *previous = attention;
                self.persist_chrome_preferences(outcome);
                outcome.repaint = true;
            }
        }
    }

    pub(super) fn acknowledge_visible_conversations(&mut self, surface: &PaneSurfaceFrame) -> bool {
        if self.outer_focused == Some(false) {
            return false;
        }
        let Some(endpoint) = self
            .endpoints
            .iter()
            .find(|endpoint| endpoint.endpoint_id == self.active_endpoint_id)
        else {
            return false;
        };
        let Some(snapshot) = endpoint.snapshot.as_deref() else {
            return false;
        };
        if snapshot.boot_id != surface.boot_id
            || snapshot.revision != surface.projection_revision
            || !cache_current(endpoint)
        {
            return false;
        }
        let visible = endpoint
            .conversations
            .as_ref()
            .map(|cache| {
                cache
                    .projection
                    .conversations
                    .iter()
                    .filter(|record| {
                        record.attachments.iter().any(|attachment| {
                            snapshot.focused_pane_id.as_ref() == Some(&attachment.public_pane_id)
                                && surface
                                    .panes
                                    .iter()
                                    .any(|pane| pane.pane_id == attachment.public_pane_id)
                        })
                    })
                    .map(|record| (record.id.clone(), record.attention_returned_at_ms))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let endpoint_id = self.active_endpoint_id.clone();
        let mut changed = false;
        for (id, attention) in visible {
            let previous = self
                .conversation_read
                .entry(read_key(&endpoint_id, &id))
                .or_default();
            if *previous < attention {
                *previous = attention;
                changed = true;
            }
        }
        if changed {
            let mut outcome = ClientShellInput::default();
            self.persist_chrome_preferences(&mut outcome);
        }
        changed
    }
}

impl ClientShellState {
    pub(crate) fn take_conversation_actions(&mut self) -> Vec<ClientShellAction> {
        std::mem::take(&mut self.conversation_actions)
    }

    fn queue_conversation_page(&mut self, endpoint_id: &ClientEndpointId) {
        let Some(endpoint) = self
            .endpoints
            .iter()
            .find(|endpoint| &endpoint.endpoint_id == endpoint_id)
        else {
            return;
        };
        if endpoint.status != ClientEndpointStatus::Online
            || !cache_current(endpoint)
            || !endpoint
                .methods
                .as_ref()
                .is_some_and(|methods| methods.contains("agent.conversation.list"))
        {
            return;
        }
        let Some(cache) = endpoint.conversations.as_ref() else {
            return;
        };
        let Some(offset) = cache.projection.next_offset else {
            return;
        };
        let revision = cache.projection.revision;
        let generation = cache.generation;
        if self.pending_requests.values().any(|pending| matches!(&pending.kind, PendingEndpointKind::ConversationList { endpoint_id: source, generation: gen, revision: rev, offset: at } if source == endpoint_id && *gen == generation && *rev == revision && *at == offset)) { return; }
        let boot_id = cache.projection.boot_id.clone();
        let request_id = format!("client-shell:{}", self.next_request_id);
        self.next_request_id = self.next_request_id.saturating_add(1);
        self.pending_requests.insert(
            request_id.clone(),
            PendingEndpointRequest {
                boot_id: boot_id.clone(),
                method_name: "agent.conversation.list".into(),
                confirmation_workspace_id: None,
                kind: PendingEndpointKind::ConversationList {
                    endpoint_id: endpoint_id.clone(),
                    generation,
                    revision,
                    offset,
                },
            },
        );
        self.conversation_actions.push(ClientShellAction::Endpoint {
            endpoint_id: endpoint_id.clone(),
            boot_id,
            request: Box::new(crate::api::schema::Request {
                id: request_id,
                method: crate::api::schema::Method::AgentConversationList(
                    crate::api::schema::AgentConversationListParams { offset, limit: 100 },
                ),
            }),
        });
    }

    pub(super) fn complete_conversation_page(
        &mut self,
        endpoint_id: ClientEndpointId,
        generation: Option<u64>,
        revision: u64,
        offset: usize,
        boot_id: &str,
        result: Result<crate::api::schema::ResponseResult, ClientShellEndpointError>,
    ) -> (bool, Vec<ClientShellAction>) {
        let Some(endpoint) = self
            .endpoints
            .iter_mut()
            .find(|endpoint| endpoint.endpoint_id == endpoint_id)
        else {
            return (false, Vec::new());
        };
        if endpoint.snapshot_generation != generation
            || endpoint
                .snapshot
                .as_deref()
                .is_none_or(|snapshot| snapshot.boot_id != boot_id)
        {
            return (false, Vec::new());
        }
        let Some(cache) = endpoint.conversations.as_mut() else {
            return (false, Vec::new());
        };
        if cache.generation != generation
            || cache.projection.revision != revision
            || cache.projection.boot_id != boot_id
            || cache.projection.next_offset != Some(offset)
        {
            return (false, Vec::new());
        }
        match result {
            Ok(crate::api::schema::ResponseResult::AgentConversations { projection }) => {
                match serde_json::from_value::<ClientAgentConversationsProjection>(projection) {
                    Ok(page)
                        if page.boot_id == boot_id
                            && page.revision == revision
                            && page.next_offset.is_none_or(|next| next > offset)
                            && page.conversations.iter().all(|record| {
                                crate::agent_inbox::validate_record(record).is_ok()
                            }) =>
                    {
                        for record in page.conversations {
                            if let Some(previous) = cache
                                .projection
                                .conversations
                                .iter_mut()
                                .find(|previous| previous.id == record.id)
                            {
                                if previous.revision <= record.revision {
                                    *previous = record;
                                }
                            } else {
                                cache.projection.conversations.push(record);
                            }
                        }
                        cache.projection.next_offset = page.next_offset;
                        cache.projection.total_count = page.total_count;
                        cache.projection.recovery_error = page.recovery_error;
                    }
                    Ok(mut page)
                        if page.boot_id == boot_id
                            && page.revision > revision
                            && page.conversations.iter().all(|record| {
                                crate::agent_inbox::validate_record(record).is_ok()
                            }) =>
                    {
                        page.conversations.clear();
                        page.next_offset = Some(0);
                        endpoint.conversations = Some(CachedConversations {
                            generation,
                            projection: page,
                        });
                    }
                    _ => {
                        cache.projection.recovery_error = Some("Conversation history changed or could not be read. Reconnect to reload all history.".into());
                        return (true, Vec::new());
                    }
                }
            }
            Err(error) => {
                cache.projection.recovery_error = Some(format!(
                    "Could not load all conversations: {}. Reconnect to retry.",
                    error.message
                ));
                return (true, Vec::new());
            }
            _ => {
                cache.projection.recovery_error = Some(
                    "Endpoint returned an invalid conversation page. Reconnect to retry.".into(),
                );
                return (true, Vec::new());
            }
        }
        self.queue_conversation_page(&endpoint_id);
        (true, self.take_conversation_actions())
    }
}

pub(super) fn prepare_panel(
    buffer: &mut Buffer,
    area: Rect,
    config: &ClientShellConfig,
    hits: &mut ShellHitMap,
) {
    super::agent_sidebar::render_agent_panel_header(buffer, area, None, config, hits);
    hits.agent_body = Rect::new(
        area.x,
        area.y.saturating_add(3),
        area.width,
        area.height.saturating_sub(3),
    );
}

impl ClientShellState {
    pub(crate) fn conversation_request_is_endpoint_qualified(
        &self,
        request_id: &str,
        endpoint_id: &ClientEndpointId,
        generation: u64,
    ) -> bool {
        self.pending_requests
            .get(request_id)
            .is_some_and(|pending| match &pending.kind {
                PendingEndpointKind::Conversation {
                    endpoint_id: source,
                } => {
                    source == endpoint_id
                        && self.endpoints.iter().any(|endpoint| {
                            &endpoint.endpoint_id == source
                                && endpoint.snapshot_generation == Some(generation)
                        })
                }
                PendingEndpointKind::ConversationList {
                    endpoint_id: source,
                    generation: expected,
                    ..
                } => source == endpoint_id && *expected == Some(generation),
                _ => false,
            })
    }
}

fn location_label<'a>(label: &'a str, fallback: &'a str) -> &'a str {
    if label.is_empty() {
        fallback
    } else {
        label
    }
}

impl ClientShellState {
    pub(super) fn reconcile_conversation_selection(&mut self, old: &[ConversationRow]) {
        let Some(selected) = self.agent_inbox_selected.as_ref() else {
            return;
        };
        let next = self.visible_conversations();
        let exists = |row: &ConversationRow| {
            next.iter()
                .any(|candidate| candidate.endpoint_id == row.endpoint_id && candidate.id == row.id)
        };
        if next
            .iter()
            .any(|row| (&row.endpoint_id, &row.id) == (&selected.0, &selected.1))
        {
            return;
        }
        let index = old
            .iter()
            .position(|row| (&row.endpoint_id, &row.id) == (&selected.0, &selected.1))
            .unwrap_or(0);
        self.agent_inbox_selected = old
            .iter()
            .skip(index.saturating_add(1))
            .find(|row| exists(row))
            .or_else(|| old.iter().take(index).rev().find(|row| exists(row)))
            .or_else(|| next.first())
            .map(|row| (row.endpoint_id.clone(), row.id.clone()));
    }
}

impl ClientShellState {
    fn reveal_selected_conversation(&mut self) {
        let Some(selected) = &self.agent_inbox_selected else {
            return;
        };
        let height = usize::from(self.hits.agent_body.height);
        if height == 0 {
            return;
        }
        let rows = self.visible_conversations();
        let mut line = self
            .endpoints
            .iter()
            .filter_map(|endpoint| endpoint.conversations.as_ref())
            .map(|cache| {
                usize::from(cache.projection.next_offset.is_some())
                    + usize::from(cache.projection.recovery_error.is_some())
            })
            .sum::<usize>();
        for section in 0..3 {
            line += 1;
            for row in rows.iter().filter(|row| row.section == section) {
                if (&row.endpoint_id, &row.id) == (&selected.0, &selected.1) {
                    if line < self.agent_scroll {
                        self.agent_scroll = line;
                    } else if line.saturating_add(2) > self.agent_scroll.saturating_add(height) {
                        self.agent_scroll = line.saturating_add(2).saturating_sub(height);
                    }
                    return;
                }
                line += 2;
            }
        }
    }
}

pub(super) fn reserved_resume_terminals(
    records: &[ConversationRecord],
) -> HashMap<&str, &ConversationRecord> {
    records
        .iter()
        .filter(|record| record.resume_state != ResumeState::None)
        .filter_map(|record| {
            record.resume_target.as_ref().and_then(|target| {
                (!target.terminal_id.is_empty()).then_some((target.terminal_id.as_str(), record))
            })
        })
        .collect()
}

pub(super) fn reserved_provisional(
    record: &ConversationRecord,
    reserved: &HashMap<&str, &ConversationRecord>,
) -> bool {
    record.session_ref.is_none()
        && !record.attachments.is_empty()
        && record.attachments.iter().all(|attachment| {
            reserved
                .get(attachment.terminal_id.as_str())
                .is_some_and(|owner| {
                    (record.provider.is_empty() || record.provider == owner.provider)
                        && (record.source.is_empty() || record.source == owner.source)
                })
        })
}
