//! The legacy format embeds bytes between the identity and metadata fields.
//! Keep the descriptor readable by old releases, but store new bytes only by hash.
use super::types::{FileSnapshot, SnapshotError, SnapshotResult};
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

pub(crate) const CONTENT_LIMIT: usize = 8 * 1024 * 1024;
const HEADER_WINDOW: usize = 32 * 1024;
pub(super) static CONTENT_JOBS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);

pub(super) fn limit_error(path: &Path, limit: usize) -> SnapshotError {
    SnapshotError::ConfigError(format!(
        "Snapshot of {} exceeds the {} byte limit",
        path.display(),
        limit
    ))
}

pub(super) fn read_bounded(
    reader: impl Read,
    limit: usize,
    path: &Path,
) -> SnapshotResult<Vec<u8>> {
    let mut bytes = Vec::new();
    reader.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(limit_error(path, limit));
    }
    Ok(bytes)
}

/// Read the small descriptor without traversing a historical byte array.
/// For large files only recognize the layout emitted by to_string_pretty.
/// Other oversized layouts remain on disk and report an explicit error.
pub(super) fn descriptor(path: &Path) -> SnapshotResult<FileSnapshot> {
    let mut file = File::open(path)?;
    let len = file.metadata()?.len();
    let bytes = if len <= (2 * HEADER_WINDOW) as u64 {
        read_bounded(file, 2 * HEADER_WINDOW, path)?
    } else {
        let mut head = vec![0; HEADER_WINDOW];
        let mut tail = vec![0; HEADER_WINDOW];
        file.read_exact(&mut head)?;
        file.seek(SeekFrom::End(-(HEADER_WINDOW as i64)))?;
        file.read_exact(&mut tail)?;
        let start = b"\n  \"compressed_content\": [";
        let end = b"],\n  \"timestamp\":";
        let i = head
            .windows(start.len())
            .position(|w| w == start)
            .ok_or_else(|| limit_error(path, 2 * HEADER_WINDOW))?;
        let j = tail
            .windows(end.len())
            .rposition(|w| w == end)
            .ok_or_else(|| limit_error(path, 2 * HEADER_WINDOW))?;
        head.truncate(i + start.len());
        head.extend_from_slice(&tail[j..]);
        head
    };
    let mut record: FileSnapshot = serde_json::from_slice(&bytes)?;
    record.compressed_content = Vec::new();
    if record.content_hash.len() != 32
        || !record.content_hash.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(SnapshotError::ConfigError(format!(
            "Invalid snapshot content hash in {}",
            path.display()
        )));
    }
    Ok(record)
}

pub(super) fn write_atomic(path: &Path, bytes: &[u8]) -> SnapshotResult<()> {
    let tmp = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    let result = fs::write(&tmp, bytes).and_then(|()| fs::rename(&tmp, path));
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    Ok(result?)
}

/// A cancelled remote read must not leave its temporary compressed file behind.
pub(super) struct StagedBlob(std::path::PathBuf);
impl Drop for StagedBlob {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

pub(super) async fn stream_blob(
    workspace: &dyn openbitfun_runtime_ports::WorkspaceFileSystem,
    source: &Path,
    directory: &Path,
    limit: usize,
) -> SnapshotResult<(String, u64)> {
    use std::io::{BufWriter, Write};
    use tokio::io::AsyncReadExt;
    let mut reader = workspace
        .open_read(source.to_string_lossy().as_ref())
        .await?;
    let path = directory.join(format!("{}.tmp", uuid::Uuid::new_v4()));
    let file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&path)?;
    let stage = StagedBlob(path);
    let mut encoder =
        flate2::write::GzEncoder::new(BufWriter::new(file), flate2::Compression::fast());
    let mut digest = md5::Context::new();
    let mut buffer = vec![0; 64 * 1024];
    let mut size = 0u64;
    loop {
        let n = reader.read(&mut buffer).await?;
        if n == 0 {
            break;
        }
        size += n as u64;
        if size > limit as u64 {
            return Err(limit_error(source, limit));
        }
        digest.consume(&buffer[..n]);
        encoder.write_all(&buffer[..n])?;
    }
    encoder.finish()?.flush()?;
    let hash = format!("{:x}", digest.compute());
    let dest = directory.join(format!("{hash}.snap"));
    if !dest.exists() {
        fs::rename(&stage.0, dest)?;
    }
    Ok((hash, size))
}

/// Older cleanup could leave a baseline's embedded array as its only copy.
/// Extract it only for explicit restore. The encoded bytes are never added to
/// the shared store, since their integrity has not yet been verified.
pub(super) fn extract_legacy_blob(
    metadata_path: &Path,
    directory: &Path,
    expected_size: u64,
) -> SnapshotResult<(File, StagedBlob)> {
    use serde::de::{
        DeserializeSeed, Deserializer, Error, IgnoredAny, MapAccess, SeqAccess, Visitor,
    };
    use std::fmt;
    use std::io::{BufReader, BufWriter, Write};

    struct Bytes<'a>(&'a mut BufWriter<File>, u64);
    impl<'de> DeserializeSeed<'de> for Bytes<'_> {
        type Value = ();
        fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<(), D::Error> {
            d.deserialize_seq(self)
        }
    }
    impl<'de> Visitor<'de> for Bytes<'_> {
        type Value = ();
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("a legacy snapshot byte array")
        }
        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> {
            let mut remaining = self.1;
            while let Some(byte) = seq.next_element::<u8>()? {
                remaining = remaining
                    .checked_sub(1)
                    .ok_or_else(|| A::Error::custom("Legacy payload exceeds its recorded size"))?;
                self.0.write_all(&[byte]).map_err(A::Error::custom)?;
            }
            Ok(())
        }
    }

    struct Record<'a>(Bytes<'a>);
    impl<'de> Visitor<'de> for Record<'_> {
        type Value = ();
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("a legacy snapshot")
        }
        fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<(), M::Error> {
            let mut bytes = Some(self.0);
            while let Some(field) = map.next_key::<String>()? {
                if field == "compressed_content" {
                    map.next_value_seed(
                        bytes
                            .take()
                            .ok_or_else(|| M::Error::duplicate_field("compressed_content"))?,
                    )?;
                } else {
                    map.next_value::<IgnoredAny>()?;
                }
            }
            if bytes.is_some() {
                return Err(M::Error::missing_field("compressed_content"));
            }
            Ok(())
        }
    }
    let source = File::open(metadata_path)?;
    let max_json_bytes = expected_size.saturating_mul(9).saturating_add(65536);
    if source.metadata()?.len() > max_json_bytes {
        return Err(SnapshotError::ConfigError(
            "Legacy snapshot exceeds its encoded size budget".into(),
        ));
    }
    let path = directory.join(format!("{}.tmp", uuid::Uuid::new_v4()));
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&path)?;
    let stage = StagedBlob(path);
    let mut output = BufWriter::new(file);
    let mut de = serde_json::Deserializer::from_reader(BufReader::new(source.take(max_json_bytes)));
    de.deserialize_map(Record(Bytes(&mut output, expected_size)))?;
    de.end()?;
    let mut file = output.into_inner().map_err(|e| e.into_error())?;
    file.rewind()?;
    Ok((file, stage))
}
