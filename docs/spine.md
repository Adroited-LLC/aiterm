# The spine — one live event stream for every harness

The spine is how a conversation reaches the phone (and, later, anything
else that wants to watch a session) as it happens. Every engine feeds it
through an adapter; every consumer reads one vocabulary. Adding a harness
means writing one adapter. Nothing above it changes.

This replaces the phone's poll of `GET /v1/sessions/{id}/conversation`
every 3 s, which re-parsed the whole transcript and swapped the whole list.

## Vocabulary

ACP-shaped (agentclientprotocol.com `session/update`), trimmed to what the
phone renders. Rust types are the source of truth: `src-tauri/src/spine/mod.rs`.

Every event on the wire:

```json
{
  "seq": 42,                 // per session, from 1, assigned by the registry
  "epoch": 1788390000123,    // registry start (ms). Changes when the desktop restarts.
  "session_id": "…",
  "agent": "claude",         // the engine id
  "ts": 1788390012345,       // ms; the source's own timestamp when it has one
  "kind": "agent_text",      // one of the kinds below
  … kind fields …
}
```

Kinds:

| kind | fields | semantics |
|---|---|---|
| `user_message` | `id`, `text` | The person (or a relay) said something. |
| `agent_text` | `id`, `text`, `done` | Assistant prose. `text` is the FULL text of this block so far, never a delta. Upsert by `id`. `done:false` means more may come for this id. |
| `agent_thought` | `id`, `text`, `done` | Reasoning, same rules as `agent_text`. |
| `tool_call` | `id`, `tool`, `title`, `category`, `input`, `status` | A tool was invoked. Appears the moment the call is issued. `input` is a one-line summary, clipped. |
| `tool_call_update` | `id`, `status`, `output` | Status moved; `output` (clipped, optional) is the result when there is one. Upsert by `id`. |
| `turn_started` | `turn` | A turn opened (a person spoke; the engine is going to answer). |
| `turn_ended` | `turn`, `reason` | `completed` \| `interrupted` \| `error` \| `unknown`. |
| `phase` | `phase`, `detail` | `working` \| `needs_you` \| `idle`. Status, not content. `detail` is human text ("running Bash", "permission: Edit foo.rs"). |
| `reset` | — | History was rebuilt (a `/clear`, a file replaced). The phone drops everything it holds for this session and fetches from `after=0`. |

`category` ∈ `read` \| `edit` \| `execute` \| `search` \| `fetch` \| `think` \| `other`.
`status` ∈ `pending` \| `running` \| `completed` \| `failed` \| `cancelled`.

Ids are stable across re-reads of the same source (a message uuid + block
index for Claude, a tool call id where the engine has one, a line ordinal
where it does not). A consumer that sees an id twice replaces, never appends.

## Endpoints

- `GET /v1/sessions/{id}/spine?after=N` →
  `{ "epoch": u64, "live": bool, "events": [ SpineEvent… ] }`
  Everything after `seq` N (N=0 for all). Calling it registers interest:
  the registry starts (or keeps) the adapter tail for that session.
  `live` is false when the session is served by the legacy adapter
  (an engine with no native adapter yet: the conversation is re-derived
  from `conversation_rich` on a slow poll and diffed into events).
- The existing WebSocket `/v1/events` now also carries
  `{"type":"spine", …SpineEvent fields flattened…}` for every session with
  a running tail. The phone ignores sessions it is not looking at.

## Client rule (the phone)

1. On opening a session: `GET …/spine?after=0`, apply all, remember `epoch`
   and `lastSeq`.
2. On a `spine` WS event for that session: if `epoch` differs → go to 1.
   If `seq == lastSeq + 1` → apply. Otherwise → `GET …/spine?after=lastSeq`
   and apply what comes back (dedupe by `seq`).
3. Apply = upsert by `id` for `agent_text`/`agent_thought`/`tool_call`/
   `tool_call_update`; append for `user_message`; `reset` → clear and go to 1.
4. Never rebuild the list from scratch on a normal event. Rows are keyed by
   id so a growing block re-renders in place.
5. On WS reconnect: `GET …/spine?after=lastSeq`.

## Registry lifecycle (desktop)

- `Spine` lives in Tauri managed state. One `SessionLog` per session:
  bounded ring (last 5 000 events or 4 MB), `next_seq`, adapter handle,
  `last_interest`.
- A tail starts when: a phone asks (`GET …/spine`), or a tab binds the
  session (`TabRegistry`), or a relay starts on it.
- A tail stops when: no tab is bound AND no interest for 15 minutes.
- Adapter driver: `bootstrap()` once (history → events, seq assigned in
  order), then `poll()` on every change of any `watch_paths()` file
  (notify, 250 ms coalesce) and on a 2 s fallback tick.
- Phase events from the terminal (OSC progress / bell) are bridged onto
  the spine by the registry so a consumer has one stream.

## Adapter contract

```rust
pub trait Adapter: Send {
    /// Full history, in order. Called once when the tail starts.
    fn bootstrap(&mut self) -> Vec<(u64, Kind)>;   // (ts ms, kind)
    /// Everything new since the last call. Cheap: read from an offset.
    fn poll(&mut self) -> Vec<(u64, Kind)>;
    /// Files whose change should trigger `poll()`.
    fn watch_paths(&self) -> Vec<PathBuf>;
}
pub fn open_adapter(agent: &str, session_id: &str) -> Option<Box<dyn Adapter>>;
```

`claude::open(session_id)` and `grok::open(session_id)` return their
adapters; anything else gets `legacy::open(agent, session_id)`.

If a source is truncated or replaced under the adapter (a `/clear`, a new
file), `poll()` returns a `Reset` followed by the rebuilt history.

## Per-engine sources

| engine | content | phase / turns |
|---|---|---|
| claude | `~/.claude/projects/<proj>/<id>.jsonl`, one line per content block (thinking, text, tool_use) + `user` lines carrying `tool_result`. Skip `isSidechain` and `isMeta`. | `user` line → `turn_started`; `system` `turn_duration` → `turn_ended`; terminal activity → `phase`. Hooks later. |
| grok | `~/.grok/sessions/<cwd>/<id>/updates.jsonl` — ACP `session/update` lines: `user_message_chunk`, `agent_message_chunk`, `agent_thought_chunk`, `tool_call`, `tool_call_update`, `_x.ai` `turn_completed`. Consecutive chunks of one kind fold into one block (id = ordinal of the first line). | `events.jsonl`: `turn_started`, `first_token`, `tool_started`, `permission_requested` → `phase needs_you`, `permission_resolved`, `turn_completed`. |
| others | legacy adapter over `conversation_rich`. | terminal activity only. |

## Ownership while it is being built

- spine core, registry, endpoints, WS, legacy adapter, phase bridge:
  `src-tauri/src/spine/mod.rs`, `spine/legacy.rs`, `remote_api.rs`, `lib.rs`, `tabs.rs`.
- claude adapter: `src-tauri/src/spine/claude.rs` (+ visibility tweaks in `detail.rs`).
- grok adapter: `src-tauri/src/spine/grok.rs` (+ visibility tweaks in `grok.rs`).
- phone: `mobile/**`.

The types and trait in `spine/mod.rs` are the contract. Change them only by
agreement; everything else is yours.
