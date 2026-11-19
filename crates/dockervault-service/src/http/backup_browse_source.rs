use super::{BrowseKey, HttpState, with_archive_data, with_local_archive};
use crate::http::storage::{completed_archive_copies, saved_archive_source, snapshot_for_download};
use dockervault_core::crypto::key_sidecar_path;
use std::{fs::File, io, path::Path, sync::Arc};
use tokio::sync::OwnedSemaphorePermit;

pub(super) struct Archive {
    pub id: i64,
    pub path: String,
    pub sidecar: Option<String>,
    pub secret: Option<BrowseKey>,
}

pub(super) async fn run_archive<T: Send + 'static>(
    state: &HttpState,
    input: Archive,
    slot: Arc<OwnedSemaphorePermit>,
    task: impl Fn(&mut File, &Path, &dyn Fn() -> bool) -> io::Result<T> + Send + Sync + 'static,
) -> io::Result<T> {
    let root = Arc::new(std::path::absolute(&state.config.backup_base_path)?);
    let input = Arc::new(input);
    let task = Arc::new(task);
    let (local, local_root, local_task, local_slot) =
        (input.clone(), root.clone(), task.clone(), slot.clone());
    let result = state
        .download_jobs
        .submit_blocking(move |token| {
            let _slot = local_slot;
            if local.path.is_empty() {
                return Err(io::ErrorKind::NotFound.into());
            }
            let encryption = local
                .secret
                .as_ref()
                .map(|key| (local.sidecar.as_deref(), key.private_key.as_str()));
            let root = local_root.to_str().ok_or(io::ErrorKind::InvalidInput)?;
            with_local_archive(
                root,
                &local.path,
                encryption,
                &|| token.is_cancelled(),
                |file, root| local_task(file, root, &|| token.is_cancelled()),
            )
        })
        .map_err(|_| io::ErrorKind::WouldBlock)?
        .wait()
        .await
        .map_err(|_| io::ErrorKind::Interrupted)?;
    if !matches!(&result, Err(error) if error.kind() == io::ErrorKind::NotFound) {
        return result;
    }
    let expected = sqlx::query_as::<_, (Option<i64>, Option<String>)>(
        "SELECT file_size,checksum FROM backups WHERE id=? AND status='COMPLETED'",
    )
    .bind(input.id)
    .fetch_optional(state.auth.pool())
    .await
    .map_err(io::Error::other)?
    .ok_or(io::ErrorKind::NotFound)?;
    let encrypted = input.secret.is_some();
    let verify = (
        expected.0,
        if encrypted { None } else { expected.1.clone() },
    );
    let mut failure = io::ErrorKind::NotFound.into();
    for (id, path) in completed_archive_copies(state, input.id)
        .await
        .map_err(io::Error::other)?
    {
        let Some(source) = saved_archive_source(state, id).await else {
            continue;
        };
        let Some((file, len, _, _)) =
            snapshot_for_download(state, source.clone(), path.clone(), slot.clone()).await
        else {
            continue;
        };
        let Some(mut archive) =
            super::super::download::verify_archive(state, file, len, &verify, slot.clone()).await
        else {
            continue;
        };
        let sidecar = if encrypted {
            let Some((file, 1..=65536, _, _)) =
                snapshot_for_download(state, source, key_sidecar_path(&path), slot.clone()).await
            else {
                continue;
            };
            Some(file)
        } else {
            None
        };
        let (copy, copy_root, copy_task, copy_slot) =
            (input.clone(), root.clone(), task.clone(), slot.clone());
        let checksum = expected.1.clone().filter(|_| encrypted);
        let result = state
            .download_jobs
            .submit_blocking(move |token| {
                let _slot = copy_slot;
                let cancelled = || token.is_cancelled();
                let key = sidecar
                    .zip(copy.secret.as_ref())
                    .map(|(file, secret)| (file, secret.private_key.as_str()));
                with_archive_data(
                    &mut archive,
                    &path,
                    &copy_root,
                    key,
                    checksum.as_deref(),
                    &cancelled,
                    |file, root| copy_task(file, root, &cancelled),
                )
            })
            .map_err(|_| io::ErrorKind::WouldBlock)?
            .wait()
            .await
            .map_err(|_| io::ErrorKind::Interrupted)?;
        match result {
            Ok(value) => return Ok(value),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => return Err(error),
            Err(error) => failure = error,
        }
    }
    Err(failure)
}
