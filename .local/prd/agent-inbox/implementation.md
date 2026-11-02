# Implementation and validation

Deliver the first release before adding timed Snooze, pins, and Undo. Each work package must leave a reviewable, verifiable state. Keep the feature disabled by default until acceptance passes.

## Work packages

1. **Characterize the baseline.** Record client/server versions inside an authorized Herdr pane and pin the implementation commit. Identify existing status, focus acknowledgement, restore, and protocol tests. In an isolated named test session, establish which Codex events distinguish human submission, autonomous continuation, approval, question, completion, and failure. Document missing capabilities.
2. **Add identity and lifecycle.** Introduce a pure reducer and versioned persistence near the agent state modules. Reuse native identity and resume handling from `src/agent_resume.rs`. Test provisional identity, duplicate attachments, moves, retries, conflicts, persistence failure, and restart recovery.
3. **Expose shared actions.** Add negotiated capabilities, neutral methods near `src/api/schema/agents.rs` and `src/app/api/agents.rs`, and optional lifecycle projections/events. Keep frozen endpoint contracts unchanged. Test older endpoint fallback and independent endpoint failure.
4. **Build the grouped Agents view.** Update `src/client/shell/agent_sidebar.rs` and the corresponding projection, actions, mouse, and aggregate-navigation paths. Preserve Spaces. Derive all input targets and visible ordering from one projection. Add the flat-mode switch and All recovery picker.
5. **Align attention and notifications.** Integrate read acknowledgement, completion grace, request precedence, unavailable rows, and deduplicated notifications. Verify viewing never settles. Keep unsupported automatic reactivation disabled and visible in the capability description.
6. **Validate the first release.** Run repository checks, protocol compatibility tests, rendering/navigation tests, performance checks, and a real Codex walkthrough. Produce a custom build with a reversible installation plan. Do not enable it in an existing working session before the isolated trial passes.
7. **Add the second delivery.** Implement durable pins, endpoint-owned Snooze scheduling, early wake, and revision-checked Undo. Extend the reducer and compatibility tests before adding controls. Gate early wake by verified provider capabilities.

## Acceptance matrix

| Scenario | Required result |
| --- | --- |
| Prompt, run, complete | Working starts collapsed. Completion returns to Inbox after grace, including while its pane is visible. |
| Completion, then view | Unread clears; Inbox membership remains. |
| View, Settle, browse, Un-settle | Settle moves the conversation to Settled without closing its pane. Browsing keeps it settled. Un-settle returns it. |
| Running turn, queued human work, or blocking request | Settle is rejected without partial mutation or process interruption. |
| Approval or question while execution remains active | Return immediately to Inbox, or stay visibly Pinned if pinned. Bypass Working and completion grace. |
| Continuation within grace | Remain in Working and suppress the pending completion notification. |
| New human prompt on Settled | Reopen only with an authoritative submission signal. Otherwise require Un-settle and expose the limitation. |
| Move, detach, restart, reattach | Preserve disposition for the same native conversation. A replacement conversation does not inherit it. |
| Late identity or duplicate attachments | Reconcile deterministically, show one conversation with selectable attachments, and leave ambiguous records visible. |
| Conflicting clients and retries | Reject stale mutations unless their postcondition already holds. Do not change timestamps on a no-op retry. |
| Lost endpoint, unknown state, or invalid registry | Show a recoverable Inbox row and report the condition. Do not silently settle or discard metadata. |
| Collapse, filter, or remove selected row | Mouse, numbered shortcuts, and adjacent navigation agree. Headers are never targets. Terminal focus stays unchanged. |
| Disable new mode | Restore flat Agents navigation without deleting lifecycle records. |
| Restore existing snapshot | Do not replay old completion alerts. |
| Older endpoint or mixed endpoint versions | Disable unsupported actions locally while retaining usable compatible connections. |
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

This specification-only branch requires whitespace checks, relative-link checks, source-reference review, and a clean documentation-only diff. It does not require a Rust build or claim runtime acceptance.
