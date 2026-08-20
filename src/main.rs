use clap::{Parser, Subcommand};
use swarmling::search::{search_all, SearchEvent};
use swarmling::sources::registry::all_sources;

#[derive(Parser)]
#[command(name = "swarmling", version, about = "Terminal torrent finder")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Search all sources and print results
    Search { query: String },
    /// Add a magnet link to the download queue
    Add {
        magnet: String,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        paused: bool,
    },
    /// Show the state of every queued download
    Status,
    /// Remove a download from the queue
    Rm {
        infohash: String,
        #[arg(long)]
        delete_files: bool,
    },
    /// Inspect and configure the VPN guard
    Vpn {
        #[command(subcommand)]
        action: VpnAction,
    },
}

#[derive(Subcommand)]
enum VpnAction {
    /// Show the VPN state and what protection this platform can give
    Status,
    /// Require a VPN before downloading
    Require {
        #[arg(value_parser = ["on", "off"])]
        value: String,
    },
}

fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    format!("{v:.1} {}", UNITS[u])
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Search { query } => {
            let mut rx = search_all(all_sources(), &query).await;
            while let Some(ev) = rx.recv().await {
                match ev {
                    SearchEvent::Results { results, .. } => {
                        for r in results {
                            println!(
                                "{}\t{}\t{}\t{}",
                                r.seeders,
                                human_size(r.size_bytes),
                                r.title,
                                r.magnet
                            );
                        }
                    }
                    SearchEvent::SourceFailed { source_id, error } => {
                        eprintln!("warn: {source_id} offline ({error})");
                    }
                }
            }
        }
        // --- add / status / rm -----------------------------------------
        //
        // These three commands deliberately never construct a librqbit
        // `Session` and never touch the network. They are pure manipulation
        // of the persisted queue file (`queue.json`): `add` appends an
        // entry, `status` prints what's persisted, `rm` drops an entry.
        //
        // A `Session` with default options starts DHT, tracker
        // communication and local service discovery the instant it is
        // constructed — before a single torrent is even added — and this
        // milestone has no VPN guard in front of that traffic yet. A
        // transfer (and therefore a `Session`) must only ever be started
        // from an explicit, VPN-guarded, long-running process added in a
        // later milestone. `reconcile::restore` and `LibrqbitEngine` still
        // exist and are unit-tested for that future process; the CLI here
        // just doesn't call them.
        Command::Add {
            magnet,
            title,
            paused,
        } => {
            use swarmling::config::{paths, settings};
            use swarmling::download::persist::{load_entries, save_entries};
            use swarmling::download::queue::QueueEntry;
            use swarmling::sources::magnet::parse_magnet;
            use swarmling::vpn::require;

            // The VPN requirement is checked before anything is written to
            // the queue. This reads interfaces and, for the nordvpn adapter,
            // runs a read-only `nordvpn status`; it constructs no session.
            let cfg = settings::load(&paths::settings_path());
            if cfg.vpn_required {
                let status = require::adapter_for(&cfg).status().await;
                if let Some(reason) = require::refusal_reason(cfg.vpn_required, &status) {
                    eprintln!("error: {reason}");
                    std::process::exit(2);
                }
            }

            let parsed = match parse_magnet(&magnet) {
                Some(p) if !p.infohash.is_empty() => p,
                _ => {
                    eprintln!("error: magnet has no usable infohash");
                    std::process::exit(1);
                }
            };

            let state_path = paths::queue_state_path();
            let mut entries = load_entries(&state_path);

            if let Some(existing) = entries.iter().find(|e| e.infohash == parsed.infohash) {
                println!("already queued: {}", existing.infohash);
                return Ok(());
            }

            let title = title.unwrap_or_else(|| {
                if !parsed.name.is_empty() {
                    parsed.name.clone()
                } else {
                    magnet.clone()
                }
            });

            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);

            entries.push(QueueEntry {
                infohash: parsed.infohash.clone(),
                magnet: magnet.clone(),
                title,
                added_unix: now,
                paused,
            });
            save_entries(&state_path, &entries)?;
            println!("added {}", parsed.infohash);
        }
        Command::Status => {
            use swarmling::config::paths;
            use swarmling::download::persist::load_entries;

            let entries = load_entries(&paths::queue_state_path());
            for entry in entries {
                // No live engine is attached here, so only the persisted
                // intent is known: report the `paused` flag, not fabricated
                // progress.
                let state = if entry.paused { "Paused" } else { "Queued" };
                println!("{state}\t-\t{}", entry.title);
            }
        }
        Command::Rm {
            infohash,
            delete_files,
        } => {
            use swarmling::config::paths;
            use swarmling::download::persist::{load_entries, save_entries};
            use swarmling::sources::magnet::is_infohash;

            if !is_infohash(&infohash) {
                eprintln!("error: not a valid infohash: {infohash}");
                std::process::exit(1);
            }

            if delete_files {
                println!(
                    "note: --delete-files is not available until the download engine is attached; no files were deleted"
                );
            }

            let state_path = paths::queue_state_path();
            let mut entries = load_entries(&state_path);
            let before = entries.len();
            entries.retain(|e| e.infohash != infohash);
            if entries.len() == before {
                eprintln!("warn: no queued entry with infohash {infohash}");
            }
            save_entries(&state_path, &entries)?;
            println!("removed {infohash}");
        }
        // `vpn status` only ever reads: it inspects interfaces and, for the
        // nordvpn adapter, shells out to a read-only `nordvpn status`. It
        // never calls `connect()` and never constructs a librqbit `Session`.
        Command::Vpn { action } => match action {
            VpnAction::Status => {
                use swarmling::config::{paths, settings};
                use swarmling::vpn::adapter::VpnStatus;
                use swarmling::vpn::policy::protection_summary;
                use swarmling::vpn::require;

                let cfg = settings::load(&paths::settings_path());
                let adapter = require::adapter_for(&cfg);

                println!("adapter: {}", adapter.name());
                match adapter.status().await {
                    VpnStatus::Connected { interface } => {
                        println!("state: connected");
                        println!("interface: {} ({})", interface.name, interface.ip);
                    }
                    VpnStatus::Disconnected => {
                        println!("state: disconnected");
                    }
                    VpnStatus::Unknown(reason) => {
                        println!("state: unknown ({reason})");
                    }
                }
                println!("protection: {}", protection_summary(std::env::consts::OS));
                println!("vpn_required: {}", cfg.vpn_required);
            }
            VpnAction::Require { value } => {
                use swarmling::config::{paths, settings};

                let path = paths::settings_path();
                let mut cfg = settings::load(&path);
                cfg.vpn_required = value == "on";
                settings::save(&path, &cfg)?;
                println!("vpn_required: {}", cfg.vpn_required);
            }
        },
    }
    Ok(())
}
