# T3: systems fixes from audit T1 (implementation)

Repo root = cwd. Read `CLAUDE.md` first. Implement exactly these three fixes with tests.

## Files you own (modify ONLY these)
- src/network/session.rs, tests/network.rs
- src/playback/core.rs
- src/app.rs: ONLY the functions `guest_play` and `start_local` (others belong to another agent)

## Fix A: bound pre-auth connections (session.rs, host)
`Host::start` spawns a detached thread per accepted TCP connection before authentication.
Cap concurrent handshakes at 4 (an `AtomicUsize` in `HostShared`, decremented by a guard
when `host_conn` returns). Over the cap: close the socket immediately and send
`Event::AuthFailed("too many pending connections")`. Test: open 10 raw TCP connections
that send nothing; a real guest must still be able to connect after they time out,
and the host must not hold more than 4 handshakes at once.

## Fix B: join-code expiry + resumable reconnect (session.rs, host + guest)
Currently `joined_once` disables expiry for everyone after the first join, and `rotate()`
breaks reconnects. New design:
- On a successful handshake, both sides derive
  `resume = crypto::mac(&secret_used, b"DuetSync-resume", &[&cn, &sn])` (32 bytes).
- Host keeps `resume: Option<[u8; 32]>`. Guest switches its credential to `resume` for all
  later reconnects.
- Host auth accepts: (a) the join secret if `now <= deadline`, or (b) the stored resume
  secret at any time. Try both MACs (`crypto::verify`). A successful resume re-derives and
  replaces the resume secret (both sides) from the new nonces.
- `rotate()` changes only the join secret/deadline; the resume secret stays valid.
- Remove `joined_once`. Keep the existing reject messages (`invalid join code`, `join code expired`).
- Session keys still come from `session_keys(credential_used, cn, sn)`.
Tests (tests/network.rs): (1) after expiry, a NEW guest with the original code is rejected
while an already-joined guest reconnects after a host-side link drop; (2) after `rotate()`,
the joined guest still reconnects; the old code is rejected for new guests. Keep all
existing tests passing.

## Fix C: late start / large resync must re-anchor, not drain (core.rs + app.rs)
`PlayerCore::fill` handles a late start (`wait < 0`) and a hard resync "behind" by popping
one frame per skipped frame. The ring holds only ~2 s, so a skip larger than the ring empties
it while `pos` still jumps: the audio plays the wrong position while it reports sync.
- core.rs: cap any skip at the frames actually popped. Set `pos` from frames actually
  consumed (never from the requested skip). If the source runs dry before the skip completes,
  set a public flag `needs_reseek: Option<u64>` (the track frame that should be playing) and
  output silence until the next `Play`. Never loop more than `rate` (1 s) frames per `fill`.
- app.rs `guest_play`/`start_local`: if the local start time is less than 300 ms in the
  future, re-anchor before seeking: `from += (now + 500 ms - start_local) * rate`,
  `start_local = now + 500 ms` (seek the feeder to the new `from`). Also, in `start_local`'s
  caller path nothing else changes.
- Unit tests in core.rs: late start by 10 s with a 2 s source buffer yields `needs_reseek`
  and never reports a position the source did not deliver; small late start (100 ms) still
  works exactly as today.

## Constraints
- Audio thread (`fill`): no allocation, no locks, bounded loops.
- Do not change the wire protocol enums except where Fix B needs it (it should not).
- Gate: `cargo fmt --check`, `cargo clippy --all-targets` 0 warnings, `cargo test` all pass.
  If `target\` is locked, use `$env:CARGO_TARGET_DIR="target-codex"`.

## Final message (max ~300 words)
Per fix: what changed (file:function), the tests added, and gate results (exact pass counts).
