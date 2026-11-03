use std::collections::HashMap;
use std::sync::{mpsc, Arc, Mutex};
use std::time::Instant;

use crossterm::event::{KeyCode, KeyEventKind, MouseEventKind};

use crate::detect::AgentState;
use crate::pty::actor::InputSubmissionReceipt;
use crate::raw_input::RawInputEvent;

#[derive(Default)]
pub(crate) struct HumanInputState {
    draft: bool,
    submitted: Option<(Instant, Option<Instant>)>,
    next_submission: u64,
    queued: HashMap<u64, QueuedSubmission>,
}

struct QueuedSubmission {
    uncertain_draft: bool,
    receipt: InputSubmissionReceipt,
    working_at: Option<Instant>,
}

impl HumanInputState {
    pub(crate) fn pending(&self) -> bool {
        self.draft || self.submitted.is_some() || !self.queued.is_empty()
    }

    pub(crate) fn record_bytes(&mut self, bytes: &[u8]) {
        for event in crate::raw_input::parse_framed_input(bytes) {
            match event {
                RawInputEvent::Key(key) if key.kind == KeyEventKind::Release => {}
                RawInputEvent::Key(key)
                    if key.code == KeyCode::Enter && key.modifiers.is_empty() =>
                {
                    // Empty Enter can submit recalled/provider-owned text that Herdr cannot see.
                    self.draft = false;
                    self.submitted = Some((Instant::now(), None));
                }
                RawInputEvent::Key(key)
                    if matches!(
                        key.code,
                        KeyCode::Left
                            | KeyCode::Right
                            | KeyCode::Up
                            | KeyCode::Down
                            | KeyCode::Home
                            | KeyCode::End
                            | KeyCode::PageUp
                            | KeyCode::PageDown
                    ) && key.generated_text.is_none() => {}
                RawInputEvent::Text(text) if text.as_str().is_empty() => {}
                RawInputEvent::Paste(text) if text.is_empty() => {}
                RawInputEvent::Mouse(mouse)
                    if matches!(
                        mouse.kind,
                        MouseEventKind::Down(_) | MouseEventKind::Up(_) | MouseEventKind::Drag(_)
                    ) =>
                {
                    self.draft = true;
                }
                // shortcut: terminal editing cannot prove a draft was erased; release after submission completes or the process exits.
                RawInputEvent::Key(_)
                | RawInputEvent::Text(_)
                | RawInputEvent::Paste(_)
                | RawInputEvent::Unsupported => self.draft = true,
                _ => {}
            }
        }
    }

    pub(crate) fn queue(
        &mut self,
        receiver: mpsc::Receiver<std::io::Result<()>>,
        state: Arc<Mutex<Self>>,
        has_text: bool,
        receipt: InputSubmissionReceipt,
    ) -> UserInputCompletion {
        self.next_submission += 1;
        let submission = self.next_submission;
        self.queued.insert(
            submission,
            QueuedSubmission {
                uncertain_draft: self.draft || has_text,
                receipt,
                working_at: None,
            },
        );
        self.draft = false;
        UserInputCompletion {
            receiver,
            state,
            submission,
        }
    }

    pub(crate) fn observe(&mut self, state: AgentState, observed_at: Instant) {
        self.queued.retain(|_, queued| {
            let Some(delivered) = queued.receipt.delivered_at() else {
                return true;
            };
            if observed_at <= delivered {
                return true;
            }
            match state {
                AgentState::Working => {
                    queued.working_at = Some(
                        queued
                            .working_at
                            .map_or(observed_at, |prior| prior.max(observed_at)),
                    );
                    true
                }
                AgentState::Idle => queued
                    .working_at
                    .is_none_or(|working| observed_at <= working),
                _ => true,
            }
        });
        let Some((submitted_at, saw_working)) = &mut self.submitted else {
            return;
        };
        if observed_at <= *submitted_at {
            return;
        }
        match state {
            AgentState::Working => {
                *saw_working =
                    Some(saw_working.map_or(observed_at, |prior| prior.max(observed_at)));
            }
            AgentState::Idle if saw_working.is_some_and(|working_at| observed_at > working_at) => {
                self.submitted = None;
            }
            _ => {}
        }
    }

    pub(crate) fn clear(&mut self) {
        self.draft = false;
        self.submitted = None;
        self.queued.clear();
    }
}

/// Resolves only this queued submission, preserving later input and other queued prompts.
pub struct UserInputCompletion {
    receiver: mpsc::Receiver<std::io::Result<()>>,
    state: Arc<Mutex<HumanInputState>>,
    submission: u64,
}

impl UserInputCompletion {
    pub fn recv(&self) -> Result<std::io::Result<()>, mpsc::RecvError> {
        let result = self.receiver.recv();
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(queued) = state.queued.remove(&self.submission) {
            if matches!(result, Ok(Ok(()))) {
                if let Some(delivered) = queued.receipt.delivered_at() {
                    if state
                        .submitted
                        .is_none_or(|(submitted, _)| submitted <= delivered)
                    {
                        state.submitted = Some((delivered, queued.working_at));
                    }
                } else {
                    state.draft = true;
                }
            } else {
                // A failed writer may have delivered text before its Enter failed.
                state.draft |= queued.uncertain_draft;
            }
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_stays_pending_through_stale_idle_and_releases_after_work() {
        let mut state = HumanInputState::default();
        let old_sample = Instant::now();
        state.record_bytes(b"\x1b[A\x1b[D\x1b[I\x1b[O");
        assert!(!state.pending());
        state.record_bytes(b"\x1b[200~prompt\x1b[201~");
        assert!(state.pending());
        state.observe(AgentState::Idle, Instant::now());
        assert!(state.pending());
        state.record_bytes(b"\r");
        state.observe(AgentState::Working, old_sample);
        state.observe(AgentState::Idle, Instant::now());
        assert!(state.pending());
        let working = state.submitted.unwrap().0 + std::time::Duration::from_millis(1);
        state.observe(AgentState::Working, working);
        state.observe(
            AgentState::Idle,
            working + std::time::Duration::from_millis(1),
        );
        assert!(!state.pending());
    }

    #[test]
    fn idle_observed_before_working_cannot_complete_the_submission() {
        let mut state = HumanInputState::default();
        state.record_bytes(b"task\r");
        let old_idle = Instant::now();
        let working = old_idle + std::time::Duration::from_millis(1);
        let completion = working + std::time::Duration::from_millis(1);
        state.observe(AgentState::Working, working);
        state.observe(AgentState::Idle, old_idle);
        assert!(state.pending());
        state.observe(AgentState::Idle, completion);
        assert!(!state.pending());
    }

    #[test]
    fn equal_clock_ticks_cannot_prove_submission_or_work_completion() {
        let mut state = HumanInputState::default();
        state.record_bytes(b"task\r");
        let submitted = state.submitted.unwrap().0;
        state.observe(AgentState::Working, submitted);
        state.observe(
            AgentState::Idle,
            submitted + std::time::Duration::from_millis(1),
        );
        assert!(state.pending());
        let working = submitted + std::time::Duration::from_millis(2);
        state.observe(AgentState::Working, working);
        state.observe(AgentState::Idle, working);
        assert!(state.pending());
        state.observe(
            AgentState::Idle,
            working + std::time::Duration::from_millis(1),
        );
        assert!(!state.pending());
    }

    #[test]
    fn completion_preserves_a_draft_typed_during_work() {
        let mut state = HumanInputState::default();
        state.record_bytes(b"prompt\r");
        let working = state.submitted.unwrap().0 + std::time::Duration::from_millis(1);
        state.observe(AgentState::Working, working);
        state.record_bytes(b"next prompt");
        state.observe(
            AgentState::Idle,
            working + std::time::Duration::from_millis(1),
        );
        assert!(state.submitted.is_none());
        assert!(state.pending());
    }

    #[test]
    fn unsupported_trailing_input_and_modified_enter_preserve_drafts() {
        for bytes in [
            b"\x1b[A\xff".as_slice(),
            b"\x1b[200~unfinished",
            b"draft\x1b[13;2u",
        ] {
            let mut state = HumanInputState::default();
            state.record_bytes(bytes);
            state.observe(AgentState::Working, Instant::now());
            state.observe(AgentState::Idle, Instant::now());
            assert!(state.pending(), "unproven draft was discarded: {bytes:?}");
        }
    }

    #[test]
    fn forwarded_mouse_buttons_are_uncertain_but_wheel_and_motion_are_navigation() {
        let mut state = HumanInputState::default();
        state.record_bytes(b"\x1b[<64;1;1M\x1b[<35;1;1M");
        assert!(!state.pending());
        for report in [
            b"\x1b[<0;1;1M".as_slice(),
            b"\x1b[<0;1;1m",
            b"\x1b[<32;1;1M",
        ] {
            state.record_bytes(report);
            assert!(state.pending());
            state.clear();
        }
    }

    #[test]
    fn delayed_submission_receiver_preserves_observed_work_and_completion() {
        for completed_before_receive in [false, true] {
            let state = Arc::new(Mutex::new(HumanInputState::default()));
            let receipt = InputSubmissionReceipt::default();
            let (reply, receiver) = mpsc::channel();
            let completion =
                state
                    .lock()
                    .unwrap()
                    .queue(receiver, state.clone(), true, receipt.clone());
            receipt.mark_delivered();
            reply.send(Ok(())).unwrap();
            let working = receipt.delivered_at().unwrap() + std::time::Duration::from_millis(1);
            let idle = working + std::time::Duration::from_millis(1);
            state.lock().unwrap().observe(AgentState::Working, working);
            if completed_before_receive {
                state.lock().unwrap().observe(AgentState::Idle, idle);
                assert!(!state.lock().unwrap().pending());
            }
            completion.recv().unwrap().unwrap();
            assert_eq!(state.lock().unwrap().pending(), !completed_before_receive);
            state.lock().unwrap().observe(AgentState::Idle, idle);
            assert!(!state.lock().unwrap().pending());
        }
    }

    #[test]
    fn queued_submission_ignores_work_before_its_delivery_receipt() {
        let state = Arc::new(Mutex::new(HumanInputState::default()));
        let receipt = InputSubmissionReceipt::default();
        let (reply, receiver) = mpsc::channel();
        let completion =
            state
                .lock()
                .unwrap()
                .queue(receiver, state.clone(), true, receipt.clone());
        let before_delivery = Instant::now();
        state
            .lock()
            .unwrap()
            .observe(AgentState::Working, before_delivery);
        state.lock().unwrap().observe(
            AgentState::Idle,
            before_delivery + std::time::Duration::from_millis(1),
        );
        assert!(state.lock().unwrap().pending());
        receipt.mark_delivered();
        reply.send(Ok(())).unwrap();
        completion.recv().unwrap().unwrap();
        let working = receipt.delivered_at().unwrap() + std::time::Duration::from_millis(2);
        state.lock().unwrap().observe(AgentState::Idle, working);
        assert!(state.lock().unwrap().pending());
        state.lock().unwrap().observe(AgentState::Working, working);
        state.lock().unwrap().observe(
            AgentState::Idle,
            working + std::time::Duration::from_millis(1),
        );
        assert!(!state.lock().unwrap().pending());
    }

    #[test]
    fn queued_failure_clears_only_its_reservation_and_exit_invalidates_old_completion() {
        let state = Arc::new(Mutex::new(HumanInputState::default()));
        let (failed_tx, failed_rx) = mpsc::channel();
        let failed = state.lock().unwrap().queue(
            failed_rx,
            state.clone(),
            false,
            InputSubmissionReceipt::default(),
        );
        let (next_tx, next_rx) = mpsc::channel();
        let next = state.lock().unwrap().queue(
            next_rx,
            state.clone(),
            false,
            InputSubmissionReceipt::default(),
        );
        failed_tx
            .send(Err(std::io::Error::other("refused")))
            .unwrap();
        assert!(failed.recv().unwrap().is_err());
        assert!(state.lock().unwrap().pending());
        state.lock().unwrap().clear();
        next_tx.send(Ok(())).unwrap();
        next.recv().unwrap().unwrap();
        assert!(!state.lock().unwrap().pending());
    }
}
