use clap::Parser;
use shreds::app::{App, Command};
use shreds::service::{self, Mode};
use std::sync::Arc;

use log::info;
use shreds::benchmark::compare_results;
use shreds::raydium::download_raydium_json;
use shreds::{benchmark, listener, logger};
use tokio::sync::RwLock;

use shreds::constants;

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenv::dotenv().ok();

    let app = App::parse();

    let log_target = app.args.log_target.unwrap();
    logger::setup(if log_target == "file" {
        logger::Target::File
    } else if log_target == "stdout" {
        logger::Target::Stdout
    } else {
        panic!("Invalid log target")
    })?;

    match app.command {
        Command::Save => {
            let bind = app.args.bind.unwrap();
            info!("Binding to address: {}", bind);

            info!("Running in save mode");
            listener::run_listener_with_save(&bind).await?;
        }
        Command::Download => {
            download_raydium_json(true).await?;
        }
        Command::Benchmark => {
            benchmark_cmd(app.args.bind.unwrap()).await?;
        }
        Command::Pubsub => {
            let pubsub_sigs = Arc::new(RwLock::new(Vec::new()));

            let pubsub_handle = tokio::spawn({
                let pubsub_sigs = pubsub_sigs.clone();
                async move {
                    tokio::time::sleep(tokio::time::Duration::from_secs(3))
                        .await;
                    benchmark::listen_pubsub(
                        vec![constants::RAYDIUM_AMM.to_string()],
                        pubsub_sigs,
                    )
                    .await
                    .expect("pubsub")
                }
            });

            tokio::signal::ctrl_c()
                .await
                .expect("Failed to listen for Ctrl+C");

            pubsub_handle.await?;
        }
        Command::ArbMode => {
            let bind = app.args.bind.unwrap();
            let post = app.args.post_url.unwrap();
            health_check(post.clone()).await?;
            info!("Binding to address: {}, posting to: {}", bind, post);
            service::run(bind, post, Mode::Arb).await?;
        }
        Command::PumpMode => {
            let bind = app.args.bind.unwrap();
            let post = app.args.post_url.unwrap();
            health_check(post.clone()).await?;
            info!("Binding to address: {}, posting to: {}", bind, post);
            service::run(bind, post, Mode::Pump).await?;
        }
        Command::GraduatesMode => {
            let bind = app.args.bind.unwrap();
            let post = app.args.post_url.unwrap();
            health_check(post.clone()).await?;
            info!("Binding to address: {}, posting to: {}", bind, post);
            service::run(bind, post, Mode::Graduates).await?;
        }
    }

    Ok(())
}

pub async fn health_check(
    post_url: String,
) -> Result<(), Box<dyn std::error::Error>> {
    info!("Running health check on: {}", post_url);

    let client = reqwest::Client::new();
    let response = client
        .get(post_url + "/healthz")
        .send()
        .await
        .expect("Failed to send request");

    if response.status().is_success() {
        Ok(())
    } else {
        Err("Failed health check".into())
    }
}

pub async fn benchmark_cmd(
    bind_addr: String,
) -> Result<(), Box<dyn std::error::Error>> {
    info!("Binding to address: {}", bind_addr);

    let pubsub_sigs = Arc::new(RwLock::new(Vec::new()));
    let shreds_sigs = Arc::new(RwLock::new(Vec::new()));

    let pubsub_handle = tokio::spawn({
        let pubsub_sigs = pubsub_sigs.clone();
        async move {
            tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
            benchmark::listen_pubsub(
                vec![constants::RAYDIUM_AMM.to_string()],
                pubsub_sigs,
            )
            .await
            .expect("pubsub")
        }
    });
    let shreds_handle = tokio::spawn({
        let shreds_sigs = shreds_sigs.clone();
        async move {
            listener::run_listener_with_algo(
                &bind_addr,
                Some(shreds_sigs),
                Mode::Arb,
                "".to_string(),
                true,
            )
            .await
            .expect("shreds")
        }
    });

    info!("Sleeping for 10 seconds...");
    tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;

    pubsub_handle.abort();
    shreds_handle.abort();

    compare_results(
        pubsub_sigs.read().await.clone(),
        shreds_sigs.read().await.clone(),
    );

    Ok(())
}
