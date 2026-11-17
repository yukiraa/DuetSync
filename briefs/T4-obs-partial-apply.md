# T4: OBS partial-apply safety (Claude Sonnet)

Repo: `D:\+ Personal Project\21. Karaoke delay` (Rust, Windows). Read `CLAUDE.md` first.

## Defect (verified)
`src/obs/mod.rs` worker `ObsCmd::SetOffsets` returns early on a connection error, before it
sends `ObsEvent::Offsets`. `src/app.rs` records the ledger (`cfg.ledger.record`) only for
verified results in `poll_obs`, and it drops `self.applying` on `Disconnected`/`Error`.
Scenario: the first input is written, then the connection dies. The ledger never learns the
baseline, so **Restore can't undo it**, and the next apply treats the compensated value as
the new baseline, which stacks compensation.

## Required behaviour
1. `src/app.rs::confirm_apply`: record every planned change in the ledger (`ledger.record(c)`:
   baseline + applied=target) **before** sending `SetOffsets`, then `save()`. This is safe:
   if a write fails, the input still equals its baseline, `plan()` sees `current != applied`,
   and it reuses the current (original) value as baseline (see `obs/compensation.rs::plan`).
2. `src/app.rs::poll_obs`: on `Offsets { restore: false }` no longer record (already done);
   keep the status message. On `restore: true` keep `forget` for verified inputs only.
   Remove the `applying` field if it becomes unused.
3. `src/obs/mod.rs` worker `SetOffsets`: never abort without reporting. On a connection
   error, push an `ApplyResult` with `Err(..)` for the failing input **and every remaining
   input**, send `ObsEvent::Offsets`, then send `ObsEvent::Disconnected(err)` and drop the
   client.
4. Tests: add to `tests/obs.rs` a mock-server case where the server closes the connection
   after the first `SetInputAudioSyncOffset`; assert the worker emits `Offsets` with one
   verified result and the rest `Err`, followed by `Disconnected`. Add a unit test in
   `obs/compensation.rs` (or reuse existing) proving that recording before a failed write
   gives `baseline == original` on the next `plan`.

## Files you own (modify ONLY these)
`src/app.rs` (only `confirm_apply`, `poll_obs`, the `applying` field), `src/obs/mod.rs`,
`src/obs/compensation.rs` (tests only), `tests/obs.rs`.

## Gate
`cargo fmt --check`, `cargo clippy --all-targets` (0 warnings), `cargo test` all pass.
Report: files changed, tests added, exact pass counts. Max 200 words. Do not commit.
