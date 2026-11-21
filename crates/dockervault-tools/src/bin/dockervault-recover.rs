//! Standalone recovery of the published archive/age sidecar formats, not a system bundle.
use dockervault_core::crypto::{CryptoError, decrypt_dek_file, decrypt_file};
use std::{
    ffi::OsString,
    fs::File,
    io::BufReader,
    path::PathBuf,
    process::ExitCode,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
const USAGE: &str = "Usage: dockervault-recover --archive FILE --output FILE --identity FILE --key FILE\n\nRecover existing DockerVault v1 or explicitly supported legacy CBC archives.\nRequires no server, database, Python, age/OpenSSL executable, or network.\nLegacy CBC archives lack authentication; successful padding is not proof of authenticity.";
struct Options {
    archive: PathBuf,
    output: PathBuf,
    identity: PathBuf,
    key: PathBuf,
}
fn parse() -> Result<Option<Options>, &'static str> {
    let mut args = std::env::args_os().skip(1);
    let (mut archive, mut output, mut identity, mut key) = (None, None, None, None);
    while let Some(arg) = args.next() {
        if arg == "--help" || arg == "-h" {
            return Ok(None);
        }
        let field = match arg.to_str() {
            Some("--archive") => &mut archive,
            Some("--output") => &mut output,
            Some("--identity") => &mut identity,
            Some("--key") => &mut key,
            _ => return Err("Unknown argument"),
        };
        let value: OsString = args.next().ok_or("Missing argument value")?;
        *field = Some(PathBuf::from(value));
    }
    Ok(Some(Options {
        archive: archive.ok_or("--archive is required")?,
        output: output.ok_or("--output is required")?,
        identity: identity.ok_or("--identity is required")?,
        key: key.ok_or("--key is required")?,
    }))
}
fn recover(options: Options, signal: &AtomicUsize) -> Result<(), CryptoError> {
    if signal.load(Ordering::SeqCst) != 0 {
        return Err(CryptoError::Cancelled);
    }
    let material = decrypt_dek_file(
        File::open(options.key)?,
        BufReader::new(File::open(options.identity)?),
    )?;
    if material.len() == 32 {
        eprintln!(
            "Warning: legacy raw key envelope permits unauthenticated CBC; padding is not an integrity guarantee."
        );
    }
    decrypt_file(&options.archive, &options.output, &material, &|| {
        signal.load(Ordering::SeqCst) != 0
    })
}
fn main() -> ExitCode {
    let options = match parse() {
        Ok(Some(options)) => options,
        Ok(None) => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Err(error) => {
            eprintln!("{error}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let signal = Arc::new(AtomicUsize::new(0));
    for number in [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM] {
        if signal_hook::flag::register_usize(number, signal.clone(), number as usize).is_err() {
            eprintln!("Recovery failed: signal supervision unavailable");
            return ExitCode::FAILURE;
        }
    }
    match recover(options, &signal) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Recovery failed: {error}");
            let number = signal.load(Ordering::SeqCst);
            ExitCode::from(if number == 0 { 1 } else { (128 + number) as u8 })
        }
    }
}
