//! Small helpers for locating external command-line tools.

/// Whether `name` resolves to an executable on `PATH` — a dependency-free `which`. On unix a
/// file in a `PATH` directory is the executable; Windows installs it as `name.exe`. Shared by
/// the clipboard probe (`export.rs`) and the URL-opener probe (`browser.rs`).
#[must_use]
pub fn on_path(name: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|path| {
        std::env::split_paths(&path).any(|dir| {
            dir.join(name).is_file() || (cfg!(windows) && dir.join(format!("{name}.exe")).is_file())
        })
    })
}
