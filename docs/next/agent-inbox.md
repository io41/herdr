# Agent inbox in the custom fork

The agent inbox groups conversations by the attention they need. This feature belongs to the `io41/herdr` custom fork and starts disabled. Existing flat Agents navigation remains available. After isolated acceptance, use the global menu item **agent inbox / flat** to switch views. Section toggles and **conversation actions...** are available in the same menu.

## Conversation sections

| Section | Contents |
| --- | --- |
| Inbox | Completed work, requests for input, failures, and uncertain state. Reading clears unread attention without removing the conversation. |
| Working | Active execution without an outstanding request for the user. Ordinary completion returns to Inbox after a two-second grace. |
| Settled | Conversations explicitly put away, including those whose terminal panes have closed. |

Working and Settled start collapsed with visible counts. Section expansion and the selected presentation mode are client preferences. Settlement belongs to the conversation on its owning server.

## Open, settle, reopen, and resume

Open focuses the selected live attachment or shows saved details for a closed conversation. Opening does not settle or reopen it. When a conversation has several live attachments, choose the one to open.

Settle puts reviewed work in Settled without closing its terminal. Active execution, queued human work, and unresolved requests prevent settlement. Settle requires native session identity and known idle or closed state. Pending or uncertain resume also prevents settlement. Reopen returns the conversation to Inbox without starting an agent.

Resume explicitly continues the saved provider session. Herdr reuses a live attachment instead of starting a duplicate. A new launch remains pending until the provider reports the matching native session. Codex reports that identity on your next user turn. Open the reserved terminal to continue; Herdr does not send a prompt automatically. An uncertain result remains visible and blocks duplicate launch attempts. Herdr never silently substitutes a new conversation.

## Recover a closed session

The saved location identifies the original Space, tab, and working directory. If the original destination still exists, resume creates a pane there. If a tab or Space has been removed, choose whether to recreate it or use another location. Names and sidebar positions do not identify a destination.

A missing working directory requires an explicit valid replacement path. Resume does not recreate a removed worktree. An unavailable endpoint, unsupported provider, or missing native session produces a recovery message rather than a fresh session.

Closing a pane does not settle its conversation. Outstanding attention stays in Inbox with a Closed label. Settled history and the session navigator retain authoritative session records after closure and restart. Search the navigator by conversation title to find closed sessions. Provisional identities without a native session are not durable resume targets.

## Provider limits

Status detection and native session identity are different signals. A running process does not prove that an agent is working or waiting for input. Herdr shows the distinctions supported by the provider and keeps unknown state visible.

Terminal input does not automatically reopen settled conversations. Use explicit Reopen or Resume. Blocked status appears as Needs input; the current integration does not distinguish approvals, questions, or failures. Prompts submitted through the agent API reopen conversations after successful submission. A successful command submission does not prove that the provider accepted the requested native session.

Pins, snooze, automatic settlement, and worktree cleanup are outside this delivery.

## Test the local copy

Run `herdr-inbox` from a separate terminal. The local launcher uses its own configuration, state directory, and named session, with the inbox enabled. Your usual `herdr` command and existing sessions remain separate. Test sessions begin empty; the launcher does not copy your current workspaces, conversations, or Herdr plugins. Providers that use XDG configuration paths may need separate setup. Codex and Claude keep their usual configuration homes.

Start an agent normally and submit a task. Check Working, completion returning to Inbox, explicit Settle, closed-session recovery, and Resume. For Codex, continue with a prompt after Resume to confirm native session identity.

Detaching the client keeps the test server and its pane processes running. To stop them, run `herdr-inbox server stop`. This exits all test-pane processes in the isolated session.

## Agent skills

Existing Herdr pane commands and the installed `herdr` skill keep their current semantics. Pane `idle` or `done` status does not imply that a conversation is settled. No global `AGENTS.md` rule is needed for this user interface.

Agents that automate the new lifecycle need the fork's advertised `agent.conversation.list`, `agent.conversation.open`, `agent.conversation.settle`, `agent.conversation.reopen`, and `agent.conversation.resume` JSON methods. There are no corresponding conversation CLI subcommands. Gate these calls on endpoint method support and request settlement only when the user asks for it.

The bundled `skills/herdr/SKILL.md` remains aligned with the latest stable release, as required by the repository's documentation policy. Fork-specific instructions stay in this guide until stable release preparation.
