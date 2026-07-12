## What changed

Describe the behavior change and why it is needed.

## Evidence

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --workspace --all-targets --locked -- -D warnings`
- [ ] `cargo test --workspace --all-targets --locked`
- [ ] Relevant API/UI smoke check completed
- [ ] Tests added or adjusted for engine/GA/contract changes

## Contracts and safety

- [ ] JSON contract changes are reflected in UI, exports, and API docs
- [ ] The profitable synthetic demo still works without live opportunities
- [ ] No secrets, real orders, custody, or on-chain transfers were introduced
- [ ] Logs and fixtures contain no sensitive data

## Reviewer notes

Call out migration risk, performance impact, and rollback steps.
