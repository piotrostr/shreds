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
    pubsub: bool,
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

    if cli.pubsub {
        benchmark::listen_pubsub(vec![RAYDIUM_AMM.to_string()]).await?;
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
