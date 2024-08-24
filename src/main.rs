use std::collections::HashMap;

use clap::{command, Parser};
use log::info;
use shreds::algo::RAYDIUM_AMM;
use shreds::raydium::download_raydium_json;
use shreds::{benchmark, listener};

#[derive(Parser)]
#[command(name = "shreds", version = "1.0", author = "piotrostr")]
struct Cli {
    /// Sets the bind address
    #[arg(
        short,
        long,
        value_name = "ADDRESS",
        default_value = "0.0.0.0:8001"
    )]
    bind: String,

    /// Run in save mode (dump packets to file)
    #[arg(short, long)]
    save: bool,

    /// Download Raydium JSON
    #[arg(short, long)]
    download: bool,

    #[arg(long)]
    benchmark: bool,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenv::dotenv().ok();

    let cli = Cli::parse();

    env_logger::Builder::default()
        .format_module_path(false)
        .filter_level(log::LevelFilter::Info)
        .format_timestamp_millis()
        .init();

    info!("Binding to address: {}", cli.bind);

    if cli.download {
        download_raydium_json(true).await?;
        return Ok(());
    }

    if cli.benchmark {
        let pubsub_handle = tokio::spawn(async move {
            benchmark::listen_pubsub(vec![RAYDIUM_AMM.to_string()])
                .await
                .expect("pubsub")
        });
        let shreds_handle = tokio::spawn(async move {
            tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;
            listener::run_listener_with_algo(&cli.bind)
                .await
                .expect("shreds")
        });
        let (bench_sigs, shreds_sigs) =
            tokio::try_join!(pubsub_handle, shreds_handle)?;
        let mut miss_count = 0;
        let mut slower_count = 0;
        let mut faster_count = 0;
        let mut shreds_sigs_timestamps = HashMap::new();
        for (timestamp, sig) in shreds_sigs.iter() {
            shreds_sigs_timestamps.insert(sig, timestamp);
        }
        for (timestamp, sig) in bench_sigs.iter() {
            if let Some(shreds_timestamp) = shreds_sigs_timestamps.get(sig) {
                match shreds_timestamp.cmp(&timestamp) {
                    std::cmp::Ordering::Equal => {}
                    std::cmp::Ordering::Less => {
                        slower_count += 1;
                    }
                    std::cmp::Ordering::Greater => {
                        faster_count += 1;
                    }
                }
            } else {
                miss_count += 1;
            }
        }

        info!("Benchmark sigs: {}", bench_sigs.len());
        info!("Shreds sigs: {}", shreds_sigs.len());
        info!("Miss count: {}", miss_count);
        info!("Slower count: {}", slower_count);
        info!("Faster count: {}", faster_count);

        return Ok(());
    }

    if cli.save {
        info!("Running in save mode");
        listener::run_listener_with_save(&cli.bind).await?;
    } else {
        info!("Running in algo mode");
        listener::run_listener_with_algo(&cli.bind).await?;
    }

    Ok(())
}
