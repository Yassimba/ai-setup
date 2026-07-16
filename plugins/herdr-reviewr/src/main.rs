fn main() -> anyhow::Result<()> {
    if std::env::args_os().nth(1).as_deref() == Some(std::ffi::OsStr::new("sidebar")) {
        let mode = std::env::args().nth(2).unwrap_or_default();
        std::process::exit(herdr_reviewr::sidebar::run(&mode));
    }
    herdr_reviewr::run()
}
