# Task 9 Phase B / Task 10 Integration Map

## Integration Baseline and Commit Order

Target: `83c8f3a` (`agent/tauri-interview-foundation`). The branches do not share that
commit as an ancestor: storage forks at `912ecbe`, and React forks at `ac4fa40`.

1. After the native build branch passes its real-machine gates, cherry-pick the completed
   Task 9 Phase A series in order:
   `3aa78de`, `483d8f5`, `47a9973`, `aed7bf5`, `ac9aed3`, `69c97e9`, `5e4882b`,
   `2ed31e4`, `653ae44`, `4e229d2`, `1597654`, `f471b0c`, `903f1e4`, `22d218e`.
2. Resolve `desktop/src-tauri/Cargo.toml` by retaining the foundation Tokio `io-util` and
   `process` features and adding the storage/build/target dependencies. Regenerate
   `Cargo.lock` from that resolved manifest rather than selecting either branch's lockfile.
3. Cherry-pick Task 10 in order:
   `5de4e94c4ad00eb6a0cc63763f3febb0c9266c72`,
   `a0069ab40f53601cad8eebc73eefbab07d12b4e5`,
   `61dc78b8c00b0211ca9a54970385d95fb8a8bfb4`,
   `494aa2a91b473f57b69a950e9b985bfd7fce11c7`.
4. Implement the remaining integration as three reviewable commits:
   `feat: expose schema-v2 persistent session API`,
   `feat: persist sidecar events before live delivery`, then
   `fix: make React restore deterministic and storage-aware`.

Expected textual conflicts are in `Cargo.toml` and possibly its dependency block again while
applying `ac9aed3`; the lockfile must be regenerated. Storage does not modify the foundation's
`sidecar.rs`, `protocol.rs`, or `lib.rs`, so preserve the lifecycle/protocol hardening from
`83c8f3a`. The four Task 10 commits primarily create semantic DTO and replay conflicts rather
than textual conflicts.

## Required Native Wiring

`AppState` must own the workspace identity, `Arc<SessionRepository>`, and the sidecar.
During setup: obtain the `KeyManager` key, open the SQLCipher database at the app data path,
manage the fully initialized state, and use a persistence-aware `SidecarEventSink`.

That sink must map validated events to storage before forwarding them through
`sidecar://event`: persist `session.state`, final `transcript.updated`,
`suggestion.completed`, notes, and pins; never persist partial transcripts or suggestion
chunks. Repository writes are idempotent by event ID. A repository error must not return an
error to the supervisor or suppress the original live event: emit recoverable storage health,
then forward the source event. Commands must validate UUIDs, language tags, bounded input,
and the state-owned workspace before repository calls.

The generated command set is `create_session`, `save_session_brief`, `complete_session`,
`list_sessions`, `get_session`, `get_timeline`, `restore_active_session`, and
`delete_session`, in addition to the existing sidecar commands. Repository calls are
synchronous, so commands and event persistence should use the established blocking boundary
rather than blocking Tauri's async executor.

## Durable Concurrent Suggestion Contract

Task 10 currently keeps `command.id -> questionTurnId` only in the Zustand store;
`RuntimeProvider.send()` does not send the turn ID to Rust. Protocol V1's `query.trigger`
payload accepts only `text` and `answer_format`, while a completed suggestion returns only
`correlation_id` (the request ID). The host therefore cannot persist the required association,
and chronological replay is ambiguous for concurrent suggestions.

Required contract: before sending a `query.trigger`, React calls a new native association
command with `{ sessionId, requestId: command.id, turnId }`. Rust validates all three IDs and
ownership, then transactionally persists `(workspace_id, session_id, request_id, turn_id)` in
a dedicated association table. On `suggestion.completed`, the persistence sink resolves
`event.correlation_id` through that table and writes both `request_id` and `turn_id` into the
timeline row. Retain the mapping through idempotent retries; it may be retired only after the
completed suggestion is durably written. Replay must load these associations before applying
timeline envelopes, then call `associateRequestWithTurn` before the corresponding completed
suggestion. No "latest unanswered turn" fallback is acceptable for concurrent requests.

This needs an explicit v1-to-v2 schema migration, plus a small
PlatformApi extension (for example `associateRequestWithTurn` and
`getRequestTurnAssociations`). It avoids changing the closed Protocol V1 payload schema.

The association table must be workspace/session owned and make repeated identical mappings
idempotent while rejecting attempts to remap a request to a different turn. Persist an
association before sending `query.trigger`; if association fails, do not send the request.

## Storage Health

Use a host-originated storage health channel/DTO, not a synthetic `sidecar://event` envelope:
a synthetic envelope cannot safely share the sidecar sequence watermark and Task 10 currently
does not model storage health. Extend `PlatformApi`/desktop adapter and `RuntimeProvider` to
subscribe to it, add `health.storage`, and render/recover from `pending`, `ready`,
`degraded`, and `error`. The health payload must be redacted and recoverable; it must contain
no key, database path, SQL, transcript, or brief content.

## DTO Alignment Required Before Merge

Task 10 `SessionRecord` is `{ id, mode, status, ui_language?, input_language,
response_language, review_language, brief? }`. Its current invokes use:

- `create_session { input: { mode, ui_language, input_language, response_language, review_language } }`
- `save_session_brief { input: { session_id, brief: Record<string, unknown> } }`
- `complete_session { sessionId, status }`, `list_sessions { limit }`, and session-ID
  arguments for get/timeline/delete.

Phase A instead persists `title`, one `language`, an optional completion timestamp, and a
string `summary`; it has no mode, status, UI/input/response/review language set, JSON brief,
or list limit. Phase B must migrate the schema/models and serialize commands to exactly the
Task 10 DTO names. `get_session` and `restore_active_session` must join/decode the brief;
`complete_session` must persist the requested status; `list_sessions` must enforce the
validated range `1..=100`. `get_timeline` must reconstruct canonical envelopes in repository host order
while exposing request-turn associations separately for deterministic replay.

Keep protocol runtime state separate from the persisted session status. In particular,
`session.state` values such as `listening` and `idle` must not mutate `SessionRecord.status`.

## Focused Tests

- Resolve `3aa78de` onto `83c8f3a`; build storage tests with the required SQLCipher toolchain.
- Command DTO round trips, UUID/language/length/ownership rejection, status and JSON-brief
  restore, limit enforcement, and deletion cascade.
- Sink ordering: durable event commits before client delivery; chunks/partials never write;
  duplicate delivery is one row; storage failure reports health and still delivers live data.
- Two query requests for different turns, reverse completed-event order, process restart, and
  replay: each completed suggestion resolves to its original turn with no heuristic fallback.
- Storage-health adapter/provider/store tests, including redaction and recovery to ready.
- Run Task 10's full test/typecheck/build suite after all contract updates, then Rust storage
  tests and clippy.

## Blocking Collisions

1. **Blocking:** Task 10's platform DTOs cannot be implemented by the Task 9 Phase A schema.
   The mismatch covers session fields, brief shape, completion status, list limit, and timeline
   association replay.
2. **Blocking:** The plan requires durable concurrent request-to-turn association, but closed
   Protocol V1 and the completed Task 10 API expose no Rust-visible turn ID. A native durable
   association command/table and replay API are required before Phase B can meet the plan.
3. **Blocking:** The plan requires recoverable storage health, but Task 10 has no storage health
   model and `runtime.error` is not mapped to health; synthetic host envelopes also collide with
   sidecar sequence semantics. Define the host storage-health DTO/channel before implementation.
4. **Gate satisfied:** `22d218e` passed 59 release storage tests, 51 native-wrapper tests,
   warnings-denied Clippy, the release desktop build, linker/PDB scans, and independent storage
   plus native-build reviews. The branch is pushed as `origin/agent/encrypted-storage`.

## Execution Status (2026-08-07)

- All 14 Phase A storage commits and all 4 original Task 10 React commits are integrated on
  `agent/tauri-interview-foundation` through `09ee4f8`.
- Schema v2 landed as `f42cebf` (`feat: expose schema-v2 persistent session API`). Independent
  review found migration/terminal-state hardening items; fix round 1 is active on
  `agent/phase9-storage-v2` and must land before this gate is complete.
- Clean-worktree native provisioning exposed CRLF-dependent OpenSSL patch bytes. `0bb30ac`
  normalizes only the injected here-string bytes, preserves the approved `b6d42f...` hash, and
  passes all 51 Windows native-wrapper tests. Foundation is pushed through `0bb30ac`.
- Native AppState/commands/persistence-aware event delivery is in progress and uncommitted.
  Repository work now crosses `spawn_blocking`; validated event delivery is serialized and
  remains persist-before-emit, with redacted storage health and live-event fallback.
- React deterministic restore fix round 1 is active on `agent/phase9-react-restore`; it must
  preserve independent storage health across sidecar restart and close initial-health/unmount
  races before integration.

## Task 7 Capture-Exclusion Evidence (2026-08-17)

- Rust formatting check passed. Focused durable-query tests passed: 10 library and 3 storage
  target tests. The inherited exhaustive-match/import fixes were retained and committed in
  `3c4a4ac`; that commit also resolves four current `-D warnings` Clippy findings in sidecar
  code/tests. The prior controller Python commit is `00e15a7` and was not staged here.
- Windows native release Clippy (`--all-targets -- -D warnings`) passed after the warning fixes.
  The release native build passed in 10m10s. The full serial aggregate test command completed
  its library target: 220 passed, 0 failed, in 5747.42s. It was intentionally interrupted as
  Cargo began the separately declared `storage` target, because Cargo.toml points that target
  directly to `src/storage/mod.rs`, duplicating the same 93 storage tests with no added coverage.
  The amended release-profile `--lib` command then passed 220/220, 0 failed, in 73.73s of test
  execution with four bounded workers, and the complete native-wrapper command exited zero.
- Exact release artifact: SHA-256
  `FD761B79195D74EF2808FDF72818AB1A9454069A4A48C864D78203BA3DFA29BD`;
  Windows build `22631`. The exact newly built executable was launched, and an independent
  user32 query found exactly one visible ownerless `CallerInterview` window for that launched
  PID with `GetWindowDisplayAffinity == 17` (`WDA_EXCLUDEFROMCAPTURE`). The process was closed;
  no HWND was retained.
- Controller evidence: desktop Vitest passed 93/93 (30.33s); desktop production build passed
  (1,935 modules, 22.73s); Python focused shutdown tests passed 4/4 (9.26s) and full
  `ai_assistant/tests` passed 130/130 (117.00s) with no warning summary. The exact release app's
  React provider invoked `capture_protection_status` on mount; Windows UI Automation found the
  resulting status element with exact accessible label `Capture protected`, proving the
  end-to-end app-reported controller state.
- Focused fail-closed acceptance passed 1/1: an Unavailable capture target prevented the
  protected dispatch closure from running, returned `capture_protection_required`, stopped the
  active sidecar runtime, and cleared runtime authorization.
- Third-party capture-tool acceptance is unverified. Snipping Tool and Teams are installed but
  were not exercised, while OBS and Zoom are not installed. Google Meet and macOS capture/CI
  evidence were unavailable, so macOS remains unclaimed. These remain pre-GA platform gates and
  cannot support claims for those tools or macOS; they do not block integration of the verified
  Windows implementation.
- Additional controller acceptance used only the Windows `System.Drawing.Graphics.CopyFromScreen`
  `SourceCopy` path. It launched the exact release executable, selected exactly one visible
  ownerless `CallerInterview` window for its launched process, reconfirmed affinity 17, and
  captured that window rectangle. Visual inspection showed the underlying desktop across the
  full crop with no CallerInterview content; the app closed cleanly. This confirms exclusion
  only for that screen-copy API and does not verify Snipping Tool, Teams, Google Meet, OBS,
  Zoom, or macOS.
