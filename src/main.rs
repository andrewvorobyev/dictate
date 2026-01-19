fn main() {
    if let Err(err) = dictate::run() {
        eprintln!("fatal: {err:#}");
        std::process::exit(1);
    }
}
