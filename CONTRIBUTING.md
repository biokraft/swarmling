# Contributing to swarmling

swarmling stays small on purpose. The best way in is to read the code you're about to touch, match how it already works, and keep your change tight.

The project is mid-rewrite: it began as a fork of [torlink](https://github.com/baairon/torlink) (TypeScript, Ink, WebTorrent) and is being rebuilt in Rust. New work goes in the Rust tree. The legacy TypeScript tree is still in the repository during the transition and gets deleted once the Rust version reaches parity — don't add to it.

## Set up

```sh
git clone https://github.com/biokraft/swarmling
cd swarmling
cargo build
cargo run -- search "ubuntu"
```

Rust 1.88 or newer (`rust-toolchain.toml` pins it).

## Before you open a PR

Run these and make sure they're clean:

```sh
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test
```

CI runs all three on Linux, macOS and Windows.

## The one rule that isn't negotiable

**Never write a test — or a script, or an example — that starts a real torrent transfer.** No test may open a live `librqbit::Session`, contact a tracker or DHT node, connect to a peer, or seed data. Pulling someone else's copyrighted material from an automated test is a legal problem for whoever runs the suite, and "it only ran for a second" is not a defence.

This is why the code is shaped the way it is. Everything the app needs from a torrent backend goes through the `TorrentEngine` trait, and the queue, persistence and restore logic are all tested against an in-memory `FakeEngine` that never opens a socket. Exactly one file (`src/engine/librqbit_engine.rs`) knows librqbit exists, and it is verified by compilation and review rather than by being run.

If a change genuinely cannot be verified without real network transfer, say so in the PR and leave it unverified rather than running it.

## The standards

### Match the existing grain

Reuse what's there before writing something new. Search sources implement one trait and are listed in one registry. HTTP goes through the shared client in `src/util/net.rs` — don't build a second one. Magnets are built by `build_magnet`, the single chokepoint that validates infohashes; don't assemble a magnet string by hand.

If you catch yourself adding a parallel way to do something the codebase already does, stop and use the one that's already there.

### Fail soft, never crash

When something you don't control goes wrong — a source is down, a feed is malformed, a state file is corrupt — degrade and say so. A dead source produces a warning and the search continues. A corrupt queue file loads as an empty queue rather than bricking the app. Reach for a clear message and a fallback before you reach for a panic.

Remote input is hostile input. Nothing parsed from a website or a feed may be able to panic the process: no unchecked indexing, no `unwrap` on anything fallible, no slicing at an offset you didn't derive safely.

### Cross-platform or it doesn't ship

swarmling runs on Windows, macOS and Linux, so anything touching the OS handles all three. "Works on my machine" is not the bar; CI will tell you.

### Test the logic

Non-trivial logic gets a test. Sources are tested against `wiremock` with captured fixtures — never against the live site. Download logic is tested against `FakeEngine`. Filesystem code uses `tempfile`, never a real user directory.

A regression test that cannot fail is worse than no test, because it looks like protection. If you're adding one for a race or an ordering bug, verify it actually fails against the unfixed code before you submit it.

### Adding a search source

The smallest useful contribution, and deliberately so. One file in `src/sources/`, implementing the `Source` trait, registered in `src/sources/registry.rs`, with wiremock tests covering a normal response and an empty or broken one. Follow an existing source as the template; `apibay.rs` and `yts.rs` are the simplest.

Keep the curation principle in mind: this is a short, hand-picked list, not an index of every tracker. A new source needs a reason to be trusted, and games stay with FitGirl alone.

### Adding a VPN adapter

Also small by design. Implement the `VpnAdapter` trait, shell out to the provider's own CLI, and never handle the user's credentials — swarmling should never see, store, or transmit them. If the provider has no CLI on a given platform, fall back to the core interface detection rather than inventing something.

### Respect the calm theme

swarmling is pastel-violet and quiet. When the terminal UI lands, there will be exactly one gradient — the wordmark. Everything else is solid colour. Please don't add a second gradient.

### Privacy is a feature

No telemetry, no analytics, no phone-home, no "anonymous" usage stats. Not behind a flag, not opt-in. If a change makes the program talk to a server the user didn't ask it to talk to, it doesn't land.

## Commits and pull requests

- Use [Conventional Commits](https://www.conventionalcommits.org) prefixes: `feat:`, `fix:`, `docs:`, `chore:`, `refactor:`.
- Say why, not just what. The diff already shows the what.
- One concern per pull request. Two unrelated ideas are two PRs.

Thanks for helping keep swarmling sharp.
