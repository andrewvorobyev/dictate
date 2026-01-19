fn main() {
    if let Err(err) = dictate_2::run() {
        eprintln!("fatal: {err:#}");
        std::process::exit(1);
    }
}
