use clap::{App, Arg};
use log::info;
use shreds::listener;
use shreds::raydium::download_raydium_json;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenv::dotenv().ok();

    let matches = App::new("shreds")
        .version("1.0")
        .author("piotrostr")
        .arg(
            Arg::with_name("bind")
                .short('b')
                .long("bind")
                .value_name("ADDRESS")
                .help("Sets the bind address")
                .takes_value(true)
                .default_value("0.0.0.0:8001"),
        )
        .arg(
            Arg::with_name("save")
                .short('s')
                .long("save")
                .help("Run in save mode (dump packets to file)")
                .takes_value(false),
        )
        .arg(
            Arg::with_name("download")
                .short('d')
                .long("download")
                .takes_value(false),
        )
        .get_matches();

    env_logger::Builder::default()
        .format_module_path(false)
        .filter_level(log::LevelFilter::Info)
        .init();

    let bind_addr = matches.value_of("bind").unwrap();
    info!("Binding to address: {}", bind_addr);

    if matches.is_present("download") {
        download_raydium_json(true).await?;
        return Ok(());
    }

    if matches.is_present("save") {
        info!("Running in save mode");
        listener::run_listener_with_save(bind_addr).await?;
    } else {
        info!("Running in algo mode");
        listener::run_listener_with_algo(bind_addr).await?;
    }

    Ok(())
}
