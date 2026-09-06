fn main() {
    if let Err(err) = apple_ship::run() {
        eprintln!("error: {err:#}");
        std::process::exit(1);
    }
}
