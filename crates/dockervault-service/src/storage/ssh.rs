//! Path handling shared by the SSH and SFTP transports.
use std::io;

/// Resolve a child relative to the configured remote directory. A leading slash
/// on a child does not replace the configured base.
pub fn remote_path(host: &str, port: u16, base: &str, child: &str) -> io::Result<String> {
    if [base, child]
        .iter()
        .any(|path| path.chars().any(char::is_control))
        || child.split('/').any(|component| component == "..")
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Invalid SSH path",
        ));
    }
    let mut base = base.trim();
    if port == 23 && host.to_ascii_lowercase().ends_with(".your-storagebox.de") {
        base = base.trim_end_matches('/');
        base = match base {
            "" | "home" | "/home" | "backups" | "/backups" => ".",
            _ => base
                .strip_prefix("/home/")
                .unwrap_or_else(|| base.trim_start_matches('/')),
        };
    }
    let child = child.trim_start_matches('/');
    match (base, child) {
        ("", "") => Ok(".".into()),
        (_, "") => Ok(base.into()),
        ("" | ".", _) => Ok(child.into()),
        _ => Ok(format!("{}/{child}", base.trim_end_matches('/'))),
    }
}
