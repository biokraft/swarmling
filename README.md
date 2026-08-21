# swarmling

**A terminal torrent finder that respects your VPN, in a single Rust binary.**

Finding a torrent is miserable. One site is a minefield of fake download buttons, another hides the real link behind a popup that spawns two more tabs, and half of what survives is dead anyway. swarmling searches a short, curated list of reputable sources at once and hands you the result, from a terminal, with nothing to configure.

It is a hard fork of [torlink](https://github.com/baairon/torlink) — a good idea with a good design — rebuilt in Rust so it ships as one binary with no runtime to install, and so the VPN story can be part of the tool instead of your problem.

> **Status: under active rewrite.** Searching works today. Downloading is deliberately not wired to a live torrent session yet — see [Where this is](#where-this-is) before you expect it to pull files down.

## Why this fork exists

**One binary, no runtime.** The original runs through `npx`, which means installing Node before you can search for anything. swarmling is a single Rust binary: download it, run it.

**Your VPN, enforced by the tool.** Most people torrent behind a VPN and hope they configured it right. swarmling's plan is to make that a property of the program: detect the VPN interface, bind all torrent traffic to it, and pause everything the moment the tunnel drops. VPN-agnostic at the core, with adapters for specific providers on top — NordVPN first, the rest open to whoever wants to write one. See [VPN guard](#vpn-guard).

**Nothing phones home.** No telemetry, no analytics, no update pings, ever.

## Where this is

The Rust rewrite is being built milestone by milestone. This table is the honest state of the branch, not a roadmap of intentions:

| Capability | State |
| --- | --- |
| Search across all sources | **Works** |
| Magnet / infohash parsing | **Works** |
| Download queue, persisted across restarts | **Works** (records what you asked for) |
| Actually transferring files | **Not yet wired** — see below |
| Terminal UI | Planned |
| VPN detection, bind policy, `vpn status` | **Works** |
| VPN kill switch applied to live torrents | Not yet wired — waits on the long-running process |
| Seeding controls, headless and daemon modes | Planned |

Downloading is the interesting omission. The engine adapter is written and tested through its trait, but no command in the current CLI opens a live torrent session — because merely opening one starts talking to trackers and the DHT, and the VPN guard that should sit in front of that traffic does not exist yet. Transfers get wired up when there is a long-running, VPN-guarded process to own them. That ordering is deliberate.

## Install

No release binaries yet. From source, with a Rust toolchain:

```sh
git clone https://github.com/biokraft/swarmling
cd swarmling
cargo install --path .
```

Planned once the rewrite lands: `cargo install swarmling`, `brew install biokraft/tap/swarmling`, an `install.sh`, and a Nix flake.

## Use it

```sh
swarmling                           # launch the terminal UI
swarmling tui                       # the same, by name
swarmling search "ubuntu 24.04"     # search every source at once
swarmling add "<magnet>"            # queue a download (records intent)
swarmling add "<magnet>" --paused   # queue it without starting it
swarmling status                    # what is queued
swarmling rm <infohash>             # drop it from the queue
swarmling vpn status                # is a VPN up, and what protection you get
swarmling vpn require on            # `add` refuses unless a VPN is confirmed up
swarmling --help                    # everything else
```

`SWARMLING_DATA_DIR` overrides where the queue and settings are kept, if you want more than one profile.

The terminal UI searches every source, sorts and filters the results, and records what you want downloaded. Like `swarmling add`, it writes that intent to the queue file and nothing more: it transfers no data and contacts no peer. Actually moving bytes waits for a later release.

Search prints one result per line — seeders, size, title, magnet — as each source answers. A source that is down produces a warning on stderr and the search carries on without it.

## What it searches

A short, hand-picked list, inherited from torlink's philosophy of curation over coverage:

| Category | Sources |
| --- | --- |
| Games | FitGirl |
| Movies | YTS, The Pirate Bay, 1337x, BitTorrented |
| TV | EZTV, The Pirate Bay, 1337x, BitTorrented |
| Anime | Nyaa, SubsPlease |

Games come from FitGirl alone, deliberately: games are the one category that can execute code, so they come from a single repacker with a long track record rather than from an open index. Everything else is video and subtitles.

## VPN guard

Planned, and the main reason this fork exists. Two layers:

**The core works with any VPN, with no configuration.** swarmling finds the active VPN interface (`tun`/`wg`/`utun` and friends), binds torrent traffic to it so nothing leaks onto your bare connection, and watches it. If the interface disappears or its address changes, every torrent pauses immediately and resumes when the tunnel is back.

**Adapters add provider-specific control** through a small trait — status, interface, connect. Adapters shell out to the provider's own CLI, so no credentials ever pass through swarmling. NordVPN ships first; more are welcome as pull requests, the same way sources are.

**Windows gets the weaker half, and swarmling says so.** Binding a socket to a network device is a Linux and macOS capability; on Windows the underlying call is unsupported outright. So on Windows there is no binding — only detect-and-pause, which cannot prevent a leak in the moments before a dropped tunnel is noticed. `swarmling vpn status` reports which of the two you are actually getting rather than claiming protection it cannot deliver.

## Credit

swarmling exists because [bairon](https://github.com/baairon) built [torlink](https://github.com/baairon/torlink) and released it under the MIT licence. The curated-source philosophy, the category layout, and a good deal of hard-won scraper behaviour came from that project and are preserved here. See [NOTICE](NOTICE).

This fork diverges on implementation language, distribution model, and the VPN feature. It is not affiliated with the original and does not speak for it.

## Contributing

[CONTRIBUTING.md](CONTRIBUTING.md) covers setup, the house rules, and where the seams are. Adding a search source or a VPN adapter are both deliberately small, self-contained jobs.

## Licence

MIT — see [LICENSE](LICENSE).
