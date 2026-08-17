## What and why

<!-- What does this change do, and why? The diff shows the what; tell us the why. -->

## Checklist

- [ ] `cargo fmt --all --check` is clean
- [ ] `cargo clippy --all-targets -- -D warnings` is clean
- [ ] `cargo test` passes
- [ ] New logic has a test (wiremock for sources, `FakeEngine` for download logic, `tempfile` for filesystem code)
- [ ] **No test, script or step of mine starts a real torrent transfer** — no live `librqbit::Session`, no tracker, DHT, peer or seeding activity
- [ ] Nothing parsed from a remote source can panic the process
- [ ] OS-touching code works on Windows, macOS and Linux
- [ ] If I added a `QueueEntry` field, it has `#[serde(default)]` so older state files still load
- [ ] One concern, with a Conventional Commits title (`feat:` / `fix:` / `docs:` / `chore:`)

New here? [CONTRIBUTING.md](../CONTRIBUTING.md) explains each of these, including why the torrent rule is absolute.
