//! Bounded file producers whose physical read lifetime is owned separately from HTTP futures.
use super::{VerifiedSource, check};
use crate::ownership::{Owner, Work, WorkError};
use futures_util::StreamExt;
use std::{
    io::{self, Read, Seek, SeekFrom},
    sync::Arc,
};
use tokio::sync::mpsc;

type SourceChunks = (mpsc::Receiver<Vec<u8>>, Work<io::Result<()>>);

pub(super) fn source_body(
    source: Arc<VerifiedSource>,
    owner: &Owner,
    offset: u64,
    length: u64,
) -> Result<(reqwest::Body, Work<io::Result<()>>), WorkError> {
    let (rx, work) = source_chunks(source, owner, offset, length)?;
    let stream = futures_util::stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|item| (Ok::<_, io::Error>(item), rx))
    });
    Ok((reqwest::Body::wrap_stream(stream.fuse()), work))
}

pub(super) fn source_chunks(
    source: Arc<VerifiedSource>,
    owner: &Owner,
    offset: u64,
    length: u64,
) -> Result<SourceChunks, WorkError> {
    let (tx, rx) = mpsc::channel(8);
    let runtime = tokio::runtime::Handle::current();
    let work = owner.submit_blocking(move |cancel| {
        let mut file = source.reopen()?;
        if offset.checked_add(length).is_none_or(|end| end > source.len()) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "Upload range outside source"));
        }
        file.seek(SeekFrom::Start(offset))?;
        let mut remaining = length;
        let mut buffer = [0u8;65536];
        while remaining > 0 {
            check(&||cancel.is_cancelled())?;
            let amount = remaining.min(buffer.len() as u64) as usize;
            let count = file.read(&mut buffer[..amount])?;
            if count == 0 { return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "Upload source truncated")); }
            remaining -= count as u64;
            let part = buffer[..count].to_vec();
            runtime.block_on(async {
                tokio::select! {
                    _ = cancel.cancelled() => Err(io::Error::new(io::ErrorKind::Interrupted, "Upload cancelled")),
                    sent = tx.send(part) => sent.map_err(|_|io::Error::new(io::ErrorKind::BrokenPipe, "Upload body closed")),
                }
            })?;
        }
        check(&||cancel.is_cancelled())?;
        source.check_unchanged()
    })?;
    Ok((rx, work))
}
