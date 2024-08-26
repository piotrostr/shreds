use std::fs::File;
use std::io::Read;

pub fn setup() -> Result<(), Box<dyn std::error::Error>> {
    let random = grab_random_bytes();
    let log_file = File::create(format!("logs/{}.log", hex::encode(random)))?;
    println!("Logging to: {:?}", log_file);
    env_logger::Builder::default()
        .format_module_path(false)
        .filter_level(log::LevelFilter::Info)
        .format_timestamp_millis()
        .target(env_logger::Target::Pipe(Box::new(log_file)))
        .init();

    Ok(())
}

fn grab_random_bytes() -> [u8; 5] {
    let mut random = [0u8; 5];
    File::open("/dev/urandom")
        .expect("asdf")
        .read_exact(&mut random)
        .expect("asdf");
    random
}
