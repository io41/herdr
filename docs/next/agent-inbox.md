# Agent inbox in the custom fork

The agent inbox groups conversations by the attention they need. This feature belongs to the `io41/herdr` custom fork and starts disabled. Existing flat Agents navigation remains available. Use the **agent view** selector to switch views. Section toggles are in that selector; **conversation actions** opens the focused conversation menu.

## Conversation sections

| Section | Contents |
| --- | --- |
| Inbox | Completed work, requests for input, failures, and uncertain state. Reading clears unread attention without removing the conversation. |
| Working | Active execution without an outstanding request for the user. Ordinary completion returns to Inbox after a two-second grace. |
| Settled | Conversations explicitly put away, including those whose terminal panes have closed. |

Working and Settled start collapsed with visible counts. Section expansion and the selected presentation mode are client preferences. Settlement belongs to the conversation on its owning server.

## Identify a conversation

Each inbox row shows a meaningful conversation, pane, or tab title and a Space, tab, and pane breadcrumb. Numeric default tab names use the Space and pane as the fallback. Closed conversations show their last known location. The provider and complete status remain visible without repeating Local for a single local endpoint.

The view selector offers Inbox, All agents by space, and All agents by attention. Inbox groups by lifecycle. The two All agents views retain the configurable agent rows with different ordering.

## Open, settle, and restore

Open focuses a live conversation without changing its settlement state. Settle saves its native session reference and last location, then closes its sole agent process and pane. It may also remove an empty tab or Space through the normal pane-close behavior. Other panes remain running. Working agents, queued input, drafts, unanswered requests, unknown state, or ambiguous duplicate attachments prevent settlement; the action shows the reason.

Restore is the single action for settled or closed conversations. It reuses an existing attachment or starts the exact saved native session in a new pane, then focuses it. Herdr first matches durable location IDs, then unique existing Space and tab names. It creates only missing containers. Duplicate matching names or a missing working directory prompt for the specific missing choice rather than showing all recovery options in advance.

A new process remains unconfirmed until the provider reports the matching native session. Choose **Open restored session**, then submit your next Codex prompt to confirm the session identity. Herdr does not send a prompt automatically. An uncertain result blocks duplicate launch attempts. Herdr never silently substitutes a new conversation.

## Keyboard actions and restart

The default prefix is Ctrl+B. Your imported profile keeps Cmd+P for the session navigator and Ctrl+B then a for Latch. Conversation actions therefore use Ctrl+B then Shift+A.

| Keys | Action |
| --- | --- |
| Ctrl+B, then Shift+A | Conversation actions for the focused or selected conversation. |
| Ctrl+B, then i | View selector, including section expansion controls. |
| Cmd+P in your imported profile | Search Spaces, tabs, panes, and saved conversations. The stock default is Ctrl+B, then g. |
| Up/Down or j/k in a menu | Select an item. |
| Enter in a menu | Perform the selected available action. |
| s in conversation actions | Settle and close the eligible conversation. |
| r in conversation actions | Restore the conversation. |
| Esc | Close the menu without acting. |
| Ctrl+B, then q | Detach the client without stopping the server. |

The action and view shortcuts are client fallbacks; explicit configured bindings take precedence. Ordinary letters in an agent terminal remain agent input.

Installing a new binary does not replace an already running server. Finish active work, detach, and run these commands from a separate terminal outside Herdr. Stopping the server exits all of that session's pane processes:

```sh
herdr-inbox server stop
herdr-inbox
```

This restarts the default inbox session and restores its saved layout and resumable native conversations. For another named session, use `herdr-inbox --session NAME server stop`, then `herdr-inbox --session NAME`. Shell-job memory does not survive a restart.

## Provider limits

Status detection and native session identity are different signals. A running process does not prove that an agent is working or waiting for input. Herdr shows the distinctions supported by the provider and keeps unknown state visible.

Terminal input does not automatically reopen settled conversations. Use **Restore**. Blocked status appears as Needs input; the current integration does not distinguish approvals, questions, or failures. Prompts submitted through the agent API reopen conversations after successful submission. A successful command submission does not prove that the provider accepted the requested native session.

Exiting Codex does not automatically settle its conversation. Codex 0.166 reports `SessionEnd.reason` as `other`, including for shutdown and disconnected-thread unload. `SessionStart` does not distinguish a manual resume from an automatic restore. These signals cannot establish the user's intent. See the [Codex hooks documentation](https://learn.chatgpt.com/docs/hooks).

Pins, snooze, automatic settlement, and worktree cleanup are outside this delivery.

## Test the local copy

Run `herdr-inbox` from a separate terminal. The local launcher uses its own configuration, state directory, and named session, with the inbox enabled. Your usual `herdr` command and existing sessions remain separate. Normal settings and all installed Herdr plugins have been imported into isolated copies. Launching the test copy does not itself copy current workspaces or conversations; use the switcher below for session transfer. Providers that use XDG configuration paths may need separate setup. Codex and Claude keep their usual configuration homes.

The imported profile includes gruvbox, custom keys and widgets, Palette, pane movement, Latch, and all five installed plugins. Its files are separate copies. The original inbox configuration is backed up under `~/.config/herdr-inbox/herdr/.import-backup-20261103T101720`. Restart the inbox server to load the imported settings and plugins. Use `herdr-inbox` for configuration and plugin commands so they target the isolated profile.

Start an agent normally and submit a task. Check Working, completion returning to Inbox, explicit Settle, and Restore. For Codex, continue with a prompt after Restore to confirm native session identity.

Detaching the client keeps the test server and its pane processes running. To stop them, run `herdr-inbox server stop`. This exits all test-pane processes in the isolated session.

## Agent skills

Existing Herdr pane commands and the installed `herdr` skill keep their current semantics. Pane `idle` or `done` status does not imply that a conversation is settled. Explicit Settle closes the conversation pane; simply exiting a provider is not automatically treated as settlement. No global `AGENTS.md` rule is needed for this user interface.

Agents that automate the new lifecycle need the fork's advertised `agent.conversation.list`, `agent.conversation.open`, `agent.conversation.settle`, `agent.conversation.reopen`, `agent.conversation.resume`, `agent.conversation.settle_and_close`, and `agent.conversation.restore` JSON methods. The UI's **Settle** action calls `agent.conversation.settle_and_close`. The original `agent.conversation.settle` changes settlement state without closing the pane. The original resume method also retains its earlier semantics; the new UI uses `agent.conversation.restore` for automatic placement. There are no corresponding conversation CLI subcommands. Gate these calls on endpoint method support and request settlement only when the user asks for it.

The bundled `skills/herdr/SKILL.md` remains aligned with the latest stable release, as required by the repository's documentation policy. Fork-specific instructions stay in this guide until stable release preparation.

## Switch existing local sessions

The installed `herdr-inbox-switch` helper transfers local default and named sessions between your normal Herdr profile and the isolated inbox profile. It backs up both profiles before stopping servers or replacing session files. Normal `default` maps to inbox `agent-inbox`; other session names stay the same. Remote endpoints are not moved or stopped. Both profiles' local servers stop. Matching destination sessions are replaced after backup. Destination-only sessions remain saved but stopped.

Switching stops pane processes and shell jobs. Only agents with usable native session references can resume their conversations. Backups preserve Herdr metadata and saved screen history, not process memory, working-tree files, or independent copies of provider conversation stores. Messages sent while testing remain in the same provider histories after switching back.

1. Finish active work and open a separate terminal outside both Herdr instances. Do not launch either app while the switch runs.
2. Preview the transfer with `herdr-inbox-switch to-inbox`. This dry run does not write backups, change files, or stop servers.
3. Run `herdr-inbox-switch to-inbox --apply`. Review the warning and type `to-inbox` to confirm shutdown.
4. To return, preview with `herdr-inbox-switch to-stable`, then run `herdr-inbox-switch to-stable --apply` and type `to-stable`.

The helper verifies private, SHA-256 checked backups, live versus saved topology, native references, and actual server process exit. It then transfers saved state and restarts only sessions that were running in the source profile. Previously stopped sessions stay stopped. Repeating a completed direction is refused when every session in the inactive source is unchanged. If any source session has changed or reopened, a new switch transfers the whole source profile, including unchanged sessions. This is replacement after backup, not a per-session merge; review every mapped destination in the preview. In an interactive terminal, it attaches to a restarted default session, or the first restarted named session. It also prints attachment commands for the other sessions.

Backups and transaction journals remain under `~/.local/state/herdr-inbox-switch/backups/`. If interrupted, use the exact transaction directory printed by the command:

Choose one recovery action. To finish the transfer:

```sh
herdr-inbox-switch --resume "/path/to/transaction" --apply
```

To restore verified original profile data and restart original sessions instead:

```sh
herdr-inbox-switch --rollback "/path/to/transaction" --apply
```

Recovery proceeds only when ownership and contents still match. Neither command overwrites unrelated changes or stops a replacement server. If recovery refuses because state changed, preserve the backups and follow the reported manual recovery information.

Herdr 0.9.3 drops the fork's durable location fields when it saves a snapshot. The untouched backups preserve those fields, but after a round trip some closed conversations may require an explicit location choice. Native session references are preserved. Symlinked or ambiguous session data is refused rather than followed or merged.

## Protect the fork from upstream updates

Inbox builds reject upstream binary self-updates even when invoked directly. The local launcher also exports `HERDR_DISABLE_UPDATES=1`, and its configuration disables background version and manifest checks. The switcher sets the same environment lock when starting inbox sessions and does not copy the normal update configuration into that profile.

For other builds containing this guard, `HERDR_DISABLE_UPDATES=1` or `HERDR_DISABLE_UPDATES=true` disables binary self-update and automatic version checks. Inbox builds remain locked even if the variable is unset or set to `0`. Update the test copy by rebuilding the fork, not by running `herdr update`.
