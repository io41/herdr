# Agent inbox design

All types, methods, and module names proposed here are design targets, not existing APIs. Revalidate source locations against the implementation base before coding.

## State ownership

The endpoint server owns conversation identity, disposition, revisions, timestamps, pending requests, and wake deadlines. The client owns section expansion, width, selection, and rendering. Expose shared facts through advertised JSON methods and optional snapshot/event data. Do not implement shared lifecycle solely through the private TUI socket.

Keep execution as existing runtime data. Working is a projection, not a persisted disposition. Read acknowledgement stays separate and preserves existing per-client behavior.

```rust
struct ConversationKey {
    endpoint: EndpointIdentity,
    provider: ProviderIdentity,
    native_session: NativeSessionIdentity,
}

enum Disposition {
    Inbox,
    Settled { settled_at: SystemTime },
    Snoozed { wake_at: SystemTime }, // second delivery
}

struct ConversationLifecycle {
    disposition: Disposition,
    pinned_position: Option<u64>, // second delivery
    attention_returned_at: SystemTime,
    last_human_submission_at: Option<SystemTime>,
    revision: u64,
}
```

Use endpoint identity, provider identity, and native conversation identity together. Titles, working directories, pane IDs, and sidebar indices are not durable conversation keys. Until native identity arrives, use a boot-scoped terminal-generation key. Do not persist that provisional binding across restarts. Reconcile it when identity arrives; a conflict leaves the conversation visible and disables ambiguous durable actions.

One lifecycle record belongs to a conversation, even with multiple attachments. Store attachment selection separately from conversation disposition. Events and actions must name their owning endpoint.

## Commands and concurrency

Proposed neutral methods are `agent.conversation.settle`, `agent.conversation.reopen`, and, in the second delivery, `agent.conversation.snooze`, `agent.conversation.wake`, and `agent.conversation.pin`.

Every mutation carries conversation identity and an expected revision. The server checks eligibility, applies the change, persists it atomically, then emits the updated projection. Failed persistence must not report success. A stale revision returns a conflict with current state. On a repeated command, check the requested postcondition first: if it already holds, return the current state without changing timestamps or revision. Otherwise enforce the revision check. This makes retries converge without overwriting intervening changes.

| Trigger | State change and visibility |
| --- | --- |
| Verified human submission | Clear settlement or snooze, record submission time, enter Working when execution starts. |
| Autonomous continuation | Update execution only. Preserve explicit settlement or snooze and preserve human-submission ordering. |
| Completion from Working | For Inbox disposition, return after grace and record attention return once. Preserve Snoozed or Settled. |
| New blocking request or failure | Bypass grace. Return to Inbox and record attention time. Clear snooze when the event is fresh and authoritative. |
| View result | Acknowledge read state only. |
| Settle | If eligible, persist Settled and clear pin or snooze. Keep the pane and conversation. |
| Un-settle or Wake | Persist Inbox and update attention time. Existing active execution projects to Working. |
| Snooze | Clear the pin, persist the absolute deadline, and preserve execution. Undo retains the prior pin position in its revision-checked snapshot. |
| Snooze deadline | Apply Wake once if the stored deadline and revision still match. |
| Disconnect or uncertain status | Preserve disposition and expose an unavailable or uncertain recovery row. |

Deduplicate events using provider event identity or monotonic source revision. A duplicate completion must not update attention time. Compare early-wake events with the snooze boundary so old requests cannot wake newly deferred work. When Settle is eligible but execution starts concurrently, serialize the action and event through the endpoint's authoritative state owner.

## Projection and persistence

Project unavailable or uncertain recovery rows into Inbox first. Then project fresh blocking requests and failures into Pinned if pinned, otherwise Inbox, even if execution is still active. Such events clear settlement or snooze through the authoritative transition before projection. As a defensive fallback, an actionable request must never remain hidden while that transition is being reconciled. For other rows, use Snoozed, Settled, Pinned, Working, then Inbox precedence. Recovery rows show their saved disposition, and inspection must not mutate it. Pinned rows stay visible during execution. Explicit settlement and snooze are cleared only by the defined actions or authoritative wake events.

Persist a versioned registry independently of pane snapshots. Use atomic replacement and recover the last valid data after an interrupted write. Missing data defaults to Inbox. Unsupported or corrupt data must produce a recoverable error and visible rows rather than overwriting the registry silently. Preserve lifecycle metadata when the UI mode is disabled. Downgrades must leave unknown data untouched.

The second delivery adds an endpoint-owned scheduler. Restore deadlines and reconcile overdue wakes before accepting lifecycle mutations. Use persisted wall-clock deadlines with monotonic in-process waits. Reconcile wall-clock changes. Explicit Wake, expiry, and early wake must converge under duplicate delivery. Input local times in the client, resolve ambiguous daylight-saving times explicitly, and transmit an absolute deadline.

## Protocol and performance

Generation-1 named endpoint codecs are frozen. Do not append fields or enum variants reachable from them or update fixtures to bless a changed wire format. Prefer new advertised methods and optional JSON projections. If a binary codec change is necessary, negotiate a new named codec and keep the old fallback.

An older endpoint disables unsupported actions and retains flat navigation. One endpoint's failure must not disconnect other endpoints. Scope presentation preferences by client and lifecycle keys by endpoint.

Compute the projection once for rendering and navigation. Keep filesystem access, process inspection, snapshot formatting, and durable writes out of pane-scaled render loops. Reuse the repository's pure view and rendering boundaries. Measure fixed geometry with one and at least fifteen populated panes before enabling the feature.
