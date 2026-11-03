# Agent inbox in the custom fork

The agent inbox groups conversations by the attention they need. This feature belongs to the `io41/herdr` custom fork and starts disabled. Existing flat Agents navigation remains available. Use the **agent view** selector to switch views. Section toggles are in that selector; **conversation actions** opens the focused conversation menu.

## Conversation sections

| Section | Contents |
| --- | --- |
| Inbox | Completed work, requests for input, failures, and uncertain state. Reading clears unread attention without removing the conversation. |
| Working | Active execution without an outstanding request for the user. Ordinary completion returns to Inbox after a two-second grace. |
| Settled | Conversations put away explicitly or after a confirmed Codex shutdown. |

Working and Settled start collapsed with visible counts. Section expansion and the selected presentation mode are client preferences. Settlement belongs to the conversation on its owning server.

## Identify a conversation

Each inbox row shows a meaningful conversation, pane, or tab title and a Space, tab, and pane breadcrumb. Numeric default tab names use the Space and pane as the fallback. Closed conversations show their last known location. The provider and complete status remain visible without repeating Local for a single local endpoint.

The view selector offers Inbox, All agents by space, and All agents by attention. Inbox groups by lifecycle. The two All agents views retain the configurable agent rows with different ordering.

## Open, settle, and restore

Open focuses a live conversation without changing its settlement state. Settle saves its native session reference and last location, then closes its sole agent process and pane. It may also remove an empty tab or Space through the normal pane-close behavior. Other panes remain running. Working agents, queued input, drafts, unanswered requests, unknown state, or ambiguous duplicate attachments prevent settlement; the action shows the reason.

Restore resumes a settled or closed conversation with one action. It reuses an existing attachment or starts the exact saved native session in a new pane, then focuses it. Herdr first matches durable location IDs, then unique existing Space and tab names. It creates only missing containers. Duplicate matching names or a missing working directory prompt for the specific missing choice rather than showing all recovery options in advance.

Closed conversations still in Inbox also offer Settle. This puts away the saved conversation without restarting its agent.

A new process remains unconfirmed until the provider reports the matching native session. Choose **Open restored session**, then submit your next Codex prompt to confirm the session identity. Herdr does not send a prompt automatically. An uncertain result blocks duplicate launch attempts. Herdr never silently substitutes a new conversation.

## Snooze and wake

Choose **Snooze** in conversation actions, then choose 1 hour, 4 hours, or 24 hours. Snooze temporarily hides the conversation from the inbox and suppresses its completion notifications. It keeps the process, input, and native session running. Ordinary completion does not end the snooze.

Open the view selector and choose **Snoozed…** to find deferred conversations and their remaining time. Choose **Wake** to bring one back early. The regular navigator also retains snoozed conversations. Unknown or unavailable state remains visible so connection trouble cannot silently hide work.

The owning server saves the wake deadline and expires it even without an attached TUI. Snoozes survive restart. Expiry, explicit Wake, a verified human submission, or a newly observed blocking request or failure clears the deadline. The resulting conversation appears in Inbox or Working according to its current execution state. Provider status does not distinguish repeated requests while an agent remains continuously blocked, so those requests cannot reliably end a snooze early.

The JSON API accepts durations from one minute through 30 days. The server computes the absolute deadline from its own clock.

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
| z in conversation actions | Choose a snooze duration. |
| w in conversation actions | Wake a snoozed conversation. |
| Esc | Close the menu without acting. |
| Ctrl+B, then q | Detach the client without stopping the server. |

The action and view shortcuts are client fallbacks; explicit configured bindings take precedence. Ordinary letters in an agent terminal remain agent input.

Installing a new binary does not replace an already running server. Finish active work, detach, and run these commands from a separate terminal outside Herdr. Stopping the server exits all of that session's pane processes:

```sh
herdr-inbox server stop
herdr-inbox
```

This restarts the default inbox session and restores its saved layout and resumable native conversations. For another named session, use `herdr-inbox --session NAME server stop`, then `herdr-inbox --session NAME`. Shell-job memory does not survive a restart.

Use `herdr-inbox status server` to check the running version. `herdr-inbox --version` reports the installed binary, which can be newer than the server. New actions require the updated server.

## Provider limits

Status detection and native session identity are different signals. A running process does not prove that an agent is working or waiting for input. Herdr shows the distinctions supported by the provider and keeps unknown state visible.

Blocked status appears as Needs input; the current integration does not distinguish approvals, questions, or failures. Prompts submitted through the agent API reopen conversations after successful submission. A successful command submission does not prove that the provider accepted the requested native session.

Codex exit settlement requires the updated Herdr integration and a trusted **SessionEnd** hook. Herdr accepts the shutdown signal only for the exact provider-confirmed session, then waits for that process to release its terminal. A shutdown signal alone, process disappearance alone, a replacement process, or an endpoint restart does not settle a conversation. The remaining shell pane stays open. Failed or uncertain shutdown evidence leaves the conversation visible.

Codex 0.166 reports `SessionEnd.reason` as `other`, including for shutdown, archive, and disconnected-thread unload. Herdr combines that event with process release; it cannot prove a zero exit status. Manual resume reopens the matching settled conversation after the provider confirms the native session, which Codex may delay until the next user turn. Herdr's automatic startup restore does not count as a manual resume. See the [Codex hooks documentation](https://learn.chatgpt.com/docs/hooks).

Run `herdr-inbox integration install codex` to install the updated integration. This updates the usual Codex home shared with the normal profile and preserves unrelated hooks. Then use `/hooks` in a new Codex session to review and trust the new or changed Herdr hooks. Codex skips untrusted hooks, so exit settlement remains unavailable until they are trusted. This does not retroactively settle conversations that closed before the hook was installed.

Pins and worktree cleanup are outside this delivery.

## Test the local copy

Run `herdr-inbox` from a separate terminal. The local launcher uses its own configuration, state directory, and named session, with the inbox enabled. Your usual `herdr` command and existing sessions remain separate. Normal settings and all installed Herdr plugins have been imported into isolated copies. Launching the test copy does not itself copy current workspaces or conversations; use the switcher below for session transfer. Providers that use XDG configuration paths may need separate setup. Codex and Claude keep their usual configuration homes.

The imported profile includes gruvbox, custom keys and widgets, Palette, pane movement, Latch, and all five installed plugins. Chezmoi manages the durable normal and inbox configuration, helpers, plugin registrations, and inbox launchers. Inbox templates reuse the normal source while keeping separate configuration, state, and cache paths and the update lock. Sessions, sockets, logs, caches, backups, provider credentials, and compiled binaries remain outside that configuration inventory. The original inbox configuration is backed up under `~/.config/herdr-inbox/herdr/.import-backup-20261103T101720`. Use `herdr-inbox` for configuration and plugin commands so they target the isolated profile.

The current dotfile additions and their existing Latch source dependencies remain uncommitted locally and have not been pushed. Chezmoi source registration is separate from a Git backup of that source.

Start an agent normally and submit a task. Check Working, completion returning to Inbox, explicit Settle, and Restore. For Codex, continue with a prompt after Restore to confirm native session identity.

Detaching the client keeps the test server and its pane processes running. To stop them, run `herdr-inbox server stop`. This exits all test-pane processes in the isolated session.

## Agent skills

Existing Herdr pane commands and the installed `herdr` skill keep their current semantics. Pane `idle` or `done` status does not imply that a conversation is settled. Explicit Settle closes the conversation pane. Codex shutdown settlement follows the provider limits above. No global `AGENTS.md` rule is needed for this user interface.

Agents that automate the new lifecycle need the fork's advertised `agent.conversation.list`, `agent.conversation.open`, `agent.conversation.settle`, `agent.conversation.reopen`, `agent.conversation.resume`, `agent.conversation.settle_and_close`, and `agent.conversation.restore` JSON methods. The UI's **Settle** action calls `agent.conversation.settle_and_close`. The original `agent.conversation.settle` changes settlement state without closing the pane. The original resume method also retains its earlier semantics; the new UI uses `agent.conversation.restore` for automatic placement. There are no corresponding conversation CLI subcommands. Gate these calls on endpoint method support and request settlement only when the user asks for it.

The bundled `skills/herdr/SKILL.md` remains aligned with the latest stable release, as required by the repository's documentation policy. Fork-specific instructions stay in this guide until stable release preparation.

The fork also advertises `agent.conversation.snooze` and `agent.conversation.wake`. Snooze takes `conversation_id`, `expected_revision`, and `duration_ms`; Wake takes the ID and expected revision. These methods do not close a pane or submit input. If an endpoint does not advertise these methods, the client disables Snooze and Wake while retaining its other supported actions.

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
