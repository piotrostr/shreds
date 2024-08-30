use clap::{arg, Parser};
use serde::Deserialize;

#[derive(Parser, Debug)]
pub struct App {
    #[clap(flatten)]
    pub args: Args,

    #[clap(subcommand)]
    pub command: Command,
}

#[derive(Parser, Debug, Deserialize)]
#[command(name = "shreds", version = "1.0", author = "piotrostr")]
pub struct Args {
    /// Sets the bind address
    #[arg(short, long, default_value = "0.0.0.0:8001")]
    pub bind: Option<String>,

    /// URL to send webhooks to
    #[arg(long, default_value = "http://0.0.0.0:6969")]
    pub post_url: Option<String>,

    #[arg(short, long, default_value = "stdout")]
    pub log_target: Option<String>,
}

#[derive(Debug, Parser)]
pub enum Command {
    /// Run in save mode (dump packets to file)
    Save,

    /// Download Raydium JSON
    Download,

    /// Run benchmark
    Benchmark,

    /// Run in pubsub mode
    Pubsub,

    /// Run in service mode (sends pump webhooks to `post_url`)
    PumpMode,

    /// Run in arb mode (listens for raydium txs)
    ArbMode,
}
