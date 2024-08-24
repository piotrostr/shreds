use indicatif::{ProgressBar, ProgressStyle};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;

use clap::{command, Parser};
use log::info;
use shreds::algo::RAYDIUM_AMM;
use shreds::benchmark::compare_results;
use shreds::raydium::download_raydium_json;
use shreds::{benchmark, listener};
use tokio::sync::RwLock;

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

#[tokio::main(flavor = "multi_thread")]
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
        let pubsub_sigs = Arc::new(RwLock::new(Vec::new()));
        let shreds_sigs = Arc::new(RwLock::new(Vec::new()));

        let pubsub_handle = tokio::spawn({
            let pubsub_sigs = pubsub_sigs.clone();
            async move {
                tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
                benchmark::listen_pubsub(
                    vec![RAYDIUM_AMM.to_string()],
                    pubsub_sigs,
                )
                .await
                .expect("pubsub")
            }
        });
        let shreds_handle = tokio::spawn({
            let shreds_sigs = shreds_sigs.clone();
            async move {
                listener::run_listener_with_algo(&cli.bind, Some(shreds_sigs))
                    .await
                    .expect("shreds")
            }
        });

        info!("Sleeping for 10 seconds...");
        sleep_with_progress(10).await;

        pubsub_handle.abort();
        shreds_handle.abort();

        compare_results(
            pubsub_sigs.read().await.clone(),
            shreds_sigs.read().await.clone(),
        );

        return Ok(());
    } else if cli.save {
        info!("Running in save mode");
        listener::run_listener_with_save(&cli.bind).await?;
    } else {
        info!("Running in algo mode");
        listener::run_listener_with_algo(&cli.bind, None).await?;
    }

    Ok(())
}

async fn sleep_with_progress(seconds: u64) {
    let pb = ProgressBar::new(seconds);
    pb.set_style(ProgressStyle::default_bar()
        .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({eta})")
        .unwrap()
        .progress_chars("#>-"));

    for _ in 0..seconds {
        sleep(Duration::from_secs(1)).await;
        pb.inc(1);
    }

    pb.finish_with_message("Done!");
}
