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
    }
    Ok(())
}
