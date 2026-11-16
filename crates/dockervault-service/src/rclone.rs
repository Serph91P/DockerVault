//! Owned offline rclone CLI capabilities; never inherit ambient credentials.
mod managed;
use dockervault_core::rclone_package::RcloneError;
pub use managed::ManagedConfigBuilder;
use std::collections::{BTreeMap, BTreeSet};
type Options = BTreeMap<String, BTreeSet<String>>;
const OUTPUT_LIMIT: usize = 8 * 1024 * 1024;
pub async fn provider_options(
    processes: &crate::process::ProcessOwner,
    blocking: &crate::ownership::Owner,
    binary: &std::path::Path,
    cancel: tokio_util::sync::CancellationToken,
) -> Result<Options, RcloneError> {
    use crate::process::ProcessSpec;
    if cancel.is_cancelled() {
        return Err(RcloneError::Cancelled);
    }
    let mut spec = ProcessSpec::new(
        binary,
        [
            "--config",
            "/dev/null",
            "--ask-password=false",
            "config",
            "providers",
        ],
    );
    spec.env = vec![
        ("PATH".into(), "/bin:/usr/bin:/usr/local/bin".into()),
        ("HOME".into(), "/nonexistent".into()),
        ("LANG".into(), "C.UTF-8".into()),
    ];
    spec.timeout = Some(std::time::Duration::from_secs(15));
    spec.stdout_limit = OUTPUT_LIMIT;
    let work = processes.start(spec).map_err(|_| RcloneError::Invalid)?;
    let physical_cancel = work.cancellation_token();
    let waiting = work.wait();
    tokio::pin!(waiting);
    let output = tokio::select! {
        biased;
        _ = cancel.cancelled() => {
            physical_cancel.cancel();
            let _ = waiting.await;
            return Err(RcloneError::Cancelled);
        },
        result = &mut waiting => result
            .map_err(|_| RcloneError::Invalid)?
            .map_err(|_| RcloneError::Invalid)?,
    };
    if !output.status.success() {
        return Err(RcloneError::Invalid);
    }
    blocking
        .submit_blocking(move |physical| {
            parse_options(&output.stdout, || {
                physical.is_cancelled() || cancel.is_cancelled()
            })
        })
        .map_err(|_| RcloneError::Invalid)?
        .wait()
        .await
        .map_err(|_| RcloneError::Invalid)?
}
fn parse_options(data: &[u8], cancelled: impl Fn() -> bool) -> Result<Options, RcloneError> {
    use RcloneError::{Cancelled, Invalid};
    if cancelled() {
        return Err(Cancelled);
    }
    if data.len() > OUTPUT_LIMIT {
        return Err(Invalid);
    }
    let raw: serde_json::Value = serde_json::from_slice(data).map_err(|_| Invalid)?;
    let mut result = Options::new();
    for provider in raw.as_array().ok_or(Invalid)? {
        if cancelled() {
            return Err(Cancelled);
        }
        let name = provider
            .get("Name")
            .and_then(serde_json::Value::as_str)
            .ok_or(Invalid)?;
        let mut names = BTreeSet::new();
        for option in provider
            .get("Options")
            .and_then(serde_json::Value::as_array)
            .ok_or(Invalid)?
        {
            if cancelled() {
                return Err(Cancelled);
            }
            names.insert(
                option
                    .get("Name")
                    .and_then(serde_json::Value::as_str)
                    .ok_or(Invalid)?
                    .to_owned(),
            );
        }
        result.insert(name.to_owned(), names);
    }
    if cancelled() {
        return Err(Cancelled);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projects_cli_metadata_like_the_accepted_provider_without_secret_values() {
        let raw=br#"[{"Name":"s3","Description":"ignored","Options":[{"Name":"region","Default":"PUBLIC"},{"Name":"secret_access_key"},{"Name":"region"}]},{"Name":"local","Options":[]}]"#;
        let actual = parse_options(raw, || false).unwrap();
        assert_eq!(
            actual,
            BTreeMap::from([
                (
                    "s3".into(),
                    BTreeSet::from(["region".into(), "secret_access_key".into()])
                ),
                ("local".into(), BTreeSet::new())
            ])
        );
        assert_eq!(parse_options(br#"[{"Name":"x","Options":[{"Name":"old"}]},{"Name":"x","Options":[{"Name":"new"}]}]"#,||false).unwrap()["x"],BTreeSet::from(["new".into()]));
    }
    #[test]
    fn rejects_invalid_metadata_and_settles_cancellation() {
        for raw in [
            b"null".as_slice(),
            b"{}",
            b"[null]",
            b"[{\"Name\":\"x\"}]",
            b"[{\"Name\":\"x\",\"Options\":[{}]}]",
            b"[{\"Name\":1,\"Options\":[]}]",
            b"[{\"Name\":\"x\",\"Options\":null}]",
            b"[] trailing",
        ] {
            assert_eq!(parse_options(raw, || false), Err(RcloneError::Invalid));
        }
        assert_eq!(parse_options(b"[]", || true), Err(RcloneError::Cancelled));
        assert_eq!(
            parse_options(&vec![b' '; OUTPUT_LIMIT + 1], || false),
            Err(RcloneError::Invalid)
        );
    }
}
