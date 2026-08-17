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
}

async fn open_queue() -> anyhow::Result<(
    swarmling::download::queue::DownloadQueue,
    std::path::PathBuf,
)> {
    use swarmling::config::paths;
    use swarmling::download::{persist::load_entries, reconcile::restore};
    use swarmling::engine::librqbit_engine::LibrqbitEngine;

    let state_path = paths::queue_state_path();
    let engine = std::sync::Arc::new(LibrqbitEngine::new(paths::data_dir().join("session")).await?);
    let (queue, report) = restore(
        engine,
        paths::default_download_dir(),
        load_entries(&state_path),
    )
    .await;
    for (infohash, error) in &report.failed {
        eprintln!("warn: could not restore {infohash} ({error})");
    }
    Ok((queue, state_path))
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
        Command::Add {
            magnet,
            title,
            paused,
        } => {
            let (queue, state_path) = open_queue().await?;
            let title = title.unwrap_or_else(|| magnet.clone());
            let infohash = queue.add(&magnet, &title, paused).await?;
            queue.save(&state_path)?;
            println!("added {infohash}");
        }
        Command::Status => {
            let (queue, _) = open_queue().await?;
            let snapshots = queue.snapshots().await;
            for entry in queue.entries() {
                let snap = snapshots.iter().find(|s| s.infohash == entry.infohash);
                let (state, percent) = match snap {
                    Some(s) if s.total_bytes > 0 => (
                        format!("{:?}", s.state),
                        format!(
                            "{:.1}%",
                            (s.progress_bytes as f64 / s.total_bytes as f64) * 100.0
                        ),
                    ),
                    Some(s) => (format!("{:?}", s.state), "0.0%".to_string()),
                    None => ("Unknown".to_string(), "-".to_string()),
                };
                println!("{state}\t{percent}\t{}", entry.title);
            }
        }
        Command::Rm {
            infohash,
            delete_files,
        } => {
            let (queue, state_path) = open_queue().await?;
            queue.remove(&infohash, delete_files).await?;
            queue.save(&state_path)?;
            println!("removed {infohash}");
        }
    }
    Ok(())
}
