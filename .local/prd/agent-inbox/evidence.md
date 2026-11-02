# Evidence and feasibility

Research began November 1, 2026 and was rechecked November 2. Herdr upstream HEAD and this branch's base are `ee5bfb5041bb3e4a49d591c72308aae3c81ecdde`. The local executable reported 0.9.5 on November 2. An earlier observation of 0.8.0 is historical, not the current installed baseline. Running client and server versions still require an authorized live inspection.

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
