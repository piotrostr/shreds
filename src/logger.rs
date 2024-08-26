use std::fs::File;
use std::io::{Read, Write};

pub fn setup() -> Result<(), Box<dyn std::error::Error>> {
    // let random = grab_random_bytes();
    // let log_file = File::create(format!("shreds-{}.log", hex::encode(random)))?;
    let log_file = File::create("shreds.log")?;
    println!("Logging to: {:?}", log_file);
    env_logger::Builder::default()
        .format_module_path(false)
        .filter_level(log::LevelFilter::Info)
        .format(|buf, record| {
            writeln!(
                buf,
                "{} {} [{}] {}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_millis(),
                record.level(),
                record.target(),
                record.args()
            )
        })
        .target(env_logger::Target::Pipe(Box::new(log_file)))
        .init();

    Ok(())
}

pub fn grab_random_bytes() -> [u8; 5] {
    let mut random = [0u8; 5];
    File::open("/dev/urandom")
        .expect("asdf")
        .read_exact(&mut random)
        .expect("asdf");
    random
}
