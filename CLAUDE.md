# optfix

Fixed-point option pricing library: `no_std`, allocation-free, no Solana dependency in `optfix/`.
Workspace: `optfix/` (published crate), `bench-program/` (Pinocchio, one instruction per function,
never deployed), `bench/` (LiteSVM CU table), `tools/` (offline coefficient generation).
Every public function has a `pub const` error bound enforced by proptest against OptionStratLib
(`~/Repos/Local/OptionStratLib`, dev-dependency only, Decimal based) or `libm`/mpmath tables.
`clippy::float_arithmetic` is denied in `optfix/`; f64 appears only in tests.
## Conventions shared with matchbook

- Style reference: `~/Repos/Local/matchbook` (read only). Copy its workspace layout, `rust-toolchain.toml`
  (stable), `[workspace.lints]`, release profile (`overflow-checks = true`), Makefile targets and docs layout.
- All code, comments, docs and commit messages in English. No dashes as punctuation in prose; use `:` or `;`.
- On-chain code: Rust, Pinocchio (not Anchor), `no_std`, no allocator, `bytemuck` zero-copy accounts.
  No floating point and no unchecked arithmetic; every rounding is explicit and its direction
  (toward the protocol or toward the user) is documented at the call site.
- Newtypes for every quantity so the compiler rejects mixing them. No `unwrap`, `expect`, `panic`.
- Tests: LiteSVM for programs (Mollusk allowed for instruction-level unit tests), proptest for math.
  Every account-validation rule (signer, owner, PDA seeds, mint, vault) has a negative test with a
  forged account.
- Every instruction has a measured CU cost in `docs/benchmarks.md`, produced by `make bench`.
- Cargo dependency requirements as `MAJOR.MINOR`. Verify crate versions and APIs on crates.io or
  docs.rs before relying on them and cite what was checked.
- `make pre-push` must pass with zero warnings before any push.

## Git

- One feature branch per milestone (`feat/m<N>-<name>`), small commits, PR to `main`.
- Never push without asking Joaquin. Commits are signed (taunais identity via includeIf).
- Worktrees go in `/Users/joaquin/tmp/worktrees/<repo>/<name>`.

## Plan

`docs/PLAN.md` is the source of truth for the domain model, accounts, milestones and open questions.
Update it when a decision changes; record decisions with the numbers that drove them.
