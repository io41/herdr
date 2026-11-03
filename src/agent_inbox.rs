//! Endpoint-owned conversation lifecycle. These types travel only over JSON.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::agent_resume::AgentSessionRef;

const REGISTRY_VERSION: u64 = 1;
const COMPLETION_GRACE_MS: u64 = 2_000;
pub const MAX_RECORD_BYTES: usize = 128 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Disposition {
    Inbox,
    Settled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversationSection {
    Inbox,
    Working,
    Settled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionState {
    Working,
    Idle,
    Unknown,
    Unavailable,
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResumeState {
    None,
    Pending,
    Uncertain,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationAttachment {
    #[serde(default)]
    pub terminal_id: String,
    pub pane_id: u32,
    #[serde(default)]
    pub public_pane_id: String,
    pub workspace_id: String,
    pub tab_id: String,
    #[serde(default)]
    pub workspace_label: String,
    #[serde(default)]
    pub tab_label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane_label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane_number: Option<usize>,
    pub workspace_location_id: String,
    pub tab_location_id: String,
    pub cwd: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationRecord {
    pub id: String,
    pub provider: String,
    pub source: String,
    pub session_ref: Option<AgentSessionRef>,
    pub title: String,
    pub saved_location: ConversationAttachment,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub retired_terminal_ids: Vec<String>,
    pub disposition: Disposition,
    pub created_at_ms: u64,
    pub attention_returned_at_ms: u64,
    pub last_human_submission_at_ms: Option<u64>,
    pub settled_at_ms: Option<u64>,
    #[serde(default)]
    pub completed_at_ms: Option<u64>,
    pub revision: u64,
    pub attachments: Vec<ConversationAttachment>,
    pub section: ConversationSection,
    pub execution: ExecutionState,
    pub queued_human_work: bool,
    pub blocking_request: bool,
    pub failure: bool,
    pub capability_limits: Vec<String>,
    pub resume_state: ResumeState,
    #[serde(default)]
    pub resume_target: Option<ConversationAttachment>,
    #[serde(default)]
    pub resume_started_at_ms: Option<u64>,
    #[serde(default)]
    pub resume_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationObservation {
    pub provider: String,
    pub source: String,
    pub session_ref: Option<AgentSessionRef>,
    /// Terminal generation, not a title or directory. Provisional IDs are boot scoped.
    pub generation: String,
    pub attachment: ConversationAttachment,
    pub title: String,
    pub execution: ExecutionState,
    pub queued_human_work: bool,
    pub blocking_request: bool,
    pub failure: bool,
    pub capability_limits: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConversationAction {
    Settle,
    SettleAndClose,
    Reopen,
    BeginResume,
    ResumeLaunched(Box<ConversationAttachment>),
    ResumeSucceeded,
    ResumeFailed,
    ResumeUncertain,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InboxError {
    NotFound,
    Conflict { current_revision: u64 },
    Ineligible(String),
    Persistence(String),
    PersistenceUncertain(String),
    Recovery(String),
}

impl fmt::Display for InboxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => write!(f, "conversation not found"),
            Self::Conflict { current_revision } => {
                write!(
                    f,
                    "conversation changed; current revision is {current_revision}"
                )
            }
            Self::Ineligible(message)
            | Self::Persistence(message)
            | Self::PersistenceUncertain(message)
            | Self::Recovery(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for InboxError {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct RegistryFile {
    version: u64,
    revision: u64,
    records: Vec<ConversationRecord>,
}

#[derive(Debug, Clone)]
pub struct InboxRegistry {
    records: BTreeMap<String, ConversationRecord>,
    pending_completions: BTreeMap<String, u64>,
    bindings: BTreeMap<String, String>,
    aliases: BTreeMap<String, String>,
    path: Option<PathBuf>,
    boot_id: String,
    revision: u64,
    writes_blocked: bool,
    pub recovery_error: Option<String>,
}

impl InboxRegistry {
    pub fn load(path: Option<PathBuf>, boot_id: String) -> Self {
        let mut registry = Self {
            records: BTreeMap::new(),
            pending_completions: BTreeMap::new(),
            bindings: BTreeMap::new(),
            aliases: BTreeMap::new(),
            path,
            boot_id,
            revision: 0,
            writes_blocked: false,
            recovery_error: None,
        };
        let Some(path) = registry.path.as_ref() else {
            return registry;
        };
        let primary = read_registry(path);
        let data = match primary {
            Ok(Some(data)) => Some(data),
            Ok(None) => {
                // A backup is committed data. A temporary file may be an uncommitted write.
                match read_registry(&sidecar(path, "bak")) {
                    Ok(data) => data,
                    Err(error) => {
                        registry.writes_blocked = true;
                        registry.recovery_error = Some(error);
                        None
                    }
                }
            }
            Err(error) => {
                registry.writes_blocked = true;
                registry.recovery_error = Some(error);
                read_registry(&sidecar(path, "bak")).ok().flatten()
            }
        };
        if let Some(data) = data {
            registry.revision = data.revision;
            for mut record in data.records {
                if record.session_ref.is_none() {
                    continue;
                }
                record.attachments.clear();
                record.execution = ExecutionState::Closed;
                if record.resume_state == ResumeState::Pending {
                    record.resume_state = ResumeState::Uncertain;
                }
                record.section = project(&record, false);
                registry.records.insert(record.id.clone(), record);
            }
        }
        registry
    }

    pub fn retired_terminal_ids(&self) -> BTreeSet<String> {
        self.records
            .values()
            .flat_map(|record| record.retired_terminal_ids.iter().cloned())
            .collect()
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn records(&self) -> Vec<ConversationRecord> {
        self.records.values().cloned().collect()
    }

    pub fn get(&self, id: &str) -> Option<&ConversationRecord> {
        self.records
            .get(self.aliases.get(id).map_or(id, String::as_str))
    }

    pub fn next_deadline_ms(&self) -> Option<u64> {
        self.pending_completions
            .values()
            .copied()
            .chain(
                self.records
                    .values()
                    .filter(|record| record.resume_state == ResumeState::Pending)
                    .filter_map(|record| {
                        record
                            .resume_started_at_ms
                            .map(|started| started.saturating_add(30_000))
                    }),
            )
            .min()
    }

    /// Reconcile a complete endpoint observation, including duplicate attachments.
    pub fn reconcile(
        &mut self,
        observations: &[ConversationObservation],
        now_ms: u64,
    ) -> Result<bool, InboxError> {
        let mut next = self.clone();
        let old = next.records.clone();
        let mut grouped: BTreeMap<String, Vec<&ConversationObservation>> = BTreeMap::new();
        let mut bindings = BTreeMap::new();
        let mut identities: BTreeMap<String, String> = next
            .records
            .values()
            .filter_map(|record| {
                record.session_ref.as_ref().map(|session_ref| {
                    (
                        native_identity(&record.provider, &record.source, session_ref),
                        record.id.clone(),
                    )
                })
            })
            .collect();
        for observation in observations {
            let binding =
                serde_json::to_string(&(&observation.generation, observation.attachment.pane_id))
                    .expect("string tuple serializes");
            let id = match authoritative_session(observation) {
                Some(session_ref) => identities
                    .entry(native_identity(
                        &observation.provider,
                        &observation.source,
                        session_ref,
                    ))
                    .or_insert_with(|| {
                        format!("conversation_{}", crate::terminal::TerminalId::alloc())
                    })
                    .clone(),
                None => observation_id(observation, &self.boot_id),
            };
            let provisional = next.bindings.get(&binding).and_then(|old_id| {
                next.records
                    .get(old_id)
                    .filter(|record| {
                        record.session_ref.is_none()
                            && record.id != id
                            && (record.provider.is_empty()
                                || record.provider == observation.provider)
                            && (record.source.is_empty() || record.source == observation.source)
                    })
                    .cloned()
            });
            if next.records.contains_key(&id) {
                if let Some(provisional) = &provisional {
                    next.aliases.insert(provisional.id.clone(), id.clone());
                    next.records.remove(&provisional.id);
                    next.pending_completions.remove(&provisional.id);
                    if let Some(submitted_at) = provisional.last_human_submission_at_ms {
                        let record = next.records.get_mut(&id).expect("existing native identity");
                        if record
                            .last_human_submission_at_ms
                            .is_none_or(|previous| previous < submitted_at)
                        {
                            record.last_human_submission_at_ms = Some(submitted_at);
                            if record
                                .settled_at_ms
                                .is_none_or(|settled| settled <= submitted_at)
                            {
                                reopen(record, submitted_at);
                            }
                        }
                    }
                }
            } else {
                let mut record =
                    provisional.unwrap_or_else(|| new_record(observation, &id, now_ms));
                if record.id != id {
                    next.records.remove(&record.id);
                    next.aliases.insert(record.id.clone(), id.clone());
                    if let Some(deadline) = next.pending_completions.remove(&record.id) {
                        next.pending_completions.insert(id.clone(), deadline);
                    }
                    record.id = id.clone();
                    record.session_ref = authoritative_session(observation).cloned();
                    record.provider = observation.provider.clone();
                    record.source = observation.source.clone();
                }
                next.records.insert(id.clone(), record);
            }
            bindings.insert(binding, id.clone());
            grouped.entry(id).or_default().push(observation);
        }
        next.bindings = bindings;
        for (id, record) in &mut next.records {
            let previous = old.get(id);
            let Some(group) = grouped.get(id) else {
                record.attachments.clear();
                record.execution = ExecutionState::Closed;
                next.pending_completions.remove(id);
                record.section = project(record, false);
                continue;
            };
            let first = group[0];
            record.title = first.title.clone();
            record.attachments = group.iter().map(|item| item.attachment.clone()).collect();
            record
                .attachments
                .sort_by_key(|attachment| attachment.pane_id);
            record
                .attachments
                .dedup_by_key(|attachment| attachment.pane_id);
            if record.attachments.len() == 1 {
                record.saved_location = record.attachments[0].clone();
            } else if previous.is_none() {
                record.saved_location = ConversationAttachment {
                    workspace_label: "Multiple locations".into(),
                    ..ConversationAttachment::default()
                };
            }
            record.capability_limits = group
                .iter()
                .flat_map(|item| item.capability_limits.iter().cloned())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            record.queued_human_work = group.iter().any(|item| item.queued_human_work);
            record.blocking_request = group.iter().any(|item| item.blocking_request);
            record.failure = group.iter().any(|item| item.failure);
            record.execution = if group
                .iter()
                .any(|item| item.execution == ExecutionState::Working)
            {
                ExecutionState::Working
            } else if group
                .iter()
                .any(|item| item.execution == ExecutionState::Unknown)
            {
                ExecutionState::Unknown
            } else if group
                .iter()
                .any(|item| item.execution == ExecutionState::Unavailable)
            {
                ExecutionState::Unavailable
            } else {
                ExecutionState::Idle
            };
            let fresh_request = record.blocking_request
                && previous.is_some_and(|previous| !previous.blocking_request);
            let fresh_failure =
                record.failure && previous.is_some_and(|previous| !previous.failure);
            if fresh_request || fresh_failure {
                reopen(record, now_ms);
            }
            if record.blocking_request || record.failure {
                next.pending_completions.remove(id);
            } else if record.execution == ExecutionState::Working {
                record.completed_at_ms = None;
                next.pending_completions.remove(id);
            } else if record.execution == ExecutionState::Idle
                && previous.is_some_and(|previous| previous.execution == ExecutionState::Working)
                && record.disposition == Disposition::Inbox
            {
                next.pending_completions
                    .entry(id.clone())
                    .or_insert_with(|| now_ms.saturating_add(COMPLETION_GRACE_MS));
            }
            record.section = project(record, next.pending_completions.contains_key(id));
        }
        // Boot-scoped records cannot be safely recovered after their terminal disappears.
        next.records
            .retain(|_, record| record.session_ref.is_some() || !record.attachments.is_empty());
        next.release_completions(now_ms);
        self.commit(next)
    }

    pub fn tick(&mut self, now_ms: u64) -> Result<bool, InboxError> {
        let mut next = self.clone();
        next.release_completions(now_ms);
        self.commit(next)
    }

    fn release_completions(&mut self, now_ms: u64) {
        self.pending_completions.retain(|id, deadline| {
            if now_ms < *deadline {
                return true;
            }
            if let Some(record) = self.records.get_mut(id) {
                if record.execution == ExecutionState::Idle
                    && record.disposition == Disposition::Inbox
                    && !record.blocking_request
                    && !record.failure
                {
                    record.attention_returned_at_ms = now_ms;
                    record.completed_at_ms = Some(now_ms);
                }
                record.section = project(record, false);
            }
            false
        });
    }

    pub fn mutate(
        &mut self,
        id: &str,
        expected_revision: u64,
        action: ConversationAction,
        now_ms: u64,
    ) -> Result<ConversationRecord, InboxError> {
        let id = self.aliases.get(id).map_or(id, String::as_str).to_string();
        let id = id.as_str();
        let current = self.records.get(id).ok_or(InboxError::NotFound)?;
        let satisfied = match &action {
            ConversationAction::Settle => current.disposition == Disposition::Settled,
            ConversationAction::SettleAndClose => {
                current.disposition == Disposition::Settled
                    && current.execution == ExecutionState::Closed
                    && current.attachments.is_empty()
            }
            ConversationAction::Reopen => current.disposition == Disposition::Inbox,
            ConversationAction::BeginResume => current.resume_state == ResumeState::Pending,
            ConversationAction::ResumeLaunched(target) => {
                current.resume_target.as_ref() == Some(target.as_ref())
            }
            ConversationAction::ResumeSucceeded => {
                current.resume_state == ResumeState::None
                    && current.disposition == Disposition::Inbox
            }
            ConversationAction::ResumeFailed => current.resume_state == ResumeState::None,
            ConversationAction::ResumeUncertain => current.resume_state == ResumeState::Uncertain,
        };
        if satisfied {
            return Ok(current.clone());
        }
        if expected_revision != current.revision {
            return Err(InboxError::Conflict {
                current_revision: current.revision,
            });
        }
        if self.writes_blocked {
            return Err(InboxError::Recovery(
                self.recovery_error.clone().unwrap_or_default(),
            ));
        }
        let mut next = self.clone();
        let record = next.records.get_mut(id).expect("record checked");
        match action {
            ConversationAction::Settle | ConversationAction::SettleAndClose => {
                if record.session_ref.is_none()
                    || !matches!(
                        record.execution,
                        ExecutionState::Idle | ExecutionState::Closed
                    )
                    || record.queued_human_work
                    || record.blocking_request
                    || record.resume_state != ResumeState::None
                {
                    return Err(InboxError::Ineligible(
                        "conversation is running, blocked, unavailable, or lacks durable identity"
                            .into(),
                    ));
                }
                if action == ConversationAction::SettleAndClose {
                    if record.attachments.len() > 1 {
                        return Err(InboxError::Ineligible(
                            "conversation has more than one attachment".into(),
                        ));
                    }
                    if let Some(attachment) = record.attachments.first() {
                        if attachment.terminal_id.is_empty() {
                            return Err(InboxError::Ineligible(
                                "conversation lacks terminal generation".into(),
                            ));
                        }
                        if !record
                            .retired_terminal_ids
                            .contains(&attachment.terminal_id)
                        {
                            record
                                .retired_terminal_ids
                                .push(attachment.terminal_id.clone());
                        }
                        record.saved_location = attachment.clone();
                    }
                    record.attachments.clear();
                    record.execution = ExecutionState::Closed;
                }
                record.disposition = Disposition::Settled;
                record.settled_at_ms = Some(now_ms);
            }
            ConversationAction::Reopen => reopen(record, now_ms),
            ConversationAction::BeginResume => {
                if record.session_ref.is_none()
                    || !record.attachments.is_empty()
                    || record.resume_state != ResumeState::None
                {
                    return Err(InboxError::Ineligible(
                        "conversation is attached or a prior resume outcome is uncertain".into(),
                    ));
                }
                record.resume_state = ResumeState::Pending;
                record.resume_error = None;
                record.resume_started_at_ms = Some(now_ms);
                record.resume_target = None;
            }
            ConversationAction::ResumeLaunched(target) => {
                if record.resume_state != ResumeState::Pending {
                    return Err(InboxError::Ineligible("resume was not started".into()));
                }
                record.resume_target = Some(*target);
            }
            ConversationAction::ResumeSucceeded => {
                if record.resume_state == ResumeState::None && record.attachments.is_empty() {
                    return Err(InboxError::Ineligible("resume was not started".into()));
                }
                record.resume_state = ResumeState::None;
                record.resume_error = None;
                record.resume_started_at_ms = None;
                record.resume_target = None;
                reopen(record, now_ms);
            }
            ConversationAction::ResumeFailed | ConversationAction::ResumeUncertain => {
                if record.resume_state == ResumeState::None {
                    return Err(InboxError::Ineligible("resume was not started".into()));
                }
                record.resume_state = if action == ConversationAction::ResumeUncertain {
                    ResumeState::Uncertain
                } else {
                    record.resume_error = Some(
                        "Resume failed before the saved session was confirmed. You can retry."
                            .into(),
                    );
                    record.resume_target = None;
                    record.resume_started_at_ms = None;
                    ResumeState::None
                };
            }
        }
        next.pending_completions.remove(id);
        record.section = project(record, false);
        self.commit(next)?;
        Ok(self.records[id].clone())
    }

    /// Call only for a verified human submission, never an inferred Working transition.
    pub fn human_submission(&mut self, id: &str, now_ms: u64) -> Result<(), InboxError> {
        let id = self.aliases.get(id).map_or(id, String::as_str).to_string();
        if self.writes_blocked {
            return Err(InboxError::Recovery(
                self.recovery_error.clone().unwrap_or_default(),
            ));
        }
        let mut next = self.clone();
        let record = next.records.get_mut(&id).ok_or(InboxError::NotFound)?;
        if record
            .last_human_submission_at_ms
            .is_some_and(|last| last >= now_ms)
        {
            return Ok(());
        }
        reopen(record, now_ms);
        record.last_human_submission_at_ms = Some(now_ms);
        record.completed_at_ms = None;
        record.queued_human_work = true;
        record.section = project(record, false);
        next.pending_completions.remove(&id);
        self.commit(next)?;
        Ok(())
    }

    fn commit(&mut self, next: Self) -> Result<bool, InboxError> {
        self.commit_with(next, Self::persist)
    }

    fn commit_with(
        &mut self,
        mut next: Self,
        persist: impl FnOnce(&Self) -> Result<(), InboxError>,
    ) -> Result<bool, InboxError> {
        let changed = self.records != next.records;
        if changed {
            next.revision = self.revision.saturating_add(1);
            for (id, record) in &mut next.records {
                if self.records.get(id) != Some(record) {
                    record.revision = self
                        .records
                        .get(id)
                        .map_or(1, |old| old.revision.saturating_add(1));
                }
            }
            // Observations still provide recovery rows when a damaged registry is read-only.
            if !next.writes_blocked {
                if let Err(error) = persist(&next) {
                    if let InboxError::PersistenceUncertain(message) = &error {
                        next.writes_blocked = true;
                        next.recovery_error = Some(message.clone());
                        *self = next;
                    }
                    return Err(error);
                }
            }
        }
        *self = next;
        Ok(changed)
    }

    fn persist(&self) -> Result<(), InboxError> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let data = RegistryFile {
            version: REGISTRY_VERSION,
            revision: self.revision,
            records: self
                .records
                .values()
                .filter(|record| record.session_ref.is_some())
                .cloned()
                .collect(),
        };
        for record in &data.records {
            validate_record(record).map_err(InboxError::Persistence)?;
        }
        let bytes = serde_json::to_vec_pretty(&data)
            .map_err(|error| InboxError::Persistence(error.to_string()))?;
        save_registry(path, &bytes)
    }
}

fn native_identity(provider: &str, source: &str, session_ref: &AgentSessionRef) -> String {
    serde_json::to_string(&(provider, source, session_ref)).expect("identity serializes")
}

fn authoritative_session(observation: &ConversationObservation) -> Option<&AgentSessionRef> {
    if observation.provider.is_empty() || observation.source.is_empty() {
        None
    } else {
        observation.session_ref.as_ref()
    }
}

fn observation_id(observation: &ConversationObservation, boot_id: &str) -> String {
    format!(
        "provisional:{}",
        serde_json::to_string(&(
            boot_id,
            &observation.generation,
            observation.attachment.pane_id
        ))
        .expect("identity serializes")
    )
}

fn new_record(observation: &ConversationObservation, id: &str, now_ms: u64) -> ConversationRecord {
    ConversationRecord {
        id: id.into(),
        provider: observation.provider.clone(),
        source: observation.source.clone(),
        session_ref: authoritative_session(observation).cloned(),
        title: observation.title.clone(),
        saved_location: observation.attachment.clone(),
        retired_terminal_ids: Vec::new(),
        disposition: Disposition::Inbox,
        created_at_ms: now_ms,
        attention_returned_at_ms: now_ms,
        last_human_submission_at_ms: None,
        settled_at_ms: None,
        completed_at_ms: None,
        revision: 0,
        attachments: Vec::new(),
        section: ConversationSection::Inbox,
        execution: observation.execution,
        queued_human_work: false,
        blocking_request: false,
        failure: false,
        capability_limits: Vec::new(),
        resume_state: ResumeState::None,
        resume_target: None,
        resume_started_at_ms: None,
        resume_error: None,
    }
}

fn reopen(record: &mut ConversationRecord, now_ms: u64) {
    record.disposition = Disposition::Inbox;
    record.settled_at_ms = None;
    record.attention_returned_at_ms = now_ms;
}

fn project(record: &ConversationRecord, completing: bool) -> ConversationSection {
    if record.session_ref.is_none()
        || matches!(
            record.execution,
            ExecutionState::Unknown | ExecutionState::Unavailable
        )
        || record.resume_state != ResumeState::None
        || record.blocking_request
        || (record.failure && record.disposition != Disposition::Settled)
    {
        ConversationSection::Inbox
    } else if record.disposition == Disposition::Settled {
        ConversationSection::Settled
    } else if record.execution == ExecutionState::Working || completing {
        ConversationSection::Working
    } else {
        ConversationSection::Inbox
    }
}

fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(format!(".{suffix}"));
    PathBuf::from(name)
}

fn read_registry(path: &Path) -> Result<Option<RegistryFile>, String> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("cannot read {}: {error}", path.display())),
    };
    let value: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|error| format!("invalid registry {}: {error}", path.display()))?;
    if value.get("version").and_then(serde_json::Value::as_u64) != Some(REGISTRY_VERSION) {
        return Err(format!(
            "unsupported conversation registry version in {}",
            path.display()
        ));
    }
    let data: RegistryFile = serde_json::from_value(value)
        .map_err(|error| format!("invalid registry {}: {error}", path.display()))?;
    let mut ids = BTreeSet::new();
    let mut native_ids = BTreeSet::new();
    for record in &data.records {
        validate_record(record)?;
        if !ids.insert(&record.id)
            || record.session_ref.is_none()
            || !native_ids.insert(native_identity(
                &record.provider,
                &record.source,
                record
                    .session_ref
                    .as_ref()
                    .expect("validated native record"),
            ))
        {
            return Err(format!(
                "invalid conversation identity in {}",
                path.display()
            ));
        }
    }
    Ok(Some(data))
}

pub fn validate_record(record: &ConversationRecord) -> Result<(), String> {
    if serde_json::to_vec(record)
        .map_err(|error| error.to_string())?
        .len()
        > MAX_RECORD_BYTES
    {
        return Err("conversation record exceeds 128 KiB".into());
    }
    if let Some(session_ref) = &record.session_ref {
        let valid = match session_ref.kind {
            crate::agent_resume::AgentSessionRefKind::Id => AgentSessionRef::id(&session_ref.value),
            crate::agent_resume::AgentSessionRefKind::Path => {
                AgentSessionRef::path(&session_ref.value)
            }
        };
        if valid.is_none() || record.provider.is_empty() || record.source.is_empty() {
            return Err("invalid native conversation identity".into());
        }
    }
    Ok(())
}

fn save_registry(path: &Path, bytes: &[u8]) -> Result<(), InboxError> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let temporary = sidecar(
        path,
        &format!("tmp-{}", crate::terminal::TerminalId::alloc()),
    );
    let backup_temporary = sidecar(&temporary, "bak");
    let result = (|| -> std::io::Result<()> {
        std::fs::create_dir_all(parent)?;
        let mut output = crate::platform::create_private_state_file(&temporary)?;
        output.write_all(bytes)?;
        output.sync_all()?;
        drop(output);
        if path.exists() {
            // Never replace corrupt data or data written by a newer process version.
            read_registry(path).map_err(std::io::Error::other)?;
            let previous = std::fs::read(path)?;
            let mut backup = crate::platform::create_private_state_file(&backup_temporary)?;
            backup.write_all(&previous)?;
            backup.sync_all()?;
            drop(backup);
            crate::platform::replace_file(&backup_temporary, &sidecar(path, "bak"))?;
        }
        crate::platform::replace_file(&temporary, path)
    })();
    let _ = std::fs::remove_file(&temporary);
    let _ = std::fs::remove_file(&backup_temporary);
    result.map_err(|error| InboxError::Persistence(error.to_string()))?;
    finish_commit_sync(crate::platform::sync_parent_directory(parent))
}

fn finish_commit_sync(result: std::io::Result<()>) -> Result<(), InboxError> {
    result.map_err(|error| InboxError::PersistenceUncertain(format!("conversation registry was replaced but directory sync failed; durability is uncertain: {error}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intentional_close_tombstones_survive_repeated_restore_and_saved_location_changes() {
        let path = temp_path("retired-generations");
        let mut registry = InboxRegistry::load(Some(path.clone()), "one".into());
        for (index, generation) in ["first", "restored", "restored-again"]
            .into_iter()
            .enumerate()
        {
            let mut observed = observation(Some("native"), 1, ExecutionState::Idle);
            observed.generation = generation.into();
            observed.attachment.terminal_id = generation.into();
            let now = (index as u64 + 1) * 10;
            registry.reconcile(&[observed], now).unwrap();
            let record = sole(&registry);
            registry
                .mutate(
                    &record.id,
                    record.revision,
                    ConversationAction::SettleAndClose,
                    now + 1,
                )
                .unwrap();
            registry = InboxRegistry::load(Some(path.clone()), format!("boot-{index}"));
            assert_eq!(sole(&registry).saved_location.terminal_id, generation);
        }
        assert_eq!(
            registry.retired_terminal_ids(),
            ["first", "restored", "restored-again"]
                .map(str::to_string)
                .into()
        );
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn legacy_nonclosing_settlement_does_not_retire_a_terminal_generation() {
        let mut registry = registry();
        registry
            .reconcile(&[observation(Some("one"), 1, ExecutionState::Idle)], 10)
            .unwrap();
        let record = sole(&registry);
        registry
            .mutate(&record.id, record.revision, ConversationAction::Settle, 20)
            .unwrap();
        assert!(registry.retired_terminal_ids().is_empty());
        assert_eq!(sole(&registry).attachments.len(), 1);
    }

    fn observation(
        session: Option<&str>,
        pane_id: u32,
        execution: ExecutionState,
    ) -> ConversationObservation {
        ConversationObservation {
            provider: "codex".into(),
            source: "herdr:codex".into(),
            session_ref: session.map(|id| AgentSessionRef::id(id).unwrap()),
            generation: format!("terminal-{pane_id}"),
            attachment: ConversationAttachment {
                terminal_id: format!("terminal-{pane_id}"),
                pane_id,
                public_pane_id: format!("w1:p{pane_id}"),
                workspace_id: "w1".into(),
                tab_id: "w1:t1".into(),
                workspace_label: "Project".into(),
                tab_label: "Main".into(),
                pane_label: None,
                pane_number: Some(1),
                workspace_location_id: "stable-workspace".into(),
                tab_location_id: "stable-tab".into(),
                cwd: "/tmp".into(),
            },
            title: "test".into(),
            execution,
            queued_human_work: false,
            blocking_request: false,
            failure: false,
            capability_limits: vec![
                "Human submission cannot be inferred from terminal status".into()
            ],
        }
    }

    fn registry() -> InboxRegistry {
        InboxRegistry::load(None, "boot-one".into())
    }

    #[test]
    fn duplicate_attachments_preserve_known_location_and_do_not_invent_an_original() {
        let original = observation(Some("one"), 9, ExecutionState::Idle);
        let mut duplicate = observation(Some("one"), 1, ExecutionState::Idle);
        duplicate.attachment.workspace_location_id = "other-space".into();
        duplicate.attachment.tab_location_id = "other-tab".into();
        let mut known = registry();
        known
            .reconcile(std::slice::from_ref(&original), 10)
            .unwrap();
        known
            .reconcile(&[duplicate.clone(), original.clone()], 20)
            .unwrap();
        assert_eq!(sole(&known).saved_location, original.attachment);

        let mut ambiguous = registry();
        ambiguous.reconcile(&[duplicate, original], 10).unwrap();
        assert!(sole(&ambiguous)
            .saved_location
            .workspace_location_id
            .is_empty());
        assert!(sole(&ambiguous).saved_location.tab_location_id.is_empty());
    }

    fn sole(registry: &InboxRegistry) -> ConversationRecord {
        registry.records()[0].clone()
    }
    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir()
            .join(format!(
                "herdr-inbox-{name}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ))
            .join("registry.json")
    }

    #[test]
    fn completion_including_seen_idle_waits_for_grace_and_continuation_cancels() {
        let mut registry = registry();
        let mut observed = observation(Some("one"), 1, ExecutionState::Working);
        registry
            .reconcile(std::slice::from_ref(&observed), 10)
            .unwrap();
        observed.execution = ExecutionState::Idle;
        registry
            .reconcile(std::slice::from_ref(&observed), 20)
            .unwrap();
        assert_eq!(sole(&registry).section, ConversationSection::Working);
        assert_eq!(registry.next_deadline_ms(), Some(2020));
        registry.tick(2019).unwrap();
        assert_eq!(sole(&registry).attention_returned_at_ms, 10);
        observed.execution = ExecutionState::Working;
        registry
            .reconcile(std::slice::from_ref(&observed), 2020)
            .unwrap();
        registry.tick(5000).unwrap();
        assert_eq!(sole(&registry).attention_returned_at_ms, 10);
        observed.execution = ExecutionState::Idle;
        registry.reconcile(&[observed], 6000).unwrap();
        registry.tick(8000).unwrap();
        assert_eq!(sole(&registry).section, ConversationSection::Inbox);
        assert_eq!(sole(&registry).attention_returned_at_ms, 8000);
        assert_eq!(sole(&registry).completed_at_ms, Some(8000));
        assert!(!registry.tick(9000).unwrap());
    }

    #[test]
    fn settlement_retries_conflicts_and_human_events_are_independent_of_execution() {
        let mut registry = registry();
        let mut observed = observation(Some("one"), 1, ExecutionState::Idle);
        registry
            .reconcile(std::slice::from_ref(&observed), 10)
            .unwrap();
        let record = sole(&registry);
        let settled = registry
            .mutate(&record.id, record.revision, ConversationAction::Settle, 20)
            .unwrap();
        assert_eq!(
            registry
                .mutate(&record.id, 0, ConversationAction::Settle, 50)
                .unwrap(),
            settled
        );
        assert!(matches!(
            registry.mutate(&record.id, 0, ConversationAction::Reopen, 50),
            Err(InboxError::Conflict { .. })
        ));
        observed.execution = ExecutionState::Working;
        registry
            .reconcile(std::slice::from_ref(&observed), 60)
            .unwrap();
        assert_eq!(sole(&registry).disposition, Disposition::Settled);
        assert_eq!(sole(&registry).last_human_submission_at_ms, None);
        registry.human_submission(&record.id, 70).unwrap();
        assert_eq!(sole(&registry).disposition, Disposition::Inbox);
        assert_eq!(sole(&registry).last_human_submission_at_ms, Some(70));
        let after = sole(&registry);
        registry.human_submission(&record.id, 70).unwrap();
        assert_eq!(sole(&registry), after);
    }

    #[test]
    fn request_precedes_working_and_settle_rejects_queued_or_running_work() {
        let mut registry = registry();
        let mut observed = observation(Some("one"), 1, ExecutionState::Working);
        registry
            .reconcile(std::slice::from_ref(&observed), 10)
            .unwrap();
        let record = sole(&registry);
        assert!(registry
            .mutate(&record.id, record.revision, ConversationAction::Settle, 20)
            .is_err());
        observed.blocking_request = true;
        registry
            .reconcile(std::slice::from_ref(&observed), 30)
            .unwrap();
        assert_eq!(sole(&registry).section, ConversationSection::Inbox);
        assert_eq!(sole(&registry).attention_returned_at_ms, 30);
        assert!(!registry
            .reconcile(std::slice::from_ref(&observed), 40)
            .unwrap());
        observed.blocking_request = false;
        observed.execution = ExecutionState::Idle;
        observed.queued_human_work = true;
        registry.reconcile(&[observed], 50).unwrap();
        let record = sole(&registry);
        assert!(registry
            .mutate(&record.id, record.revision, ConversationAction::Settle, 60)
            .is_err());
    }

    #[test]
    fn late_identity_duplicate_attachments_moves_and_replacement_do_not_transfer_settlement() {
        let mut registry = registry();
        let mut observed = observation(None, 1, ExecutionState::Idle);
        registry
            .reconcile(std::slice::from_ref(&observed), 10)
            .unwrap();
        let provisional = sole(&registry);
        assert!(registry
            .mutate(
                &provisional.id,
                provisional.revision,
                ConversationAction::Settle,
                20
            )
            .is_err());
        observed.session_ref = AgentSessionRef::id("one");
        registry
            .reconcile(std::slice::from_ref(&observed), 30)
            .unwrap();
        let record = sole(&registry);
        assert_ne!(record.id, provisional.id);
        assert_eq!(record.created_at_ms, 10);
        registry
            .mutate(&record.id, record.revision, ConversationAction::Settle, 40)
            .unwrap();
        let mut duplicate = observed.clone();
        duplicate.attachment.pane_id = 2;
        duplicate.attachment.workspace_id = "w2".into();
        registry
            .reconcile(&[observed.clone(), duplicate.clone()], 50)
            .unwrap();
        assert_eq!(registry.records().len(), 1);
        assert_eq!(sole(&registry).attachments.len(), 2);
        registry.reconcile(&[duplicate], 60).unwrap();
        assert_eq!(sole(&registry).disposition, Disposition::Settled);
        observed.session_ref = AgentSessionRef::id("two");
        registry.reconcile(&[observed], 70).unwrap();
        assert_eq!(registry.records().len(), 2);
        let fresh = registry
            .records()
            .into_iter()
            .find(|record| !record.attachments.is_empty())
            .unwrap();
        assert_eq!(fresh.disposition, Disposition::Inbox);
        assert_eq!(
            registry.get(&record.id).unwrap().disposition,
            Disposition::Settled
        );
    }

    #[test]
    fn durable_closed_sessions_and_pending_resume_survive_restart_without_replayed_completion() {
        let path = temp_path("restart");
        let mut registry = InboxRegistry::load(Some(path.clone()), "one".into());
        registry
            .reconcile(&[observation(Some("one"), 1, ExecutionState::Idle)], 10)
            .unwrap();
        let record = sole(&registry);
        registry
            .mutate(&record.id, record.revision, ConversationAction::Settle, 20)
            .unwrap();
        registry.reconcile(&[], 30).unwrap();
        let closed = sole(&registry);
        assert!(closed.attachments.is_empty());
        registry
            .mutate(
                &closed.id,
                closed.revision,
                ConversationAction::BeginResume,
                40,
            )
            .unwrap();
        let mut restored = InboxRegistry::load(Some(path.clone()), "two".into());
        let restored_record = sole(&restored);
        assert_eq!(restored_record.resume_state, ResumeState::Uncertain);
        assert_eq!(restored_record.disposition, Disposition::Settled);
        assert_eq!(restored_record.section, ConversationSection::Inbox);
        assert_eq!(restored_record.attention_returned_at_ms, 10);
        assert!(restored
            .mutate(
                &record.id,
                restored_record.revision,
                ConversationAction::BeginResume,
                50
            )
            .is_err());
        assert!(!restored.tick(10000).unwrap());
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn resume_success_reopens_only_exact_session_and_uncertain_outcome_blocks_retry() {
        let mut registry = registry();
        registry
            .reconcile(&[observation(Some("one"), 1, ExecutionState::Idle)], 10)
            .unwrap();
        let record = sole(&registry);
        registry
            .mutate(&record.id, record.revision, ConversationAction::Settle, 20)
            .unwrap();
        registry.reconcile(&[], 30).unwrap();
        let record = sole(&registry);
        let pending = registry
            .mutate(
                &record.id,
                record.revision,
                ConversationAction::BeginResume,
                40,
            )
            .unwrap();
        assert_eq!(pending.disposition, Disposition::Settled);
        let resumed = registry
            .mutate(
                &record.id,
                pending.revision,
                ConversationAction::ResumeSucceeded,
                50,
            )
            .unwrap();
        assert_eq!(resumed.disposition, Disposition::Inbox);
        let pending = registry
            .mutate(
                &record.id,
                resumed.revision,
                ConversationAction::BeginResume,
                60,
            )
            .unwrap();
        let uncertain = registry
            .mutate(
                &record.id,
                pending.revision,
                ConversationAction::ResumeUncertain,
                70,
            )
            .unwrap();
        assert_eq!(uncertain.section, ConversationSection::Inbox);
        assert!(registry
            .mutate(
                &record.id,
                uncertain.revision,
                ConversationAction::BeginResume,
                80
            )
            .is_err());
    }

    #[test]
    fn persistence_failure_rolls_back_entire_mutation() {
        let path = temp_path("rollback");
        let mut registry = InboxRegistry::load(Some(path.clone()), "one".into());
        registry
            .reconcile(&[observation(Some("one"), 1, ExecutionState::Idle)], 10)
            .unwrap();
        let before = sole(&registry);
        std::fs::create_dir(sidecar(&path, "bak")).unwrap();
        assert!(matches!(
            registry.mutate(&before.id, before.revision, ConversationAction::Settle, 20),
            Err(InboxError::Persistence(_))
        ));
        assert_eq!(sole(&registry), before);
        let restored = InboxRegistry::load(Some(path.clone()), "two".into());
        assert_eq!(sole(&restored).disposition, Disposition::Inbox);
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn corrupt_and_future_registry_are_preserved_and_backup_is_recoverable() {
        let path = temp_path("recover");
        let mut registry = InboxRegistry::load(Some(path.clone()), "one".into());
        registry
            .reconcile(&[observation(Some("one"), 1, ExecutionState::Idle)], 10)
            .unwrap();
        let record = sole(&registry);
        registry
            .mutate(&record.id, record.revision, ConversationAction::Settle, 20)
            .unwrap();
        for contents in ["interrupted", "{\"version\":999,\"records\":[]}"] {
            std::fs::write(&path, contents).unwrap();
            let mut recovered = InboxRegistry::load(Some(path.clone()), "two".into());
            assert!(recovered.recovery_error.is_some());
            assert_eq!(recovered.records().len(), 1);
            recovered
                .reconcile(&[observation(Some("new"), 2, ExecutionState::Idle)], 30)
                .unwrap();
            let fresh = recovered
                .records()
                .into_iter()
                .find(|record| record.session_ref.as_ref().unwrap().value == "new")
                .unwrap();
            assert!(matches!(
                recovered.mutate(&fresh.id, fresh.revision, ConversationAction::Settle, 40),
                Err(InboxError::Recovery(_))
            ));
            assert_eq!(std::fs::read_to_string(&path).unwrap(), contents);
        }
        std::fs::remove_file(&path).unwrap();
        let recovered = InboxRegistry::load(Some(path.clone()), "three".into());
        assert_eq!(recovered.records().len(), 1);
        assert!(recovered.recovery_error.is_none());
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn provisional_identity_never_persists_and_noop_observation_keeps_revision() {
        let path = temp_path("provisional");
        let mut registry = InboxRegistry::load(Some(path.clone()), "one".into());
        let observed = observation(None, 1, ExecutionState::Idle);
        registry
            .reconcile(std::slice::from_ref(&observed), 10)
            .unwrap();
        let record = sole(&registry);
        assert!(!registry.reconcile(&[observed], 20).unwrap());
        assert_eq!(sole(&registry), record);
        assert!(InboxRegistry::load(Some(path.clone()), "two".into())
            .records()
            .is_empty());
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn closed_settled_history_stays_settled_after_restart() {
        let path = temp_path("closed-settled");
        let mut registry = InboxRegistry::load(Some(path.clone()), "one".into());
        registry
            .reconcile(&[observation(Some("one"), 1, ExecutionState::Idle)], 10)
            .unwrap();
        let record = sole(&registry);
        registry
            .mutate(&record.id, record.revision, ConversationAction::Settle, 20)
            .unwrap();
        registry.reconcile(&[], 30).unwrap();
        assert_eq!(sole(&registry).execution, ExecutionState::Closed);
        assert_eq!(sole(&registry).section, ConversationSection::Settled);
        let restored = InboxRegistry::load(Some(path.clone()), "two".into());
        assert_eq!(sole(&restored).section, ConversationSection::Settled);
        assert_eq!(sole(&restored).settled_at_ms, Some(20));
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn late_source_promotion_keeps_exact_human_submission_callback_binding() {
        let mut registry = registry();
        let mut observed = observation(Some("one"), 1, ExecutionState::Idle);
        observed.source.clear();
        registry
            .reconcile(std::slice::from_ref(&observed), 10)
            .unwrap();
        let provisional = sole(&registry);
        assert!(provisional.session_ref.is_none());
        observed.source = "herdr:codex".into();
        registry
            .reconcile(std::slice::from_ref(&observed), 20)
            .unwrap();
        assert_eq!(registry.records().len(), 1);
        let promoted = sole(&registry);
        assert_ne!(promoted.id, provisional.id);
        assert_eq!(promoted.created_at_ms, 10);
        registry.human_submission(&provisional.id, 30).unwrap();
        assert_eq!(
            registry
                .get(&promoted.id)
                .unwrap()
                .last_human_submission_at_ms,
            Some(30)
        );
        observed.session_ref = AgentSessionRef::id("replacement");
        registry.reconcile(&[observed], 40).unwrap();
        registry.human_submission(&provisional.id, 50).unwrap();
        let replacement = registry
            .records()
            .into_iter()
            .find(|record| {
                record
                    .session_ref
                    .as_ref()
                    .is_some_and(|session| session.value == "replacement")
            })
            .unwrap();
        assert_eq!(replacement.last_human_submission_at_ms, None);
    }

    #[test]
    fn post_replace_sync_failure_reports_uncertainty_and_keeps_committed_state_visible() {
        let path = temp_path("uncertain-sync");
        let mut registry = InboxRegistry::load(Some(path.clone()), "one".into());
        registry
            .reconcile(&[observation(Some("one"), 1, ExecutionState::Idle)], 10)
            .unwrap();
        let mut next = registry.clone();
        let id = sole(&registry).id;
        next.records.get_mut(&id).unwrap().disposition = Disposition::Settled;
        let result = registry.commit_with(next, |candidate| {
            candidate.persist()?;
            finish_commit_sync(Err(std::io::Error::other(
                "injected directory sync failure",
            )))
        });
        assert!(matches!(result, Err(InboxError::PersistenceUncertain(_))));
        assert_eq!(registry.get(&id).unwrap().disposition, Disposition::Settled);
        assert!(registry.writes_blocked);
        assert!(registry.recovery_error.is_some());
        let restored = InboxRegistry::load(Some(path.clone()), "two".into());
        assert_eq!(restored.get(&id).unwrap().disposition, Disposition::Settled);
        let record = registry.get(&id).unwrap();
        assert!(matches!(
            registry.mutate(&id, record.revision, ConversationAction::Reopen, 20),
            Err(InboxError::Recovery(_))
        ));
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn provisional_duplicate_merges_into_existing_identity_without_losing_human_event() {
        let mut registry = registry();
        let known = observation(Some("one"), 1, ExecutionState::Idle);
        registry
            .reconcile(std::slice::from_ref(&known), 10)
            .unwrap();
        let native = sole(&registry);
        registry
            .mutate(&native.id, native.revision, ConversationAction::Settle, 20)
            .unwrap();
        let mut unknown = observation(None, 2, ExecutionState::Idle);
        registry
            .reconcile(&[known.clone(), unknown.clone()], 30)
            .unwrap();
        let provisional = registry
            .records()
            .into_iter()
            .find(|record| record.session_ref.is_none())
            .unwrap();
        registry.human_submission(&provisional.id, 40).unwrap();
        unknown.session_ref = AgentSessionRef::id("one");
        registry.reconcile(&[known, unknown], 50).unwrap();
        let merged = sole(&registry);
        assert_eq!(merged.id, native.id);
        assert_eq!(merged.attachments.len(), 2);
        assert_eq!(merged.disposition, Disposition::Inbox);
        assert_eq!(merged.last_human_submission_at_ms, Some(40));
        registry.human_submission(&provisional.id, 60).unwrap();
        assert_eq!(sole(&registry).last_human_submission_at_ms, Some(60));
    }

    #[test]
    fn unknown_execution_stays_visible_without_erasing_disposition_and_failure_can_be_settled() {
        let mut registry = registry();
        let mut observed = observation(Some("one"), 1, ExecutionState::Idle);
        registry
            .reconcile(std::slice::from_ref(&observed), 10)
            .unwrap();
        let record = sole(&registry);
        registry
            .mutate(&record.id, record.revision, ConversationAction::Settle, 20)
            .unwrap();
        observed.execution = ExecutionState::Unknown;
        registry
            .reconcile(std::slice::from_ref(&observed), 30)
            .unwrap();
        assert_eq!(sole(&registry).section, ConversationSection::Inbox);
        assert_eq!(sole(&registry).disposition, Disposition::Settled);
        observed.execution = ExecutionState::Idle;
        registry
            .reconcile(std::slice::from_ref(&observed), 40)
            .unwrap();
        assert_eq!(sole(&registry).section, ConversationSection::Settled);
        observed.failure = true;
        registry
            .reconcile(std::slice::from_ref(&observed), 50)
            .unwrap();
        let failed = sole(&registry);
        assert_eq!(failed.section, ConversationSection::Inbox);
        assert_eq!(failed.disposition, Disposition::Inbox);
        registry
            .mutate(&failed.id, failed.revision, ConversationAction::Settle, 60)
            .unwrap();
        assert_eq!(sole(&registry).section, ConversationSection::Settled);
        assert!(!registry.reconcile(&[observed], 70).unwrap());
    }
}
