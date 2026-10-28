use openbitfun_runtime_ports::{ExecOutputChunk, ExecOutputStream, ExecTerminalSize};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::{mpsc, Mutex};
use tokio::time::Instant;

pub const BACKGROUND_COMMAND_OUTPUT_CAPTURE_LIMIT_BYTES: usize = 1024 * 1024;
pub const BACKGROUND_COMMAND_OUTPUT_COMPLETED_TTL: Duration = Duration::from_secs(30 * 60);
pub const MAX_BACKGROUND_COMMAND_OUTPUT_RECORDS: usize = 128;

/// Human-facing summary of the capture retention policy, embedded in
/// not-found answers so the model can self-correct instead of retrying.
pub fn background_command_output_retention_policy_message() -> String {
    format!(
        "Captured command output is retained per command up to {} KB, for {} minutes after the command completes, and for at most {} commands.",
        BACKGROUND_COMMAND_OUTPUT_CAPTURE_LIMIT_BYTES / 1024,
        BACKGROUND_COMMAND_OUTPUT_COMPLETED_TTL.as_secs() / 60,
        MAX_BACKGROUND_COMMAND_OUTPUT_RECORDS
    )
}

static BACKGROUND_COMMAND_OUTPUT_CAPTURE: OnceLock<Arc<BackgroundCommandOutputCapture>> =
    OnceLock::new();

pub fn background_command_output_capture() -> Arc<BackgroundCommandOutputCapture> {
    BACKGROUND_COMMAND_OUTPUT_CAPTURE
        .get_or_init(|| Arc::new(BackgroundCommandOutputCapture::default()))
        .clone()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundCommandOutputStatus {
    Running,
    Exited,
    Interrupted,
    Killed,
    Pruned,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct BackgroundCommandSessionKey {
    remote: bool,
    exec_session_id: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackgroundCommandOutputMetadata {
    pub agent_session_id: Option<String>,
    pub exec_session_id: Option<i32>,
    pub command: String,
    pub workdir: Option<String>,
    pub remote: bool,
    pub tty: bool,
    /// Explicit null means unknown geometry; absent on older hosts.
    #[serde(default)]
    pub terminal_size: Option<ExecTerminalSize>,
    pub status: BackgroundCommandOutputStatus,
    pub exit_code: Option<i32>,
    pub started_at: u64,
    pub ended_at: Option<u64>,
    pub retained_bytes: usize,
    pub retained_limit_bytes: usize,
    pub truncated_from_start: bool,
}

#[derive(Debug, Clone)]
pub struct StartBackgroundCommandOutputCapture {
    /// Private backend key used before the exec session id is known.
    pub capture_id: String,
    pub agent_session_id: Option<String>,
    pub command: String,
    pub workdir: Option<String>,
    pub remote: bool,
    pub tty: bool,
    pub terminal_size: Option<ExecTerminalSize>,
}

#[derive(Debug, Clone)]
pub struct ReadBackgroundCommandOutputRequest {
    pub exec_session_id: i32,
    pub remote: bool,
    pub cursor: Option<u64>,
}

/// Addressable read keyed by the capture id (the originating tool call id)
/// instead of the exec session id. Used by the model-facing read tool because
/// a command that finishes inside its first tool call never gets an
/// exec-session link.
#[derive(Debug, Clone)]
pub struct ReadBackgroundCommandOutputRefRequest {
    pub capture_id: String,
    pub cursor: Option<u64>,
    /// `Stdout`/`Stderr` keep only that stream's chunks; `None`/`Combined`
    /// keep the combined arrival-ordered view.
    pub stream: Option<ExecOutputStream>,
}

#[derive(Debug, Clone)]
pub struct ListBackgroundCommandOutputRequest {
    pub agent_session_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadBackgroundCommandOutputResponse {
    pub metadata: BackgroundCommandOutputMetadata,
    pub cursor: u64,
    pub reset: bool,
    pub snapshot: Option<String>,
    pub chunks: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListBackgroundCommandOutputResponse {
    pub activities: Vec<BackgroundCommandOutputMetadata>,
}

#[derive(Default)]
pub struct BackgroundCommandOutputCapture {
    records: Mutex<HashMap<String, BackgroundCommandOutputRecord>>,
    session_index: Mutex<HashMap<BackgroundCommandSessionKey, String>>,
}

struct BackgroundCommandOutputRecord {
    agent_session_id: Option<String>,
    command: String,
    workdir: Option<String>,
    remote: bool,
    tty: bool,
    terminal_size: Option<ExecTerminalSize>,
    exec_session_id: Option<i32>,
    status: BackgroundCommandOutputStatus,
    exit_code: Option<i32>,
    started_at: u64,
    ended_at: Option<u64>,
    completed_at: Option<Instant>,
    chunks: VecDeque<BackgroundCommandOutputChunk>,
    next_cursor: u64,
    retained_bytes: usize,
    truncated_from_start: bool,
}

struct BackgroundCommandOutputChunk {
    cursor: u64,
    stream: ExecOutputStream,
    text: String,
    bytes: usize,
}

impl BackgroundCommandOutputCapture {
    pub async fn start_capture(
        self: &Arc<Self>,
        request: StartBackgroundCommandOutputCapture,
    ) -> mpsc::UnboundedSender<ExecOutputChunk> {
        self.cleanup_expired().await;
        let (tx, mut rx) = mpsc::unbounded_channel::<ExecOutputChunk>();
        let record = BackgroundCommandOutputRecord {
            agent_session_id: request.agent_session_id,
            command: request.command,
            workdir: request.workdir,
            remote: request.remote,
            tty: request.tty,
            terminal_size: request.terminal_size.filter(|_| request.tty),
            exec_session_id: None,
            status: BackgroundCommandOutputStatus::Running,
            exit_code: None,
            started_at: now_unix_seconds(),
            ended_at: None,
            completed_at: None,
            chunks: VecDeque::new(),
            next_cursor: 0,
            retained_bytes: 0,
            truncated_from_start: false,
        };
        {
            let mut records = self.records.lock().await;
            records.insert(request.capture_id.clone(), record);
            prune_record_count(&mut records);
        }

        let capture = Arc::clone(self);
        tokio::spawn(async move {
            while let Some(chunk) = rx.recv().await {
                capture.append_chunk(&request.capture_id, chunk).await;
            }
        });

        tx
    }

    pub async fn set_session_id(&self, capture_id: &str, exec_session_id: Option<i32>) {
        let mut records = self.records.lock().await;
        let Some(record) = records.get_mut(capture_id) else {
            return;
        };

        let old_key = record
            .exec_session_id
            .map(|id| BackgroundCommandSessionKey {
                remote: record.remote,
                exec_session_id: id,
            });
        record.exec_session_id = exec_session_id;
        let new_key = record
            .exec_session_id
            .map(|id| BackgroundCommandSessionKey {
                remote: record.remote,
                exec_session_id: id,
            });
        drop(records);

        let mut session_index = self.session_index.lock().await;
        if let Some(old_key) = old_key {
            session_index.remove(&old_key);
        }
        if let Some(new_key) = new_key {
            session_index.insert(new_key, capture_id.to_string());
        }
    }

    pub async fn update_lifecycle(
        &self,
        capture_id: &str,
        exec_session_id: i32,
        status: BackgroundCommandOutputStatus,
        exit_code: Option<i32>,
    ) -> Option<BackgroundCommandOutputMetadata> {
        let mut records = self.records.lock().await;
        let record = records.get_mut(capture_id)?;
        record.exec_session_id = Some(exec_session_id);
        record.status = status;
        record.exit_code = exit_code;
        if status != BackgroundCommandOutputStatus::Running {
            if record.ended_at.is_none() {
                record.ended_at = Some(now_unix_seconds());
            }
            if record.completed_at.is_none() {
                record.completed_at = Some(Instant::now());
            }
        }
        let metadata = record.metadata();
        let key = BackgroundCommandSessionKey {
            remote: record.remote,
            exec_session_id,
        };
        drop(records);

        self.session_index
            .lock()
            .await
            .insert(key, capture_id.to_string());

        Some(metadata)
    }

    pub async fn finish(
        &self,
        capture_id: &str,
        status: BackgroundCommandOutputStatus,
        exit_code: Option<i32>,
    ) -> Option<BackgroundCommandOutputMetadata> {
        let mut records = self.records.lock().await;
        let record = records.get_mut(capture_id)?;
        finish_record(record, status, exit_code);
        Some(record.metadata())
    }

    pub async fn finish_by_session(
        &self,
        remote: bool,
        exec_session_id: i32,
        status: BackgroundCommandOutputStatus,
        exit_code: Option<i32>,
    ) -> Option<BackgroundCommandOutputMetadata> {
        let capture_id = self.capture_id_for_session(remote, exec_session_id).await?;
        self.finish(&capture_id, status, exit_code).await
    }

    pub async fn read(
        &self,
        request: ReadBackgroundCommandOutputRequest,
    ) -> Option<ReadBackgroundCommandOutputResponse> {
        self.cleanup_expired_except_session(request.remote, request.exec_session_id)
            .await;

        let capture_id = self
            .capture_id_for_session(request.remote, request.exec_session_id)
            .await?;
        let records = self.records.lock().await;
        let record = records.get(&capture_id)?;
        Some(build_read_response(record, request.cursor, None))
    }

    /// Addressable read by capture id, optionally filtered to one stream.
    pub async fn read_by_ref(
        &self,
        request: ReadBackgroundCommandOutputRefRequest,
    ) -> Option<ReadBackgroundCommandOutputResponse> {
        let records = self.records.lock().await;
        let record = records.get(&request.capture_id)?;
        Some(build_read_response(record, request.cursor, request.stream))
    }

    /// Metadata for a capture id without consuming anything; the read tool
    /// uses this to answer list-style lookups and not-found errors.
    pub async fn metadata_by_ref(
        &self,
        capture_id: &str,
    ) -> Option<BackgroundCommandOutputMetadata> {
        let records = self.records.lock().await;
        records
            .get(capture_id)
            .map(BackgroundCommandOutputRecord::metadata)
    }

    pub async fn list(
        &self,
        request: ListBackgroundCommandOutputRequest,
    ) -> ListBackgroundCommandOutputResponse {
        let activities = self
            .list_with_capture_ids(request)
            .await
            .into_iter()
            .map(|(_, metadata)| metadata)
            .collect::<Vec<_>>();
        ListBackgroundCommandOutputResponse { activities }
    }

    /// Like [`Self::list`] but keeps the capture id (the model-facing
    /// `output_ref`) next to each activity so reads can be addressed.
    pub async fn list_with_capture_ids(
        &self,
        request: ListBackgroundCommandOutputRequest,
    ) -> Vec<(String, BackgroundCommandOutputMetadata)> {
        self.cleanup_expired().await;

        let records = self.records.lock().await;
        let mut activities = records
            .iter()
            .filter(|(capture_id, record)| {
                record.exec_session_id.is_some()
                    && !capture_id.is_empty()
                    && request.agent_session_id.as_ref().is_none_or(|session_id| {
                        record.agent_session_id.as_deref() == Some(session_id.as_str())
                    })
            })
            .map(|(capture_id, record)| (capture_id.clone(), record.metadata()))
            .collect::<Vec<_>>();
        activities.sort_by_key(|(_, metadata)| (metadata.started_at, metadata.exec_session_id));
        activities
    }

    /// Resolves the capture id (model-facing `output_ref`) of a still-tracked
    /// exec session.
    pub async fn capture_id_for_exec_session(
        &self,
        remote: bool,
        exec_session_id: i32,
    ) -> Option<String> {
        self.capture_id_for_session(remote, exec_session_id).await
    }

    async fn capture_id_for_session(&self, remote: bool, exec_session_id: i32) -> Option<String> {
        self.session_index
            .lock()
            .await
            .get(&BackgroundCommandSessionKey {
                remote,
                exec_session_id,
            })
            .cloned()
    }

    async fn append_chunk(&self, capture_id: &str, chunk: ExecOutputChunk) {
        if chunk.text.is_empty() {
            return;
        }

        let bytes = chunk.text.len();
        let mut records = self.records.lock().await;
        let Some(record) = records.get_mut(capture_id) else {
            return;
        };
        record.chunks.push_back(BackgroundCommandOutputChunk {
            cursor: record.next_cursor,
            stream: chunk.stream,
            text: chunk.text,
            bytes,
        });
        record.next_cursor = record.next_cursor.saturating_add(1);
        record.retained_bytes = record.retained_bytes.saturating_add(bytes);
        while record.retained_bytes > BACKGROUND_COMMAND_OUTPUT_CAPTURE_LIMIT_BYTES {
            if let Some(dropped) = record.chunks.pop_front() {
                record.retained_bytes = record.retained_bytes.saturating_sub(dropped.bytes);
                record.truncated_from_start = true;
            } else {
                break;
            }
        }
    }

    async fn cleanup_expired(&self) {
        self.cleanup_expired_except_capture("").await;
    }

    async fn cleanup_expired_except_session(&self, remote: bool, exec_session_id: i32) {
        if let Some(capture_id) = self.capture_id_for_session(remote, exec_session_id).await {
            self.cleanup_expired_except_capture(&capture_id).await;
        } else {
            self.cleanup_expired().await;
        }
    }

    async fn cleanup_expired_except_capture(&self, keep_capture_id: &str) {
        let now = Instant::now();
        let mut removed_keys = Vec::new();
        {
            let mut records = self.records.lock().await;
            records.retain(|capture_id, record| {
                if capture_id == keep_capture_id {
                    return true;
                }
                let keep = record.completed_at.is_none_or(|completed_at| {
                    now.duration_since(completed_at) <= BACKGROUND_COMMAND_OUTPUT_COMPLETED_TTL
                });
                if !keep {
                    if let Some(exec_session_id) = record.exec_session_id {
                        removed_keys.push(BackgroundCommandSessionKey {
                            remote: record.remote,
                            exec_session_id,
                        });
                    }
                }
                keep
            });
        }

        if !removed_keys.is_empty() {
            let mut session_index = self.session_index.lock().await;
            for key in removed_keys {
                session_index.remove(&key);
            }
        }
    }
}

impl BackgroundCommandOutputRecord {
    fn snapshot(&self) -> String {
        self.chunks
            .iter()
            .map(|chunk| chunk.text.as_str())
            .collect::<String>()
    }

    fn snapshot_stream(&self, stream: ExecOutputStream) -> String {
        self.chunks
            .iter()
            .filter(|chunk| chunk.stream == stream)
            .map(|chunk| chunk.text.as_str())
            .collect::<String>()
    }

    fn metadata(&self) -> BackgroundCommandOutputMetadata {
        BackgroundCommandOutputMetadata {
            agent_session_id: self.agent_session_id.clone(),
            exec_session_id: self.exec_session_id,
            command: self.command.clone(),
            workdir: self.workdir.clone(),
            remote: self.remote,
            tty: self.tty,
            terminal_size: self.terminal_size,
            status: self.status,
            exit_code: self.exit_code,
            started_at: self.started_at,
            ended_at: self.ended_at,
            retained_bytes: self.retained_bytes,
            retained_limit_bytes: BACKGROUND_COMMAND_OUTPUT_CAPTURE_LIMIT_BYTES,
            truncated_from_start: self.truncated_from_start,
        }
    }
}

fn finish_record(
    record: &mut BackgroundCommandOutputRecord,
    status: BackgroundCommandOutputStatus,
    exit_code: Option<i32>,
) {
    record.status = status;
    record.exit_code = exit_code;
    if record.ended_at.is_none() {
        record.ended_at = Some(now_unix_seconds());
    }
    if record.completed_at.is_none() {
        record.completed_at = Some(Instant::now());
    }
}

/// Shared read protocol for session-keyed and capture-id-keyed reads:
/// first read (or an aged-out cursor) returns a full snapshot, later reads
/// return only the chunks after `cursor`. `stream` restricts both views to
/// one tagged stream; `None`/`Combined` keep the arrival-ordered view.
fn build_read_response(
    record: &BackgroundCommandOutputRecord,
    cursor: Option<u64>,
    stream: Option<ExecOutputStream>,
) -> ReadBackgroundCommandOutputResponse {
    let stream_filter = match stream {
        Some(ExecOutputStream::Stdout) | Some(ExecOutputStream::Stderr) => stream,
        Some(ExecOutputStream::Combined) | None => None,
    };
    let matches_stream = |chunk: &BackgroundCommandOutputChunk| match stream_filter {
        Some(filter) => chunk.stream == filter,
        None => true,
    };

    let first_cursor = record.chunks.front().map(|chunk| chunk.cursor);
    let should_reset = cursor
        .zip(first_cursor)
        .is_some_and(|(cursor, first)| cursor < first);
    let snapshot = if cursor.is_none() || should_reset {
        Some(match stream_filter {
            Some(filter) => record.snapshot_stream(filter),
            None => record.snapshot(),
        })
    } else {
        None
    };
    let chunks = if let Some(cursor) = cursor.filter(|_| !should_reset) {
        record
            .chunks
            .iter()
            .filter(|chunk| chunk.cursor >= cursor && matches_stream(chunk))
            .map(|chunk| chunk.text.clone())
            .collect()
    } else {
        Vec::new()
    };

    ReadBackgroundCommandOutputResponse {
        metadata: record.metadata(),
        cursor: record.next_cursor,
        reset: should_reset,
        snapshot,
        chunks,
    }
}

fn prune_record_count(records: &mut HashMap<String, BackgroundCommandOutputRecord>) {
    while records.len() > MAX_BACKGROUND_COMMAND_OUTPUT_RECORDS {
        let Some(oldest_capture_id) = records
            .iter()
            .min_by_key(|(_, record)| (record.completed_at.is_none(), record.started_at))
            .map(|(capture_id, _)| capture_id.clone())
        else {
            break;
        };
        records.remove(&oldest_capture_id);
    }
}

fn now_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::{
        background_command_output_capture, BackgroundCommandOutputStatus,
        ReadBackgroundCommandOutputRefRequest, ReadBackgroundCommandOutputRequest,
        StartBackgroundCommandOutputCapture,
    };
    use openbitfun_runtime_ports::{ExecOutputChunk, ExecOutputStream};

    #[test]
    fn background_output_metadata_reads_legacy_payload_and_round_trips_geometry() {
        let legacy = serde_json::json!({
            "agentSessionId": null, "execSessionId": 42, "command": "test",
            "workdir": null, "remote": true, "tty": true, "status": "running",
            "exitCode": null, "startedAt": 0, "endedAt": null,
            "retainedBytes": 0, "retainedLimitBytes": 1048576, "truncatedFromStart": false
        });
        let mut metadata: super::BackgroundCommandOutputMetadata =
            serde_json::from_value(legacy.clone()).unwrap();
        assert_eq!(metadata.terminal_size, None);
        let old_round_trip = serde_json::to_value(&metadata).unwrap();
        for (key, value) in legacy.as_object().unwrap() {
            assert_eq!(&old_round_trip[key], value);
        }
        // New hosts explicitly distinguish unknown geometry from old absent fields.
        assert!(old_round_trip["terminalSize"].is_null());
        metadata.terminal_size = Some(super::ExecTerminalSize { cols: 80, rows: 24 });
        let encoded = serde_json::to_value(&metadata).unwrap();
        assert_eq!(
            encoded["terminalSize"],
            serde_json::json!({"cols": 80, "rows": 24})
        );
        let restored: super::BackgroundCommandOutputMetadata =
            serde_json::from_value(encoded).unwrap();
        assert_eq!(restored.terminal_size, metadata.terminal_size);
    }

    #[tokio::test]
    async fn background_command_output_reads_snapshot_then_incremental_chunks() {
        let capture_id = format!(
            "test-capture-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock should be after unix epoch")
                .as_nanos()
        );
        let capture = background_command_output_capture();
        let tx = capture
            .start_capture(StartBackgroundCommandOutputCapture {
                capture_id: capture_id.clone(),
                agent_session_id: Some("agent-session-1".to_string()),
                command: "echo hi".to_string(),
                workdir: None,
                remote: false,
                tty: false,
                terminal_size: None,
            })
            .await;

        capture
            .update_lifecycle(
                &capture_id,
                4242,
                BackgroundCommandOutputStatus::Running,
                None,
            )
            .await
            .expect("record exists");
        tx.send(ExecOutputChunk::combined("hello"))
            .expect("capture receiver alive");
        tx.send(ExecOutputChunk::combined(" world"))
            .expect("capture receiver alive");
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        let first = capture
            .read(ReadBackgroundCommandOutputRequest {
                exec_session_id: 4242,
                remote: false,
                cursor: None,
            })
            .await
            .expect("record exists");
        assert_eq!(first.snapshot.as_deref(), Some("hello world"));
        assert_eq!(first.cursor, 2);

        tx.send(ExecOutputChunk::combined("!"))
            .expect("capture receiver alive");
        capture
            .finish_by_session(false, 4242, BackgroundCommandOutputStatus::Exited, Some(0))
            .await
            .expect("record exists");
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        let second = capture
            .read(ReadBackgroundCommandOutputRequest {
                exec_session_id: 4242,
                remote: false,
                cursor: Some(first.cursor),
            })
            .await
            .expect("record exists");
        assert_eq!(second.snapshot, None);
        assert_eq!(second.chunks, vec!["!"]);
        assert_eq!(
            second.metadata.status,
            BackgroundCommandOutputStatus::Exited
        );
    }

    #[tokio::test]
    async fn read_by_ref_filters_snapshot_and_chunks_by_stream() {
        let capture_id = format!(
            "ref-capture-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock should be after unix epoch")
                .as_nanos()
        );
        let capture = background_command_output_capture();
        let tx = capture
            .start_capture(StartBackgroundCommandOutputCapture {
                capture_id: capture_id.clone(),
                agent_session_id: None,
                command: "build".to_string(),
                workdir: None,
                remote: false,
                tty: false,
                terminal_size: None,
            })
            .await;
        tx.send(ExecOutputChunk {
            stream: ExecOutputStream::Stdout,
            text: "out-1 ".to_string(),
        })
        .expect("capture receiver alive");
        tx.send(ExecOutputChunk {
            stream: ExecOutputStream::Stderr,
            text: "err-1 ".to_string(),
        })
        .expect("capture receiver alive");
        tx.send(ExecOutputChunk {
            stream: ExecOutputStream::Stdout,
            text: "out-2".to_string(),
        })
        .expect("capture receiver alive");
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        let stderr_snapshot = capture
            .read_by_ref(ReadBackgroundCommandOutputRefRequest {
                capture_id: capture_id.clone(),
                cursor: None,
                stream: Some(ExecOutputStream::Stderr),
            })
            .await
            .expect("record exists");
        assert_eq!(stderr_snapshot.snapshot.as_deref(), Some("err-1 "));
        assert_eq!(stderr_snapshot.cursor, 3);

        let stdout_incremental = capture
            .read_by_ref(ReadBackgroundCommandOutputRefRequest {
                capture_id,
                cursor: Some(2),
                stream: Some(ExecOutputStream::Stdout),
            })
            .await
            .expect("record exists");
        assert_eq!(stdout_incremental.snapshot, None);
        assert_eq!(stdout_incremental.chunks, vec!["out-2"]);
    }

    #[tokio::test]
    async fn metadata_by_ref_answers_missing_captures_with_none() {
        let capture = background_command_output_capture();

        assert!(capture
            .metadata_by_ref("no-such-capture-id")
            .await
            .is_none());
    }
}
