# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this repository is

swarmling — a terminal torrent finder, being rewritten in Rust. It is a hard fork of `baairon/torlink` (TypeScript + Ink + WebTorrent). Both trees are present during the transition:

- **`src/**/*.rs` — the Rust rewrite. All new work goes here.**
- `src/**/*.ts`, `src/**/*.tsx`, `scripts/`, `package.json`, `nix/` — the legacy TypeScript app, kept until the Rust version reaches parity, then deleted. Do not add to it.

Work happens on the `rust` branch. `main` still holds the TypeScript app.

## THE HARD RULE

**Never start a real torrent transfer.** No test, script, example, or verification step may open a live `librqbit::Session`, add a magnet to a real engine, contact a tracker or DHT node, connect to a peer, or seed data. Never run `cargo run` on the download subcommands.

This is project policy, not a style preference: automated transfers of unknown content carry legal exposure for whoever runs the suite, in whichever jurisdiction they run it. Treat it as absolute.

A live session is now constructed in exactly one place in production code: `src/tui/run.rs`, which builds the one `LibrqbitFactory` in the codebase and hands it to a `Driver`. `src/supervisor/factory.rs` is the only other file that names librqbit. Nothing about this weakens the rule above — no test, script, example, or CI step constructs a `LibrqbitEngine` or `LibrqbitFactory` or opens a socket; every test goes through the `SessionFactory` seam and a fake. Manual verification of download behaviour remains a maintainer's job, on their own machine, behind their own VPN. Never yours.

Consequences that shape the whole design:
- Everything the app needs from a torrent backend goes through the `TorrentEngine` trait (`src/engine/mod.rs`).
- `src/engine/fake.rs` is an in-memory `FakeEngine` that never opens a socket — the queue, persistence and restore logic are all tested against it.
- `src/engine/librqbit_engine.rs` is the only file that knows librqbit exists. It is verified by compilation, clippy and review only.
- Plain HTTPS search queries to source sites are fine; they involve no P2P.
- Manual verification of download behaviour is a maintainer's job, on their own machine, behind their own VPN. Never yours.

## Commands

```sh
cargo build
cargo test                                   # full suite
cargo test --lib nyaa                        # one module
cargo test -- parses                         # by test name
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings    # CI denies warnings
cargo run -- search "ubuntu"                 # SAFE: HTTP only, no P2P
```

Never run `cargo run -- add`, `status`, or `rm` against a real session — see the hard rule.

## Architecture

- `src/sources/` — one file per torrent site, each implementing the async `Source` trait, all listed in `registry.rs`. `magnet.rs` parses magnet URIs (base32 to hex infohash normalization); `rss.rs` is the shared feed helper. Sources are tested with `wiremock` and a `base_url` field that tests override — no test contacts a live site.
- `src/search.rs` — `search_all` fans out to every source concurrently and streams `SearchEvent`s over an mpsc channel. Each source is wrapped in a timeout; a failure or timeout becomes `SourceFailed`, never a panic.
- `src/engine/` — the `TorrentEngine` trait, the fake, and the librqbit adapter. `build_magnet` (in `sources/types.rs`) is the single chokepoint for magnet construction and validates infohashes; nothing else should assemble a magnet.
- `src/download/` — `queue.rs` (the user's intent, with idempotent adds guarded by an async mutex), `persist.rs` (atomic versioned JSON state), `reconcile.rs` (restore persisted entries into an engine at startup).
- `src/config/paths.rs` — platform data and download directories via `directories`.
- `src/main.rs` — clap CLI. **The download subcommands deliberately construct no session**: `add` resolves the infohash locally, `status` and `rm` only read and write the queue file. The only place that constructs a live session is `src/tui/run.rs`, described below.
- `src/supervisor/` — the live session, held only while the TUI runs. `state.rs` is a pure reconciler: given what the user wants and the tunnel's state, it decides what to build, add, pause, or kill, with no I/O of its own — the VPN dropping produces a kill instruction here, not a pause. `factory.rs` is the seam between that decision layer and a real engine: `SessionFactory` is what tests implement with a fake, and `LibrqbitFactory` is the only production implementer, naming librqbit alongside `src/tui/run.rs`. `driver.rs` holds the one live `Arc<dyn TorrentEngine>` handle in the process and executes the reconciler's instructions against it, with a per-call timeout so a wedged engine call cannot hold the kill switch hostage.

## House rules

- Fail soft everywhere. Remote input is hostile: no unchecked indexing, no `unwrap` on anything fallible, no panics from a malformed feed or a corrupt state file.
- Reuse the seams: shared HTTP client (`util/net.rs`), `build_magnet`, the `Source` registry. Don't add a parallel mechanism.
- Cross-platform always; CI is a 3-OS matrix.
- Non-trivial logic gets a test. A regression test must be verified to actually fail against the unfixed code.
- Persisted state is versioned — new `QueueEntry` fields need `#[serde(default)]` so an older file still loads.
- No telemetry, ever. No personal information (emails, machine paths, usernames) in anything committed.
- Theme is pastel-violet and quiet; exactly one gradient, the wordmark.
- Conventional Commits; one concern per commit.

## Local planning docs

`docs/` is gitignored and never committed. Design specs and implementation plans live there for reference but must not be added to git.
