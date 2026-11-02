# Agent attention inbox

Status: implemented in the `io41/herdr` custom fork, with the new view disabled by default. See [verification results](evidence.md#implementation-verification) for completed checks and remaining environment limits.

Replace the flat Agents list with an inbox that brings completed work and human requests into view. Keep running work collapsed and let the user explicitly settle reviewed conversations. Preserve Spaces, tabs, panes, terminal processes, and workspace membership.

Read these documents in order:

1. [Requirements](requirements.md) defines behavior and delivery scope.
2. [Design](design.md) defines state ownership, identity, transitions, and compatibility constraints.
3. [Implementation and validation](implementation.md) defines ordered work packages and acceptance checks.
4. [Evidence](evidence.md) records source references, alternatives, and known signal gaps.

The first delivery includes Inbox, Working, Settled, explicit Settle and Reopen, durable closed-session history, exact-session resume with location recovery, restart recovery, and consistent navigation. Pins, timed Snooze, and Undo follow in a second delivery. Full parity requires authoritative provider signals; terminal process liveness cannot establish whether an agent is waiting for the user or for autonomous work.

These documents live under `.local/prd/` as required by the repository's planning convention. Only this specification directory is deliberately tracked in the custom fork. The rest of `.local/` remains ignored. This branch does not propose an upstream issue or pull request.
