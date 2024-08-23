use shreds::listener;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::Builder::default()
        .format_module_path(false)
        .filter_level(log::LevelFilter::Info)
        .init();

    listener::run_listener_with_algo().await?;

    Ok(())
}
