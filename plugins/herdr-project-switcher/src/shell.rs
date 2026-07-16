use std::path::Path;

use crate::config::WindowsShell;

pub fn cd_command(path: &Path, windows_shell: WindowsShell) -> String {
    let raw = path.to_string_lossy();
    if cfg!(windows) {
        match windows_shell {
            WindowsShell::Powershell => {
                format!("Set-Location -LiteralPath '{}'", raw.replace('\'', "''"))
            }
            WindowsShell::Cmd => format!("cd /d \"{}\"", raw.replace('"', "\"\"")),
        }
    } else {
        format!("cd '{}'", raw.replace('\'', r"'\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(not(windows))]
    fn quotes_posix_paths() {
        assert_eq!(
            cd_command(Path::new("/tmp/a'b"), WindowsShell::Powershell),
            "cd '/tmp/a'\\''b'"
        );
    }
}
