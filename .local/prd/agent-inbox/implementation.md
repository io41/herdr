# Implementation and validation

Deliver the first release before adding timed Snooze, pins, and Undo. Each work package must leave a reviewable, verifiable state. Keep the feature disabled by default until acceptance passes.

## Work packages

1. **Characterize the baseline.** Record client/server versions inside an authorized Herdr pane and pin the implementation commit. Identify existing status, focus acknowledgement, restore, and protocol tests. In an isolated named test session, establish which Codex events distinguish human submission, autonomous continuation, approval, question, completion, and failure. Document missing capabilities.
2. **Add identity and lifecycle.** Introduce a pure reducer and versioned persistence near the agent state modules. Reuse native identity and resume handling from `src/agent_resume.rs`. Test provisional identity, duplicate attachments, moves, retries, conflicts, persistence failure, and restart recovery.
3. **Add closed-session recovery.** Retain authoritative conversations independently of live panes. Persist durable Space and tab identity separately from native session identity. Implement explicit Open, Reopen, and Resume, missing-location choices, directory validation, attachment selection, and uncertain-launch recovery.
4. **Expose shared actions.** Add negotiated capabilities, neutral methods near `src/api/schema/agents.rs` and `src/app/api/agents.rs`, and optional lifecycle projections/events. Keep frozen endpoint contracts unchanged. Test older endpoint fallback and independent endpoint failure.
5. **Build the grouped Agents view.** Update `src/client/shell/agent_sidebar.rs` and the corresponding projection, actions, mouse, and aggregate-navigation paths. Preserve Spaces. Derive all input targets and visible ordering from one projection. Add the flat-mode switch and All recovery picker.
6. **Align attention and notifications.** Integrate read acknowledgement, completion grace, request precedence, unavailable rows, and deduplicated notifications. Verify viewing never settles. Keep unsupported automatic reactivation disabled and visible in the capability description.
7. **Validate the first release.** Run repository checks, protocol compatibility tests, rendering/navigation tests, performance checks, and a real Codex walkthrough. Produce a custom build with a reversible installation plan. Do not enable it in an existing working session before the isolated trial passes.
8. **Add the second delivery.** Implement durable pins, endpoint-owned Snooze scheduling, early wake, and revision-checked Undo. Extend the reducer and compatibility tests before adding controls. Gate early wake by verified provider capabilities.

## Acceptance matrix

| Scenario | Required result |
| --- | --- |
| Prompt, run, complete | Working starts collapsed. Completion returns to Inbox after grace, including while its pane is visible. |
| Completion, then view | Unread clears; Inbox membership remains. |
| View, Settle, browse, Reopen | Settle durably saves and closes the sole eligible agent/pane. Browsing keeps it settled. Restore reuses or creates the required location and focuses the restored pane. |
| Running turn, queued human work, or blocking request | Settle is rejected without partial mutation or process interruption. |
| Approval or question while execution remains active | Return immediately to Inbox, or stay visibly Pinned if pinned. Bypass Working and completion grace. |
| Continuation within grace | Remain in Working and suppress the pending completion notification. |
| New human prompt on Settled | Reopen only with an authoritative submission signal. Otherwise require Reopen and expose the limitation. |
| Move, detach, restart, reattach | Preserve disposition for the same native conversation. A replacement conversation does not inherit it. |
| Late identity or duplicate attachments | Reconcile deterministically, show one conversation with selectable attachments, and leave ambiguous records visible. |
| Conflicting clients and retries | Reject stale mutations unless their postcondition already holds. Do not change timestamps on a no-op retry. |
| Lost endpoint, unknown state, or invalid registry | Show a recoverable Inbox row and report the condition. Do not silently settle or discard metadata. |
| Collapse, filter, or remove selected row | Mouse, numbered shortcuts, and adjacent navigation agree. Headers are never targets. Terminal focus stays unchanged. |
| Disable new mode | Restore flat Agents navigation without deleting lifecycle records. |
| Restore existing snapshot | Do not replay old completion alerts. |
| Older endpoint or mixed endpoint versions | Disable unsupported actions locally while retaining usable compatible connections. |
| Close pane, restart, Resume | Retain the conversation and resume its exact provider session in the saved location. |
| Original tab, Space, or directory removed | Reuse uniquely matching existing names, create only missing containers, and prompt only for a missing directory or ambiguous destination. Never silently use an unrelated tab or create a worktree. |
| Resume timeout after process launch | Show Status unknown in Inbox and reconcile attachments before retry. Never launch a duplicate automatically. |
| Closed conversation with pending attention | Keep it in Inbox marked Closed; closure never settles it. |
| Browse settled closed conversation | Show saved details without starting an agent or changing disposition. |
| Second delivery: snooze through completion | Keep hidden until deadline or a supported early-wake event. |
| Second delivery: expiry while detached or restarted | Wake once after restoring the deadline, with no TUI required. |
| Second delivery: old error, duplicate event, Wake/expiry race | Do not wake from stale evidence or overwrite newer disposition. |
| Second delivery: pin and undo | Pin remains visible during execution. Undo rejects intervening revisions. |
| Second delivery: pinned, then Snooze, then expiry | Snooze clears the pin. Expiry returns to Inbox, or Working if execution is active. Revision-valid Undo of Snooze restores the previous pin position. |

## Evidence required for completion

- Pure reducer and persistence tests cover state transitions, interrupted writes, schema errors, and identity replacement.
- Fake-clock tests cover grace, deadline expiry, restart, time changes, and daylight-saving input in the second delivery.
- Render and input tests cover narrow terminals, section counts, selection fallback, multiple endpoints, and keyboard/mouse agreement.
- Frozen codec fixtures, digests, wire-tag tests, and `tests/fixtures/endpoint-method-shapes-v1.json` remain valid without changing generation-1 expectations.
- Run `just check` and relevant protocol tests for the implementation. Run `just bench-render-scale` at one and at least fifteen populated panes and record the scaling delta where applicable.
- In an authorized Herdr pane, run the real Codex walkthrough in an isolated named test session. Do not control an existing user session from outside Herdr. A passing simulation or build alone does not prove provider signals or UI behavior.

Implementation requires the checks above in addition to whitespace, relative-link, and source-reference checks. Record runtime limitations and acceptance results separately; a build alone does not establish acceptance.

## Additional revised UX acceptance

- Generic titles fall back to meaningful tab, Space, and pane context. Duplicate tab titles remain distinguishable and closed records show last-known location.
- The header contains one working view selector with no leftover flat/grouped text. Keyboard-only entry and modal Settle/Restore shortcuts are covered; unprefixed typing remains terminal input.
- Settle refuses queued/unconfirmed input, active work, unresolved requests, ambiguous attachments, and failed persistence without killing a process.
- Crash-before-debounce recovery prunes exact retired generations before spawning agents, keeps replacements and legacy nonclosing settlement, and preserves valid surviving screen history.
- Restore reuses existing Space/tab names when old durable IDs are unavailable, creates only missing containers, and focuses the exact restored pane before success is reported to the requesting client.
