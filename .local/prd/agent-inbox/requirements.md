# Agent inbox requirements

This is the approved behavior contract. Requirement IDs apply to the custom fork implementation, not existing Herdr commands.

## Sections and scope

The final section order is Pinned, Inbox, Working, Snoozed, Settled. Pinned and Inbox start expanded. Working, Snoozed, and Settled start collapsed with counts visible. Empty sections can be omitted, but their saved expansion preferences must survive. Pinned remains deferred. The current Snooze delivery uses a dedicated Snoozed picker in the view selector and retains the three sidebar section preferences for compatibility.

| ID | Requirement |
| --- | --- |
| R1 | Explicit UI Settle durably saves the conversation and closes its sole eligible agent and pane. Preserve unrelated processes and containers; closing the last pane may remove an empty tab or Space. Retain names and native identity for Restore. |
| R2 | Keep execution state, read acknowledgement, and conversation disposition independent. Viewing a completed result clears unread attention without settling it. Browsing Settled does not reopen the conversation. |
| R3 | Project active execution into Working. Return ordinary completion to Inbox even when the pane was visible during completion. A new approval or question returns immediately. |
| R4 | Settle is explicit, reversible, and durable. Reopen returns the conversation to Inbox. Reject Settle during active execution, queued or unconfirmed human input, or an unresolved blocking request. Checked session persistence and generation tombstones must prevent a settled pane from reappearing after restart. |
| R5 | Collapse Working and Settled by default. Persist expansion preferences per client. Disposition is shared by clients connected to the owning endpoint. |
| R6 | Inbox sorts newest attention return first. Working sorts latest real human submission first. Settled sorts newest settlement first. Break equal timestamps by stable conversation identity. Reading and autonomous continuation do not reorder Inbox. |
| R7 | A verified new human submission reopens settled work. Provider events must distinguish this from automatic continuation. If that signal is unavailable, require explicit Reopen and disclose the limitation. |
| R8 | Preserve disposition across pane moves, detach, reconnect, and restart when conversation identity is authoritative. A new conversation in an old pane must not inherit settlement. Ambiguous identity remains visible. |
| R9 | Mouse targeting, numbered shortcuts, next/previous navigation, and displayed row order use the same projection. Headers are not agent targets. If the selected row disappears, choose the next visible agent, then the previous one, or no agent. Background projection changes do not move terminal focus. Explicit Open and Restore focus their target pane. |
| R10 | Keep hidden work recoverable through expanded sections, an All picker, and existing Spaces navigation. Endpoint failure leaves its rows visible and marked unavailable. Never infer completion or settlement from a disconnect. |
| R11 | Unknown execution or identity must not silently hide work. Preserve a visible fallback when the server lacks the capability. |
| R12 | Notifications and attention counts follow the lifecycle projection. Reading suppresses an unread indicator, not pending-work membership. Restore must not replay historical completion notifications. |
| R13 | Existing flat Agents mode remains selectable. Default the new mode off until the isolated acceptance walkthrough passes. Disabling it restores the ordinary view without deleting lifecycle records. |

## Conversation actions and recovery

A row represents a conversation, including when its terminal is closed. Show its saved Space, tab, and working directory. Session identity is independent of the saved location.

| ID | Requirement |
| --- | --- |
| R18 | Open focuses an existing attachment or shows saved details for a closed conversation. Opening never changes disposition. Multiple attachments require an explicit attachment choice. |
| R19 | Restore is the single UI action for settled or closed conversations. Reuse an existing attachment or launch the exact saved session, and focus that pane. Unconfirmed native attachment remains distinct. Legacy Reopen and Resume API methods keep their prior semantics. |
| R20 | Restore first uses durable location identity, then unique saved Space and displayed tab names as a placement preference. Reuse the matching tab, otherwise create only missing containers. Ask for a destination only when matching names are ambiguous. Never infer conversation identity from a name or silently use an unrelated current tab. |
| R21 | A missing directory or worktree requires an explicit valid replacement path. Never recreate a worktree automatically. Disconnected endpoints, unsupported resume, missing sessions, and ambiguous identity remain visible with recovery information. |
| R22 | A definitive resume failure preserves disposition and reports the error. An uncertain launch projects to Inbox as Status unknown. Reconcile existing attachments before allowing a retry so a timeout cannot create duplicate execution. |
| R23 | Closed conversations remain searchable in Settled and All conversations. Retain native identity and saved location across pane closure and restart. Do not persist provisional identities as resumable history. |
| R24 | Inbox labels describe the available evidence: Done, Needs input, Approval required, Failed, or Status unknown. Use provider-specific labels only when that distinction is supported. Reading clears unread attention but preserves membership. |

For ordinary completion, hold the row in Working for two seconds to avoid flicker during automatic continuation. A fresh request, failure, or explicit placement bypasses this grace. Cancel the pending completion notification if work resumes during grace.

## Second delivery

| ID | Requirement |
| --- | --- |
| R14 | Pins keep conversations above Inbox, including while working. Pinning clears settlement or snooze. Settle removes the pin. Preserve stable pin order. |
| R15 | Snooze clears any pin and stores an absolute wake deadline. Ordinary completion and autonomous continuation stay snoozed. Deadline expiry or explicit Wake restores Inbox disposition, including when no TUI was attached at expiry. Active execution then projects to Working unless a human request or failure requires attention. |
| R16 | A fresh authoritative human request, failure, or human submission wakes Snoozed early. Old errors and duplicate events do not. Expose provider capability limits rather than claiming unsupported wake behavior. |
| R17 | Undo restores the prior disposition, pin, and deadline only if no intervening mutation changed the conversation revision. Failed or stale Undo leaves current state intact. |

## Exclusions and decisions

- Do not add inactivity-based or PR-based automatic settlement in either delivery. Pins therefore have no automatic-settlement policy in this scope.
- Settle intentionally closes its sole conversation pane after checked persistence. A matching native Codex shutdown hook followed by release of that same process settles its conversation without closing the remaining shell pane. Other pane closures, process disappearance, and disconnects preserve visible work. Preserve the conversation and saved location after closure.
- Do not add provider switching, remote orchestration, PR monitoring, scheduling prompts, or automatic worktree cleanup.
- Do not change status detection to make the sidebar appear correct. Report unsupported semantic signals as capability limits.
- If duplicate pane attachments resolve to the same authoritative conversation, show one conversation row with an attachment picker. Never choose a target attachment from sidebar position alone.

## Codex shutdown and manual resume

| ID | Requirement |
| --- | --- |
| R25 | Settle automatically only after a fresh exact Codex SessionEnd hook and matching process release. Bind evidence to the current process acquisition, native identity, and terminal generation. A crash without the hook, endpoint shutdown, replacement process, stale event, or hook alone does not settle. Keep the remaining shell pane. |
| R26 | A provider-confirmed resume of the exact settled session reopens it when the process was launched outside Herdr managed restore. Handle either hook/detector ordering and preserve settlement during automatic startup restore. Native confirmation may be lazy until a user turn. |
| R27 | Add the shutdown report through a separate JSON method. Preserve existing method shapes and frozen codecs. Register and uninstall the new Codex hook without replacing unrelated hooks; require the normal Codex trust flow. |
