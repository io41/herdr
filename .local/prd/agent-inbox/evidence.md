# Evidence and feasibility

Research began November 1, 2026 and was rechecked November 2. The historical upstream source used for research is `ee5bfb5041bb3e4a49d591c72308aae3c81ecdde`. The local executable reported 0.9.5 on November 2. An earlier observation of 0.8.0 is historical, not the current installed baseline. Running client and server versions still require an authorized live inspection.

## Why this requires a core change

The [agent view schema](https://github.com/herdrdev/herdr/blob/ee5bfb5041bb3e4a49d591c72308aae3c81ecdde/src/api/schema/agents.rs#L70-L79) supports `source`, `label`, `filter`, and `sort`. It does not define sections. The [plugin contract](https://github.com/herdrdev/herdr/blob/ee5bfb5041bb3e4a49d591c72308aae3c81ecdde/docs/next/website/src/content/docs/plugins.mdx) excludes native non-terminal plugin UI.

A plugin can approximate settlement with namespaced metadata and filter one flat list. It can provide actions and a recovery picker. The [view API contract](https://github.com/herdrdev/herdr/blob/ee5bfb5041bb3e4a49d591c72308aae3c81ecdde/docs/next/website/src/content/docs/socket-api.mdx) describes a single transient view owner. Filtering does not change global attention counts or notifications. Plugin startup hooks are one-shot commands; durable timed Snooze needs additional supervised scheduling and recovery.

A separate plugin dashboard could draw groups in a terminal pane, but it would not replace the native Agents area. The chosen approach is therefore server-owned conversation lifecycle plus native grouped client rendering.

## T3 behavior used as reference

The reference T3 source commit is `8760d063e903406c8e8f695b0852c0a15fc1e00f`. It describes source behavior, not a verified installed binary.

- [Sidebar section logic](https://github.com/pingdotgg/t3code/blob/8760d063e903406c8e8f695b0852c0a15fc1e00f/apps/web/src/components/Sidebar.logic.ts) defines section precedence and attention presentation.
- [Inbox projection and tests](https://github.com/pingdotgg/t3code/blob/8760d063e903406c8e8f695b0852c0a15fc1e00f/packages/client-runtime/src/state/threadInbox.ts) defines Working, attention return, ordering, and completion grace. Tests are adjacent in `threadInbox.test.ts`.
- [Settlement and snooze projection](https://github.com/pingdotgg/t3code/blob/8760d063e903406c8e8f695b0852c0a15fc1e00f/packages/client-runtime/src/state/threadSettled.ts) separates explicit disposition from execution.
- [Sidebar guide](https://github.com/pingdotgg/t3code/blob/8760d063e903406c8e8f695b0852c0a15fc1e00f/docs/user/thread-sidebar.md) documents Settle, Snooze, recovery, and section behavior.

Reading is separate from settlement. Working is distinct from awaiting the user. Ordinary snoozed completion stays deferred. These are the behaviors carried into the fork specification. Optional T3 cleanup, automatic settlement, and orchestration features are outside scope.

The reference guide and server disagree about whether pins prevent automatic settlement. The fork avoids this ambiguity by excluding automatic settlement from the planned deliveries.

## Herdr constraints and unresolved evidence

- [Agent detection and state](https://github.com/herdrdev/herdr/blob/ee5bfb5041bb3e4a49d591c72308aae3c81ecdde/src/pane/agent_detection.rs) and [agent sidebar projection](https://github.com/herdrdev/herdr/blob/ee5bfb5041bb3e4a49d591c72308aae3c81ecdde/src/client/shell/agent_sidebar.rs) are starting points for baseline characterization. Herdr's seen/unseen completion distinction must not stand in for explicit settlement.
- [Native resume handling](https://github.com/herdrdev/herdr/blob/ee5bfb5041bb3e4a49d591c72308aae3c81ecdde/src/agent_resume.rs) supplies identity and restoration context. Pane IDs alone are insufficient for durable lifecycle.
- [Project architecture and protocol rules](https://github.com/herdrdev/herdr/blob/ee5bfb5041bb3e4a49d591c72308aae3c81ecdde/AGENTS.md) require server/client separation, pure rendering, frozen endpoint codecs, and scaling evidence.
- Approval, question, failure, human submission, and awaited autonomous work need provider-specific evidence. Do not infer them from process existence or incidental terminal text. Unsupported signals remain explicit implementation blockers for the affected automatic behavior, not permission to fabricate status.

The recovered work inspected source and existing tests. It did not run those tests, install a plugin, build a patch, or verify sidebar parity live. A T3 desktop inspection attempt on November 2 timed out. Implementers must complete the acceptance work rather than treating these research findings as runtime proof.

## Implementation verification

The implementation worktree is `~/Code/gh/herdr-worktrees/agent-inbox` on `feat/agent-inbox`, based on `d537a0906f5b0ced4a1014d0ab1c1eef5507c883`. The implementation is reviewed in the isolated worktree and delivered through its branch history. The existing installed binary, running user session, and global agent configuration are not replaced by the side-by-side test installation.

Final validation uses the repository-pinned Rust 1.96.1 and Zig 0.16.0 in a temporary task directory. The earlier baseline suite used Homebrew Rust 1.99 with Zig 0.16.0, so the full-suite comparison is not compiler-controlled. The Homebrew compiler versions do not match this checkout's pins.

- Formatting and Clippy with warnings denied pass. The final targeted run passes 473 tests covering the complete client surface, lifecycle, resume, native identity, API schema, and protocol compatibility. Frozen generation-1 fixtures remain unchanged.
- The unchanged baseline runs 3,932 tests: 3,867 pass and 65 fail. The complete integration run has 3,929 passing tests and 64 failures out of 3,993. Of those failures, 61 also fail on the baseline. The other three exposed menu and narrow-layout regressions; their fixes pass the final 473-test rerun. This is not a clean full-suite result.
- Six architecture tests, the integration asset checks, and documentation contract checks pass. Python and Bun maintenance checks are blocked by this account's inability to create files under `/var/tmp`. Windows cross-validation is blocked by the missing SDK configuration.
- The enabled inbox renderer at fixed 120×40 geometry measures 89,381 ns per frame with one pane and 99,713 ns with fifteen panes, a 1.12× ratio. The broader render benchmark passes six profiles. Its native-graphics profile fails an assertion that also reproduces on the unchanged baseline.
- Real Codex 0.166.0 checks pass working, completion grace, Inbox return, explicit settlement, viewing without reopening, closed history, restart persistence, revision conflicts, and missing-location recovery. The resumed CLI loads the same transcript. Codex emits SessionStart on the next user turn; a deliberate test prompt then confirms the same native session and reopens the original conversation. The product never submits a prompt automatically to obtain that signal.

An isolated actual TUI check also passes: Inbox, Working, Settled, and a closed conversation received through the optional server companion render in the client. The test client and server are stopped. Artifacts remain under `/private/tmp/herdr-inbox-tui-1793626200/`.

Detailed command output remains under `/private/tmp/herdr-toolchain/`. The real-provider acceptance artifacts are under `/private/tmp/herdr-inbox-resume-turn-1793625829/`. The local decision trail is `.local/prd/agent-inbox/decisions.tsv` and remains outside the tracked change.


## November 3 UX and safe-close validation

The sidebar now uses meaningful titles and saved Space, tab, and pane breadcrumbs. The view selector replaces the stale flat/grouped header. The UI uses the new advertised `agent.conversation.settle_and_close` and `agent.conversation.restore` methods; legacy method semantics and generation-1 fixtures remain unchanged.

Settle checkpoints the tagged layout, persists the closed conversation and retired terminal generations, then closes its exact eligible pane. Restore reuses durable IDs or unique displayed names, creates only missing containers, and focuses the resulting pane. Raw input, actual delivery, native startup, revisions, duplicate attachments, and uncertain identity guard destructive close. Snapshot pruning preserves unrelated screen history and refuses invalid provenance.

Validation uses pinned Rust 1.96.1 and Zig 0.16.0:

- Final affected-path run: 741 passed. The final narrow-row and keyboard run: 36 passed. Final snapshot/input run: 46 passed. Unix actor run: 25 passed. These runs overlap and must not be summed as unique tests.
- `just lint` passes. Generated API schema check passes after regeneration. Frozen endpoint fixtures and `skills/herdr/SKILL.md` are unchanged.
- Full native run: 3,993 passed, 62 failed, 15 skipped. Of the 62 failures, 61 are also present in the saved baseline. One runner could not execute its test binary. A concurrent rebuild is the suspected cause; the complete 46-test snapshot/input rerun passes. The saved baseline uses a different compiler, so this is not a controlled before/after comparison. The full suite is not green.
- UI hot-path architecture: 6 passed. Authorized integration asset and documentation contract checks pass. Maintenance: 168 passed and 10 release-test setup errors because `/var/tmp` is not writable by this user. Release workflows: 4 passed and one `/var/tmp` setup failure. Windows lint cannot run without the Windows SDK. No system directory permissions or SDK license state were changed.
- Credential-free API fixture `/private/tmp/herdr-toolchain/ux-placement-smoke.py` passes automatic Space/tab reuse, new-pane focus, duplicate prevention, and preserving pending startup and an unrelated shell.
- Real TUI fixture `/private/tmp/herdr-toolchain/ux-tui-smoke.py` passes. ANSI replay in `/private/tmp/huit-51gji4gg/` verifies the saved conversation, compact breadcrumb, clean header, and both keyboard view-selector labels.
- Fixed 120x40 debug renderer: one pane 631,405 ns/frame; fifteen panes 836,436 ns/frame, 1.32x. This measures within-run scaling, not a controlled comparison with earlier measurements.
- A fresh live Codex round trip remains unverified because startup requires folder trust. A proposed temporary-home test that copied credentials was rejected by automatic approval review and was not executed. The replacement fixtures use no copied credentials.

Normal gruvbox settings, custom bindings, widgets, Palette, pane movement, Latch, and all five plugins are staged in the isolated inbox profile. `herdr-inbox config check` passes, and the actual plugin list reports all five enabled. The original inbox profile is backed up privately. The running user server has not been reloaded or restarted; plugin startup behavior needs the user's restart. The launcher pins configuration, cache, state, and binary paths without editing `.zshrc`.

Automatic settlement on Codex exit and automatic reopening on manual resume remain deferred. Codex 0.166's documented SessionEnd reason is `other`, and SessionStart cannot establish manual intent. Snooze and pins remain outside this delivery.

Independent GPT-6.1-Sol high review found and reproduced an acknowledgement-ordering defect. Actual writer delivery receipts and delayed-receiver regressions address it. The follow-up safety review found no supported data-loss issue. A separate prose review checked the fork guide.


## November 3 follow-up: Settle guard, Snooze, and native shutdown

Live inspection reproduced two idle conversations, Update DB obs docs and --tui flag scope, with `queued_human_work=true`. The DB docs conversation retained that flag after the user quit Codex and the record became Closed with no attachments. No Settle API request reached the server when the disabled menu item was selected. The exact earlier input event was not logged, so it cannot be attributed to a particular key.

The exact-source input harness fails before the plain-Escape fix and passes afterward while preserving existing drafts and submissions. Closed-runtime reconciliation and registry loading now clear stale queued work. Disabled Settle reports pending input separately from active execution and displays a notice when attempted.

Snooze uses an optional JSON deadline and unchanged disposition/section enum values. The server owns duration validation, persistence, expiry, and early wake. The client hides snoozed rows and completion notifications, retains All navigation, exposes a Snoozed picker and Wake, and uses local time only for countdown text. A visible countdown repaints at minute boundaries.

Codex integration version 11 follows version 10 in released v0.9.5. Its separate `pane.report_agent_session_end` method requires the exact native ID, transcript path, sequence, and producer timestamp. Transient end evidence is bound to current process acquisition and matched release. Shutdown, stale producer data, replacement processes, duplicate attachments, and absent native end evidence cannot settle other work. Manual-resume evidence handles either hook/detector ordering, survives transient write failures, and yields to later successful explicit Settle or Snooze. The remaining shell pane stays open.

Validation uses pinned Rust 1.96.1 and Zig 0.16.0:

- The broad affected-path run executes 1,132 tests: 1,131 pass and one old test expects integration version 10. Replacing that expectation with the version constant is followed by a 200-test integration, lifecycle, and Snooze run with 200 passing. These runs overlap and are not 1,332 unique tests. Logs: `inbox-lifecycle-focused-final.log` and `inbox-lifecycle-corrections.log` in `/private/tmp/herdr-toolchain/`.
- All 14 native lifecycle route tests pass, including real persistence obstruction and recovery. An earlier fixture backdated process acquisition before the previous exit and correctly triggered stale-process protection. The fixture now uses fresh observations and waits for a later producer millisecond; production guards are unchanged.
- Final `just lint` passes. Generated schema passes. Frozen codecs, generation-1 method fixtures, and bundled stable skill are unchanged.
- Required `just check` passes formatting and Clippy, then stops on the known `/var/tmp` graphics-directory permission failure. Maintenance has 168 passing cases and 10 release-test setup errors for the same unwritable directory. UI hot-path checks, all three integration asset suites, and documentation contracts pass. Windows validation remains blocked by the missing SDK.
- `/private/tmp/herdr-toolchain/snooze-api-smoke.py` passes real JSON API Snooze, explicit Wake, server restart persistence, and deadline expiry without a TUI. It uses a synthetic native history record and no provider credentials.
- The real 140x40 TUI fixture passes sidebar width 36, hidden Inbox membership, Snoozed count and picker, countdown, actual z-to-duration Snooze, actual w-to-Wake, and corresponding server state. Captures and cleanup evidence are under `/private/tmp/herdr-toolchain/snooze-tui-yvr74s_p/`.
- The final shell hook fixture verifies unchanged SessionStart reporting, dedicated SessionEnd reporting, bounded producer timestamps, and rejection of ephemeral, nested, unsupported, or unknown-reason input. Evidence: `/private/tmp/herdr-codex-hook-fixture-result.txt`.
- Fixed 120x40 debug rendering measures 644,188 ns/frame for one pane and 830,650 for fifteen, a 1.29x within-run ratio. This is not a controlled comparison with a different build or earlier host load.

The live inbox configuration is set to 36 columns and its reload returns applied with no diagnostics. No saved manual width overrides it. Chezmoi recursively verifies both profiles and both inbox launchers, with empty targeted diffs. Normal durable configuration hashes remain unchanged. Inbox templates depend on existing uncommitted normal Latch sources; all these dotfile additions remain in the source working tree rather than a separate incomplete Git commit. The existing rendered Latch tests have 65 passes and one cold-PTY startup latency failure under host load; no Latch code was changed for this task.

Independent GPT-6.1-Sol high review found shutdown, process-generation, event-ordering, and manual-resume persistence issues. The fixes have regression coverage, and the final focused review reports no remaining findings in its scope. A separate prose review checked the user guide.

Native automation still requires installing and trusting the new Codex hook. SessionEnd plus release is a graceful-shutdown signal, not proof of exit status zero. Provider hook trust is not bypassed. A fresh real Codex UI quit/resume round trip remains unverified until normal hook trust is granted. Older already-closed records are not retroactively inferred as settled. The active user server is not stopped or restarted by this work.
