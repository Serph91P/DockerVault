use super::{Cancelled, Invalid, Result};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs::File,
    io::Write,
    os::{fd::AsRawFd, unix::fs::PermissionsExt},
};
use tokio_util::sync::CancellationToken;

pub(super) struct MaterializedFiles {
    _files: BTreeMap<String, File>,
    pub(super) environment: Vec<(OsString, OsString)>,
}
fn check(cancel: &CancellationToken) -> Result<()> {
    if cancel.is_cancelled() {
        Err(Cancelled)
    } else {
        Ok(())
    }
}
fn path(file: &File) -> String {
    format!("/proc/{}/fd/{}", std::process::id(), file.as_raw_fd())
}
fn replace(data: &[u8], needle: &[u8], value: &[u8]) -> Vec<u8> {
    let mut output = Vec::with_capacity(data.len());
    let mut rest = data;
    while let Some(offset) = rest.windows(needle.len()).position(|part| part == needle) {
        output.extend_from_slice(&rest[..offset]);
        output.extend_from_slice(value);
        rest = &rest[offset + needle.len()..];
    }
    output.extend_from_slice(rest);
    output
}
fn allocate(
    key: &str,
    records: &Value,
    allocated: &mut BTreeMap<String, File>,
    cancel: &CancellationToken,
) -> Result<String> {
    check(cancel)?;
    if let Some(file) = allocated.get(key) {
        return Ok(path(file));
    }
    let record = records.get(key).ok_or(Invalid)?;
    let mut data = zeroize::Zeroizing::new(
        STANDARD
            .decode(
                record
                    .get("content_b64")
                    .and_then(Value::as_str)
                    .ok_or(Invalid)?,
            )
            .map_err(|_| Invalid)?,
    );
    if let Some(dependencies) = record.get("dependencies") {
        for dependency in dependencies.as_array().ok_or(Invalid)? {
            let dependency = dependency.as_str().ok_or(Invalid)?;
            let child = allocate(dependency, records, allocated, cancel)?;
            data = zeroize::Zeroizing::new(replace(
                &data,
                format!("dv-recovery-file:{dependency}").as_bytes(),
                child.as_bytes(),
            ));
        }
    }
    // Parent-owned CLOEXEC descriptors avoid a global inheritable-FD window;
    // the guarded CLI opens only these explicit parent procfd paths.
    let mut file = File::from(
        nix::sys::memfd::memfd_create(
            "dv-provider-credential",
            nix::sys::memfd::MFdFlags::MFD_CLOEXEC,
        )
        .map_err(|_| Invalid)?,
    );
    file.set_permissions(std::fs::Permissions::from_mode(0o600))
        .map_err(|_| Invalid)?;
    for chunk in data.chunks(65536) {
        check(cancel)?;
        file.write_all(chunk).map_err(|_| Invalid)?;
    }
    let result = path(&file);
    allocated.insert(key.to_owned(), file);
    Ok(result)
}
impl MaterializedFiles {
    pub(super) fn create(
        files: &Value,
        references: &Value,
        cancel: &CancellationToken,
    ) -> Result<Self> {
        check(cancel)?;
        let references = references.as_object().ok_or(Invalid)?;
        let mut ids = Vec::new();
        for fields in references.values() {
            for reference in fields.as_object().ok_or(Invalid)?.values() {
                ids.push(reference.as_str().ok_or(Invalid)?.to_owned());
            }
        }
        let records = dockervault_core::rclone_package::collect_materials(files, &ids, &|| {
            cancel.is_cancelled()
        })?;
        let mut allocated = BTreeMap::new();
        let mut environment = Vec::new();
        for (name, fields) in references {
            for (field, reference) in fields.as_object().ok_or(Invalid)? {
                let file = allocate(
                    reference.as_str().ok_or(Invalid)?,
                    &records,
                    &mut allocated,
                    cancel,
                )?;
                environment.push((
                    dockervault_core::rclone_path::environment_name(name, field).into(),
                    file.into(),
                ));
            }
        }
        check(cancel)?;
        Ok(Self {
            _files: allocated,
            environment,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{Engine, engine::general_purpose::STANDARD};
    use serde_json::json;
    use sha2::{Digest, Sha256};
    use std::os::unix::fs::PermissionsExt;
    fn digest(bytes: &[u8]) -> String {
        Sha256::digest(bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }
    #[test]
    fn anonymous_credentials_are_dependency_rewritten_deduplicated_exec_readable_and_closed() {
        let child = b"PUBLIC binary provider credential\0data";
        let child_id = digest(child);
        let parent = format!("[PUBLIC]\nkey_file=dv-recovery-file:{child_id}\n");
        let parent_id = digest(parent.as_bytes());
        let files = json!({
            (child_id.clone()):{"kind":"provider_credential","content_b64":STANDARD.encode(child)},
            (parent_id.clone()):{"kind":"provider_credential","content_b64":STANDARD.encode(parent.as_bytes()),"dependencies":[child_id.clone()]}
        });
        let refs = json!({"PUBLIC":{"config_file":parent_id,"service_account_file":child_id},"OTHER":{"config_file":parent_id}});
        let cancel = tokio_util::sync::CancellationToken::new();
        let material = MaterializedFiles::create(&files, &refs, &cancel).unwrap();
        for file in material._files.values() {
            let flags = nix::fcntl::fcntl(file, nix::fcntl::FcntlArg::F_GETFD).unwrap();
            assert!(
                nix::fcntl::FdFlag::from_bits_truncate(flags)
                    .contains(nix::fcntl::FdFlag::FD_CLOEXEC)
            );
        }
        let env: std::collections::BTreeMap<_, _> = material.environment.iter().cloned().collect();
        let parent_path = std::path::PathBuf::from(
            &env[std::ffi::OsStr::new("RCLONE_CONFIG_PUBLIC_CONFIG_FILE")],
        );
        let child_path = std::path::PathBuf::from(
            &env[std::ffi::OsStr::new("RCLONE_CONFIG_PUBLIC_SERVICE_ACCOUNT_FILE")],
        );
        assert_eq!(
            env[std::ffi::OsStr::new("RCLONE_CONFIG_OTHER_CONFIG_FILE")],
            parent_path.as_os_str()
        );
        assert_eq!(std::fs::read(&child_path).unwrap(), child);
        let expected = format!("[PUBLIC]\nkey_file={}\n", child_path.display());
        let out = std::process::Command::new("/bin/cat")
            .arg(&parent_path)
            .output()
            .unwrap();
        assert!(out.status.success());
        assert_eq!(out.stdout, expected.as_bytes());
        assert_eq!(
            std::fs::metadata(&parent_path)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert!(
            std::fs::read_link(&parent_path)
                .unwrap()
                .to_string_lossy()
                .starts_with("/memfd:dv-provider-credential")
        );
        use std::os::unix::fs::MetadataExt;
        let identities: Vec<_> = [&parent_path, &child_path]
            .into_iter()
            .map(|path| {
                let info = std::fs::metadata(path).unwrap();
                (info.dev(), info.ino())
            })
            .collect();
        drop(material);
        // Other concurrent tests may reuse the descriptor number, never its
        // still-live private memfd identity.
        for (path, identity) in [&parent_path, &child_path].into_iter().zip(identities) {
            assert!(
                !std::fs::metadata(path).is_ok_and(|info| (info.dev(), info.ino()) == identity)
            );
        }
        let mut wrong = files.clone();
        wrong[&child_id]["content_b64"] = json!(STANDARD.encode(b"PUBLIC tampered"));
        assert!(MaterializedFiles::create(&wrong, &refs, &cancel).is_err());
        cancel.cancel();
        assert!(matches!(
            MaterializedFiles::create(&files, &refs, &cancel),
            Err(dockervault_core::rclone_package::RcloneError::Cancelled)
        ));
    }
}
