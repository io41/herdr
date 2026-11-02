# Agent inbox requirements

This is a proposed behavior contract. Requirement IDs apply to the custom fork implementation, not existing Herdr commands.

## Sections and scope

The final section order is Pinned, Inbox, Working, Snoozed, Settled. Pinned and Inbox start expanded. Working, Snoozed, and Settled start collapsed with counts visible. Empty sections can be omitted, but their saved expansion preferences must survive. The first delivery omits Pinned and Snoozed until their actions and persistence are implemented.

| ID | Requirement |
| --- | --- |
| R1 | Preserve Spaces, workspace membership, tabs, pane geometry, and terminal ownership. Settling never closes, moves, or sends input to a pane. |
| R2 | Keep execution state, read acknowledgement, and conversation disposition independent. Viewing a completed result clears unread attention without settling it. Browsing Settled does not reopen the conversation. |
| R3 | Project active execution into Working. Return ordinary completion to Inbox even when the pane was visible during completion. A new approval or question returns immediately. |
| R4 | Settle is explicit, reversible, and durable. Un-settle returns the conversation to Inbox. Reject Settle during active execution, queued human work, or an unresolved blocking request. |
| R5 | Collapse Working and Settled by default. Persist expansion preferences per client. Disposition is shared by clients connected to the owning endpoint. |
| R6 | Inbox sorts newest attention return first. Working sorts latest real human submission first. Settled sorts newest settlement first. Break equal timestamps by stable conversation identity. Reading and autonomous continuation do not reorder Inbox. |
| R7 | A verified new human submission reopens settled work. Provider events must distinguish this from automatic continuation. If that signal is unavailable, require explicit Un-settle and disclose the limitation. |
| R8 | Preserve disposition across pane moves, detach, reconnect, and restart when conversation identity is authoritative. A new conversation in an old pane must not inherit settlement. Ambiguous identity remains visible. |
| R9 | Mouse targeting, numbered shortcuts, next/previous navigation, and displayed row order use the same projection. Headers are not agent targets. If the selected row disappears, choose the next visible agent, then the previous one, or no agent. Do not move terminal focus automatically. |
| R10 | Keep hidden work recoverable through expanded sections, an All picker, and existing Spaces navigation. Endpoint failure leaves its rows visible and marked unavailable. Never infer completion or settlement from a disconnect. |
| R11 | Unknown execution or identity must not silently hide work. Preserve a visible fallback when the server lacks the capability. |
| R12 | Notifications and attention counts follow the lifecycle projection. Reading suppresses an unread indicator, not pending-work membership. Restore must not replay historical completion notifications. |
| R13 | Existing flat Agents mode remains selectable. Default the new mode off until the isolated acceptance walkthrough passes. Disabling it restores the ordinary view without deleting lifecycle records. |

## Second delivery

| ID | Requirement |
| --- | --- |
| R14 | Pins keep conversations above Inbox, including while working. Pinning clears settlement or snooze. Settle removes the pin. Preserve stable pin order. |
| R15 | Snooze clears any pin and stores an absolute wake deadline. Ordinary completion and autonomous continuation stay snoozed. Deadline expiry or explicit Wake restores Inbox disposition, including when no TUI was attached at expiry. Active execution then projects to Working unless a human request or failure requires attention. |
| R16 | A fresh authoritative human request, failure, or human submission wakes Snoozed early. Old errors and duplicate events do not. Expose provider capability limits rather than claiming unsupported wake behavior. |
| R17 | Undo restores the prior disposition, pin, and deadline only if no intervening mutation changed the conversation revision. Failed or stale Undo leaves current state intact. |

For ordinary completion, hold the row in Working for two seconds to avoid flicker during automatic continuation. A fresh request, failure, or explicit placement bypasses this grace. Cancel the pending completion notification if work resumes during grace.

## Exclusions and decisions

- Do not add inactivity-based or PR-based automatic settlement in either delivery. Pins therefore have no automatic-settlement policy in this scope.
- Retain terminal panes. A historical conversation catalogue after pane closure is separate work.
- Do not add provider switching, remote orchestration, PR monitoring, scheduling prompts, or automatic worktree cleanup.
- Do not change status detection to make the sidebar appear correct. Report unsupported semantic signals as capability limits.
- If duplicate pane attachments resolve to the same authoritative conversation, show one conversation row with an attachment picker. Never choose a target attachment from sidebar position alone.
