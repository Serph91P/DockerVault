use dockervault_tools::archive_worker::{MAX_REQUEST_BYTES, Report, Request, create_archive};
use std::{
    fs::File,
    io::{self, Read, Write},
    os::unix::ffi::OsStrExt,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

fn main() {
    let signal = Arc::new(AtomicUsize::new(0));
    let result = run(&signal);
    let mut stdout = io::BufWriter::new(io::stdout().lock());
    let code = match result {
        Ok(mut report) => {
            if write_report(&mut stdout, &mut report)
                .and_then(|()| stdout.flush())
                .is_ok()
            {
                0
            } else {
                1
            }
        }
        Err(message) => {
            let value = serde_json::json!({"status":"error","message":message});
            let _ = serde_json::to_writer(&mut stdout, &value);
            let _ = stdout.flush();
            let number = signal.load(Ordering::SeqCst);
            if number == 0 { 1 } else { 128 + number as i32 }
        }
    };
    std::process::exit(code);
}

fn run(signal: &Arc<AtomicUsize>) -> Result<Report, &'static str> {
    if std::env::args_os().len() != 1 {
        return Err("Archive worker accepts only its versioned stdin request");
    }
    if nix::unistd::geteuid().is_root()
        && let Ok(caller) = std::env::var("SUDO_UID")
    {
        let caller = caller
            .parse::<u32>()
            .map_err(|_| "Invalid archive caller identity")?;
        if caller == u32::MAX {
            return Err("Invalid archive caller identity");
        }
        // kill(2) authorizes the admitted owner via real UID; effective/saved root retain only this helper's I/O privileges.
        nix::unistd::setresuid(
            nix::unistd::Uid::from_raw(caller),
            nix::unistd::Uid::from_raw(0),
            nix::unistd::Uid::from_raw(0),
        )
        .map_err(|_| "Cannot retain archive process ownership")?;
    }
    for number in [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM] {
        signal_hook::flag::register_usize(number, Arc::clone(signal), number as usize)
            .map_err(|_| "Cannot register archive cancellation")?;
    }
    let mut input = Vec::new();
    io::stdin()
        .lock()
        .take((MAX_REQUEST_BYTES + 1) as u64)
        .read_to_end(&mut input)
        .map_err(|_| "Cannot read archive request")?;
    let request = Request::parse(&input)?;
    if !nix::unistd::geteuid().is_root() {
        return Err("Archive worker must run through its fixed root policy");
    }
    let flags = nix::fcntl::OFlag::O_RDONLY
        | nix::fcntl::OFlag::O_DIRECTORY
        | nix::fcntl::OFlag::O_NOFOLLOW
        | nix::fcntl::OFlag::O_CLOEXEC;
    let backups = File::from(
        nix::fcntl::open("/backups", flags, nix::sys::stat::Mode::empty())
            .map_err(|_| "Cannot open fixed backup directory")?,
    );
    create_archive(&request, &backups, &|| signal.load(Ordering::SeqCst) != 0)
        .map_err(|_| "Archive creation failed or cancelled")
}

fn write_report(output: &mut impl Write, report: &mut Report) -> io::Result<()> {
    output.write_all(b"{\"status\":\"ok\",\"skipped_files\":[")?;
    for (index, path) in report.skipped_files()?.enumerate() {
        let path = path?;
        if index != 0 {
            output.write_all(b",")?;
        }
        write_path(output, &path)?;
    }
    output.write_all(b"]}")
}

// Preserve Python filesystem surrogateescape in the existing JSON string array.
fn write_path(output: &mut impl Write, path: &Path) -> io::Result<()> {
    output.write_all(b"\"")?;
    let mut bytes = path.as_os_str().as_bytes();
    while !bytes.is_empty() {
        let (valid, invalid) = match std::str::from_utf8(bytes) {
            Ok(_) => (bytes.len(), 0),
            Err(error) => (
                error.valid_up_to(),
                error
                    .error_len()
                    .unwrap_or(bytes.len() - error.valid_up_to()),
            ),
        };
        let text = std::str::from_utf8(&bytes[..valid]).map_err(io::Error::other)?;
        let encoded = serde_json::to_vec(text).map_err(io::Error::other)?;
        output.write_all(&encoded[1..encoded.len() - 1])?;
        for byte in &bytes[valid..valid + invalid] {
            write!(output, "\\udc{byte:02x}")?;
        }
        bytes = &bytes[valid + invalid..];
    }
    output.write_all(b"\"")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::ffi::OsStringExt;

    #[test]
    fn spooled_report_keeps_the_complete_existing_json_array_above_capture_limit() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("dv-report-json-{}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        let source = root.join("source");
        let backups = root.join("backups");
        std::fs::create_dir(&source).unwrap();
        std::fs::create_dir(&backups).unwrap();
        for i in 0..2500 {
            let p = source.join(format!("denied-{i:04}"));
            std::fs::write(&p, b"PUBLIC unavailable").unwrap();
            std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o000)).unwrap();
        }
        let request=Request::parse(&serde_json::to_vec(&serde_json::json!({"version":1,"sources":[["data",source]],"dest":"/backups/report.tar","compress":false,"allowed_sources":[source]})).unwrap()).unwrap();
        let mut report =
            create_archive(&request, &File::open(&backups).unwrap(), &|| false).unwrap();
        let mut output = Vec::new();
        write_report(&mut output, &mut report).unwrap();
        assert!(output.len() > 65536);
        let value: serde_json::Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(value["status"], "ok");
        let paths = value["skipped_files"].as_array().unwrap();
        assert_eq!(paths.len(), 2500);
        let unique: std::collections::BTreeSet<_> =
            paths.iter().map(|v| v.as_str().unwrap()).collect();
        assert_eq!(unique.len(), 2500);
        for i in 0..2500 {
            assert!(unique.contains(source.join(format!("denied-{i:04}")).to_str().unwrap()));
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn skipped_paths_keep_surrogateescaped_bytes_and_json_metacharacters() {
        let path = std::path::PathBuf::from(std::ffi::OsString::from_vec(vec![
            b'a', 0xff, b'\n', b'"', b'\\',
        ]));
        let mut bytes = Vec::new();
        write_path(&mut bytes, &path).unwrap();
        assert_eq!(bytes, b"\"a\\udcff\\n\\\"\\\\\"");
    }
}
