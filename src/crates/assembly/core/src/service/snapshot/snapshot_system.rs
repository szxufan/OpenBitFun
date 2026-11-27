use super::storage::{
    descriptor, limit_error, read_bounded, write_atomic, CONTENT_JOBS, CONTENT_LIMIT,
};
use crate::service::snapshot::isolation_manager::IsolationManager;
use crate::service::snapshot::types::{
    FileMetadata, FileSnapshot, SnapshotError, SnapshotResult, SnapshotType, StorageStats,
};
use crate::service::workspace_runtime::{WorkspaceRuntimeContext, WorkspaceRuntimeTarget};
use log::{debug, error, info, warn};
use openbitfun_runtime_ports::{WorkspaceFileSystem, WorkspaceMetadata, WorkspacePathKind};
use openbitfun_services_core::workspace::LocalWorkspaceFs;
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Instant, SystemTime};
use tokio::sync::RwLock;
use uuid::Uuid;

/// Baseline snapshot cache
pub struct BaselineCache {
    /// In-memory cache: exact path spelling -> snapshot_id. Raw OS strings
    /// preserve POSIX backslashes on a Windows controller; Path equality does not.
    /// `None` indicates it has been queried but does not exist.
    cache: Arc<RwLock<HashMap<OsString, Option<String>>>>,

    /// Baseline metadata directory
    baseline_dir: PathBuf,
}

impl BaselineCache {
    /// Creates a new baseline cache.
    pub fn new(baseline_dir: PathBuf) -> Self {
        debug!(
            "BaselineCache initialized: directory={}",
            baseline_dir.display()
        );

        Self {
            cache: Arc::new(RwLock::new(HashMap::new())),
            baseline_dir,
        }
    }

    /// Gets the baseline snapshot ID for a file.
    ///
    /// Strategy: check the in-memory map first; if missing, check the directory, then cache the result.
    pub async fn get(&self, file_path: &Path) -> Option<String> {
        {
            let cache = self.cache.read().await;
            if let Some(cached) = cache.get(file_path.as_os_str()) {
                debug!("Baseline cache hit: file_path={:?}", file_path);
                return cached.clone();
            }
        }

        debug!("Querying baseline directory: file_path={:?}", file_path);
        if let Some(snapshot_id) = self.query_directory(file_path).await {
            debug!(
                "Found baseline snapshot: file_path={:?} snapshot_id={}",
                file_path, snapshot_id
            );

            {
                let mut cache = self.cache.write().await;
                cache.insert(
                    file_path.as_os_str().to_os_string(),
                    Some(snapshot_id.clone()),
                );
            }

            return Some(snapshot_id);
        }

        debug!("Baseline snapshot not found: file_path={:?}", file_path);
        {
            let mut cache = self.cache.write().await;
            cache.insert(file_path.as_os_str().to_os_string(), None);
        }
        None
    }

    /// Queries baseline snapshots from the directory.
    async fn query_directory(&self, file_path: &Path) -> Option<String> {
        let entries = fs::read_dir(&self.baseline_dir).ok()?;

        let mut found_snapshots: Vec<(SystemTime, String)> = Vec::new();

        for entry in entries.flatten() {
            let path = entry.path();

            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }

            let metadata = match descriptor(&path) {
                Ok(metadata) => metadata,
                Err(error) => {
                    warn!(
                        "Skipping unavailable baseline {}: {}",
                        path.display(),
                        error
                    );
                    continue;
                }
            };

            if metadata.snapshot_type == SnapshotType::Baseline
                && metadata.file_path.as_os_str() == file_path.as_os_str()
            {
                found_snapshots.push((metadata.timestamp, metadata.snapshot_id));
            }
        }

        if found_snapshots.is_empty() {
            return None;
        }

        found_snapshots.sort_by_key(|snapshot| std::cmp::Reverse(snapshot.0));
        let (timestamp, snapshot_id) = &found_snapshots[0];

        debug!(
            "Found {} baseline snapshots, using latest: snapshot_id={} timestamp={:?}",
            found_snapshots.len(),
            snapshot_id,
            timestamp
        );

        Some(snapshot_id.clone())
    }

    /// Creates a baseline from a "before" snapshot.
    pub async fn create_from_snapshot(
        &self,
        file_path: &Path,
        before_snapshot_id: &str,
        active_snapshots: &HashMap<String, FileSnapshot>,
    ) -> SnapshotResult<String> {
        debug!(
            "Creating baseline snapshot: file_path={:?} before_snapshot_id={}",
            file_path, before_snapshot_id
        );

        let before_snapshot = active_snapshots
            .get(before_snapshot_id)
            .ok_or_else(|| SnapshotError::SnapshotNotFound(before_snapshot_id.to_string()))?;

        let baseline_id = format!("baseline_{}", Uuid::new_v4());

        let baseline_metadata = FileSnapshot {
            snapshot_id: baseline_id.clone(),
            file_path: file_path.to_path_buf(),
            content_hash: before_snapshot.content_hash.clone(),
            snapshot_type: SnapshotType::Baseline,
            compressed_content: Vec::new(),
            timestamp: SystemTime::now(),
            metadata: before_snapshot.metadata.clone(),
        };

        let baseline_meta_path = self.baseline_dir.join(format!("{}.json", baseline_id));
        let metadata_json = serde_json::to_string_pretty(&baseline_metadata)?;
        write_atomic(&baseline_meta_path, metadata_json.as_bytes())?;

        debug!(
            "Created baseline snapshot: file_path={:?} baseline_id={} metadata_path={}",
            file_path,
            baseline_id,
            baseline_meta_path.display()
        );

        {
            let mut cache = self.cache.write().await;
            cache.insert(
                file_path.as_os_str().to_os_string(),
                Some(baseline_id.clone()),
            );
        }

        Ok(baseline_id)
    }

    /// Creates an empty baseline for files that are first introduced during the session.
    pub async fn create_empty(
        &self,
        file_path: &Path,
        empty_content_hash: &str,
        content_path: &Path,
    ) -> SnapshotResult<String> {
        let baseline_id = format!("baseline_empty_{}", Uuid::new_v4());

        if !content_path.exists() {
            fs::write(content_path, [])?;
        }

        let baseline_metadata = FileSnapshot {
            snapshot_id: baseline_id.clone(),
            file_path: file_path.to_path_buf(),
            content_hash: empty_content_hash.to_string(),
            snapshot_type: SnapshotType::Baseline,
            compressed_content: Vec::new(),
            timestamp: SystemTime::now(),
            metadata: FileMetadata {
                size: 0,
                permissions: None,
                last_modified: SystemTime::now(),
                encoding: "utf-8".to_string(),
            },
        };

        let baseline_meta_path = self.baseline_dir.join(format!("{}.json", baseline_id));
        let metadata_json = serde_json::to_string_pretty(&baseline_metadata)?;
        write_atomic(&baseline_meta_path, metadata_json.as_bytes())?;

        {
            let mut cache = self.cache.write().await;
            cache.insert(
                file_path.as_os_str().to_os_string(),
                Some(baseline_id.clone()),
            );
        }

        Ok(baseline_id)
    }
}

/// Simplified file snapshot system
///
/// Only stores snapshots of file content; does not manage a change queue.
pub struct FileSnapshotSystem {
    workspace_fs: Option<Arc<dyn WorkspaceFileSystem>>,
    remote_isolation: Option<IsolationManager>,
    snapshot_dir: PathBuf,
    snapshot_by_hash_dir: PathBuf,
    snapshot_metadata_dir: PathBuf,
    hash_to_path: HashMap<String, PathBuf>,
    active_snapshots: HashMap<String, FileSnapshot>,
    capture_limit: usize,
    compression_enabled: bool,
    dedup_enabled: bool,
    baseline_cache: BaselineCache,
}

impl FileSnapshotSystem {
    /// Creates a new file snapshot system.
    pub fn new(runtime_context: WorkspaceRuntimeContext) -> Self {
        let workspace_fs = matches!(
            runtime_context.target,
            WorkspaceRuntimeTarget::LocalWorkspace { .. }
        )
        .then(|| Arc::new(LocalWorkspaceFs) as Arc<dyn WorkspaceFileSystem>);
        Self::build(runtime_context, workspace_fs)
    }

    pub fn with_workspace_fs(
        runtime_context: WorkspaceRuntimeContext,
        workspace_fs: Arc<dyn WorkspaceFileSystem>,
    ) -> Self {
        Self::build(runtime_context, Some(workspace_fs))
    }

    fn build(
        runtime_context: WorkspaceRuntimeContext,
        workspace_fs: Option<Arc<dyn WorkspaceFileSystem>>,
    ) -> Self {
        let snapshot_dir = runtime_context.snapshots_dir.clone();
        let remote_isolation = match &runtime_context.target {
            WorkspaceRuntimeTarget::RemoteWorkspaceMirror { remote_root, .. } => Some(
                IsolationManager::new(PathBuf::from(remote_root), runtime_context.clone()),
            ),
            WorkspaceRuntimeTarget::LocalWorkspace { .. } => None,
        };

        Self {
            workspace_fs,
            remote_isolation,
            snapshot_by_hash_dir: runtime_context.snapshot_by_hash_dir.clone(),
            snapshot_metadata_dir: runtime_context.snapshot_metadata_dir.clone(),
            snapshot_dir,
            hash_to_path: HashMap::new(),
            active_snapshots: HashMap::new(),
            capture_limit: super::types::SnapshotConfig::default()
                .max_snapshot_size_mb
                .saturating_mul(1024 * 1024),
            compression_enabled: true,
            dedup_enabled: true,
            baseline_cache: BaselineCache::new(runtime_context.snapshot_baselines_dir.clone()),
        }
    }

    pub(super) fn set_capture_budget_mb(&mut self, mb: usize) {
        self.capture_limit = mb.saturating_mul(1024 * 1024);
    }

    pub(crate) fn workspace_fs(&self) -> SnapshotResult<&dyn WorkspaceFileSystem> {
        self.workspace_fs.as_deref().ok_or_else(|| {
            SnapshotError::ConfigError(
                "Remote snapshot workspace filesystem was not explicitly bound".into(),
            )
        })
    }

    async fn validate_workspace_path(&self, path: &Path) -> SnapshotResult<()> {
        if let Some(isolation) = &self.remote_isolation {
            isolation
                .validate_workspace_path(path, self.workspace_fs()?)
                .await?;
        }
        Ok(())
    }

    pub(crate) async fn workspace_metadata(
        &self,
        path: &Path,
    ) -> SnapshotResult<Option<WorkspaceMetadata>> {
        self.validate_workspace_path(path).await?;
        Ok(self
            .workspace_fs()?
            .metadata(path.to_string_lossy().as_ref(), true)
            .await?)
    }

    pub(crate) async fn workspace_exists(&self, path: &Path) -> SnapshotResult<bool> {
        Ok(self.workspace_metadata(path).await?.is_some())
    }

    pub(crate) async fn read_workspace_text(&self, path: &Path) -> SnapshotResult<String> {
        self.validate_workspace_path(path).await?;
        let _permit = CONTENT_JOBS
            .acquire()
            .await
            .expect("snapshot semaphore stays open");
        let bytes = self
            .workspace_fs()?
            .read_file_bounded(path.to_string_lossy().as_ref(), CONTENT_LIMIT)
            .await?
            .ok_or_else(|| limit_error(path, CONTENT_LIMIT))?;
        String::from_utf8(bytes).map_err(|error| {
            SnapshotError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, error))
        })
    }

    pub(crate) async fn remove_workspace_file(&self, path: &Path) -> SnapshotResult<()> {
        self.validate_workspace_path(path).await?;
        Ok(self
            .workspace_fs()?
            .remove_file(path.to_string_lossy().as_ref())
            .await?)
    }

    /// Initializes the snapshot system.
    pub async fn initialize(&mut self) -> SnapshotResult<()> {
        let total_started_at = Instant::now();
        info!("Initializing file snapshot system");

        let directories_started_at = Instant::now();
        self.ensure_directories().await?;
        debug!(
            "File snapshot initialize step completed: step=ensure_directories duration_ms={}",
            directories_started_at.elapsed().as_millis()
        );

        let index_started_at = Instant::now();
        self.load_snapshot_index().await?;
        debug!(
            "File snapshot initialize step completed: step=load_snapshot_index duration_ms={}",
            index_started_at.elapsed().as_millis()
        );

        info!(
            "File snapshot system initialized: loaded_snapshots={} duration_ms={}",
            self.active_snapshots.len(),
            total_started_at.elapsed().as_millis()
        );
        Ok(())
    }

    /// Ensures required directories exist.
    async fn ensure_directories(&self) -> SnapshotResult<()> {
        let directories = [
            &self.snapshot_dir,
            &self.snapshot_by_hash_dir,
            &self.snapshot_metadata_dir,
            &self.baseline_cache.baseline_dir,
        ];

        for dir in &directories {
            if !dir.exists() {
                return Err(SnapshotError::ConfigError(format!(
                    "Snapshot runtime directory is missing: {}",
                    dir.display()
                )));
            }
        }

        Ok(())
    }

    /// Loads the existing snapshot index.
    ///
    /// Workspaces accumulate thousands of small metadata files, and this load
    /// gates the first snapshot view of a workspace. Reading them sequentially
    /// on one thread costs seconds, so the files are read and parsed on a
    /// bounded set of blocking threads and merged on the async side.
    async fn load_snapshot_index(&mut self) -> SnapshotResult<()> {
        let started_at = Instant::now();
        let metadata_dir = self.snapshot_metadata_dir.clone();

        if !metadata_dir.exists() {
            return Ok(());
        }

        let mut metadata_paths = Vec::new();
        for entry in fs::read_dir(&metadata_dir)? {
            let path = entry?.path();
            if path.extension().and_then(|s| s.to_str()) == Some("json") {
                metadata_paths.push(path);
            }
        }

        const MAX_LOAD_THREADS: usize = 8;
        const MIN_FILES_PER_THREAD: usize = 64;
        let thread_count = (metadata_paths.len() / MIN_FILES_PER_THREAD).clamp(1, MAX_LOAD_THREADS);
        let chunk_size = metadata_paths.len().div_ceil(thread_count).max(1);

        let mut load_tasks = Vec::with_capacity(thread_count);
        for chunk in metadata_paths.chunks(chunk_size) {
            let chunk = chunk.to_vec();
            load_tasks.push(tokio::task::spawn_blocking(move || {
                let mut snapshots = Vec::with_capacity(chunk.len());
                for path in chunk {
                    let parsed = descriptor(&path);
                    match parsed {
                        Ok(snapshot) => snapshots.push(snapshot),
                        Err(e) => {
                            warn!(
                                "Failed to load snapshot metadata: path={} error={}",
                                path.display(),
                                e
                            );
                        }
                    }
                }
                snapshots
            }));
        }

        let mut loaded_count = 0;
        for task in load_tasks {
            let snapshots = task.await.map_err(|e| {
                SnapshotError::ConfigError(format!("Snapshot metadata load task failed: {e}"))
            })?;
            for snapshot in snapshots {
                self.hash_to_path.insert(
                    snapshot.content_hash.clone(),
                    self.get_content_path(&snapshot.content_hash),
                );
                self.active_snapshots
                    .insert(snapshot.snapshot_id.clone(), snapshot);
                loaded_count += 1;
            }
        }

        debug!(
            "Loaded snapshot metadata files: count={} duration_ms={}",
            loaded_count,
            started_at.elapsed().as_millis()
        );
        Ok(())
    }

    /// Loads snapshot metadata.
    async fn load_snapshot_metadata(&self, path: &Path) -> SnapshotResult<FileSnapshot> {
        descriptor(path)
    }

    /// Creates a file snapshot.
    pub async fn create_snapshot(&mut self, file_path: &Path) -> SnapshotResult<String> {
        self.create_snapshot_with_ownership(file_path, false).await
    }

    /// Creates a snapshot with an independent metadata handle while retaining
    /// content-addressed blob deduplication. The caller owns this handle and may
    /// delete it without invalidating another operation's snapshot reference.
    pub(crate) async fn create_owned_snapshot(
        &mut self,
        file_path: &Path,
    ) -> SnapshotResult<String> {
        self.create_snapshot_with_ownership(file_path, true).await
    }

    async fn create_snapshot_with_ownership(
        &mut self,
        file_path: &Path,
        independent_handle: bool,
    ) -> SnapshotResult<String> {
        debug!("Creating snapshot: file_path={}", file_path.display());

        let Some(workspace_metadata) = self.workspace_metadata(file_path).await? else {
            error!(
                "File not found for snapshot: file_path={}",
                file_path.display()
            );
            return Err(SnapshotError::FileNotFound(file_path.to_path_buf()));
        };
        if workspace_metadata.kind != WorkspacePathKind::File {
            return Err(SnapshotError::ConfigError(format!(
                "Snapshot target is not a regular file: {}",
                file_path.display()
            )));
        }
        let size = workspace_metadata.size.unwrap_or(0);
        if size > self.capture_limit as u64 {
            return Err(limit_error(file_path, self.capture_limit));
        }
        let _permit = CONTENT_JOBS
            .acquire()
            .await
            .expect("snapshot semaphore stays open");
        let (content_hash, metadata) = if size > CONTENT_LIMIT as u64 {
            let (hash, recorded_size) = super::storage::stream_blob(
                self.workspace_fs()?,
                file_path,
                &self.snapshot_by_hash_dir,
                self.capture_limit,
            )
            .await?;
            (
                hash,
                Self::file_metadata(&workspace_metadata, recorded_size, "unknown")?,
            )
        } else {
            let limit = self.capture_limit.min(CONTENT_LIMIT);
            let content = self
                .workspace_fs()?
                .read_file_bounded(file_path.to_string_lossy().as_ref(), limit)
                .await?
                .ok_or_else(|| limit_error(file_path, limit))?;
            let metadata = Self::extract_file_metadata(&workspace_metadata, &content)?;
            let content_hash = self.calculate_content_hash(&content);
            let blob_path = self.get_content_path(&content_hash);
            if !blob_path.exists() {
                let gzip_input = content.starts_with(&[0x1f, 0x8b]);
                let compressed = if gzip_input || (self.compression_enabled && content.len() > 1024)
                {
                    Some(self.compress_content(&content)?)
                } else {
                    None
                };
                let stored = compressed
                    .as_deref()
                    .filter(|data| gzip_input || data.len() < content.len() * 4 / 5)
                    .unwrap_or(&content);
                write_atomic(&blob_path, stored)?;
            }
            (content_hash, metadata)
        };

        if !independent_handle
            && self.dedup_enabled
            && self.hash_to_path.contains_key(&content_hash)
        {
            if let Some(snapshot_id) = self.find_snapshot_by_hash(&content_hash) {
                debug!(
                    "Found duplicate content, reusing existing snapshot: content_hash={}",
                    content_hash
                );
                return Ok(snapshot_id);
            }

            debug!(
                "Found reusable content without active snapshot metadata, creating new snapshot metadata: content_hash={}",
                content_hash
            );
        }

        let snapshot = FileSnapshot {
            snapshot_id: Uuid::new_v4().to_string(),
            file_path: file_path.to_path_buf(),
            content_hash: content_hash.clone(),
            snapshot_type: SnapshotType::Before,
            compressed_content: Vec::new(),
            timestamp: SystemTime::now(),
            metadata,
        };

        self.store_snapshot(&snapshot).await?;

        self.hash_to_path
            .insert(content_hash, self.get_content_path(&snapshot.content_hash));
        let snapshot_id = snapshot.snapshot_id.clone();
        self.active_snapshots.insert(snapshot_id.clone(), snapshot);

        debug!("Snapshot created successfully: snapshot_id={}", snapshot_id);
        Ok(snapshot_id)
    }

    /// Extracts file metadata.
    fn extract_file_metadata(
        metadata: &WorkspaceMetadata,
        content: &[u8],
    ) -> SnapshotResult<FileMetadata> {
        let encoding = if content.is_ascii() {
            "ascii"
        } else if std::str::from_utf8(content).is_ok() {
            "utf-8"
        } else {
            "binary"
        };
        Self::file_metadata(metadata, content.len() as u64, encoding)
    }

    fn file_metadata(
        metadata: &WorkspaceMetadata,
        size: u64,
        encoding: &str,
    ) -> SnapshotResult<FileMetadata> {
        Ok(FileMetadata {
            size,
            permissions: metadata.permissions,
            last_modified: metadata.modified.ok_or_else(|| {
                SnapshotError::ConfigError(
                    "Workspace provider did not supply a modification time for the snapshot".into(),
                )
            })?,
            encoding: encoding.into(),
        })
    }

    /// Computes content hash.
    fn calculate_content_hash(&self, content: &[u8]) -> String {
        format!("{:x}", md5::compute(content))
    }

    /// Compresses content.
    fn compress_content(&self, content: &[u8]) -> Result<Vec<u8>, std::io::Error> {
        use flate2::write::GzEncoder;
        use flate2::Compression;
        use std::io::Write;

        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(content)?;
        encoder.finish()
    }

    async fn store_snapshot(&self, snapshot: &FileSnapshot) -> SnapshotResult<()> {
        write_atomic(
            &self.get_metadata_path(&snapshot.snapshot_id),
            &serde_json::to_vec_pretty(snapshot)?,
        )
    }

    /// Returns the content file path.
    fn get_content_path(&self, content_hash: &str) -> PathBuf {
        self.snapshot_by_hash_dir
            .join(format!("{}.snap", content_hash))
    }

    /// Returns the metadata file path.
    fn get_metadata_path(&self, snapshot_id: &str) -> PathBuf {
        if snapshot_id.starts_with("baseline_") {
            self.baseline_cache
                .baseline_dir
                .join(format!("{}.json", snapshot_id))
        } else {
            self.snapshot_metadata_dir
                .join(format!("{}.json", snapshot_id))
        }
    }

    /// Finds a snapshot ID by hash.
    fn find_snapshot_by_hash(&self, content_hash: &str) -> Option<String> {
        for (snapshot_id, snapshot) in &self.active_snapshots {
            if snapshot.content_hash == content_hash {
                return Some(snapshot_id.clone());
            }
        }
        None
    }

    /// Recorded logical size (bytes) from snapshot metadata, without loading file contents.
    pub async fn get_snapshot_recorded_size_bytes(&self, snapshot_id: &str) -> SnapshotResult<u64> {
        let snapshot = self.load_snapshot_from_disk(snapshot_id).await?;
        Ok(snapshot.metadata.size)
    }

    /// Loads snapshot metadata from disk (without using in-memory cache).
    async fn load_snapshot_from_disk(&self, snapshot_id: &str) -> SnapshotResult<FileSnapshot> {
        debug!(
            "Loading snapshot metadata from disk: snapshot_id={}",
            snapshot_id
        );
        let metadata_path = self.get_metadata_path(snapshot_id);

        if !metadata_path.exists() {
            return Err(SnapshotError::SnapshotNotFound(snapshot_id.to_string()));
        }

        let snapshot = self.load_snapshot_metadata(&metadata_path).await?;
        debug!(
            "Snapshot metadata loaded successfully: snapshot_id={}",
            snapshot_id
        );
        Ok(snapshot)
    }

    /// Gets snapshot content (string), read directly from disk.
    pub async fn get_snapshot_content(&self, snapshot_id: &str) -> SnapshotResult<String> {
        let content_bytes = self.restore_snapshot_content(snapshot_id).await?;
        String::from_utf8(content_bytes).map_err(|e| {
            SnapshotError::ConfigError(format!("Snapshot content is not valid UTF-8: {}", e))
        })
    }

    /// Restores snapshot content (read directly from disk, without using in-memory cache).
    pub async fn restore_snapshot_content(&self, snapshot_id: &str) -> SnapshotResult<Vec<u8>> {
        let snapshot = self.load_snapshot_from_disk(snapshot_id).await?;

        let path = self.get_content_path(&snapshot.content_hash);
        if snapshot.metadata.size > CONTENT_LIMIT as u64 {
            return Err(limit_error(&snapshot.file_path, CONTENT_LIMIT));
        }
        let _permit = CONTENT_JOBS
            .acquire()
            .await
            .expect("snapshot semaphore stays open");
        let stored = match fs::File::open(&path) {
            Ok(file) => read_bounded(file, CONTENT_LIMIT + 64 * 1024, &path)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                // Legacy records may have the only remaining copy inline.
                let metadata_path = self.get_metadata_path(snapshot_id);
                let file = fs::File::open(&metadata_path)?;
                let json_limit = CONTENT_LIMIT * 9 + 65536;
                if file.metadata()?.len() > json_limit as u64 {
                    return Err(limit_error(&metadata_path, json_limit));
                }
                let record: FileSnapshot = serde_json::from_reader(std::io::BufReader::new(
                    std::io::Read::take(file, json_limit as u64 + 1),
                ))?;
                if record.compressed_content.is_empty() {
                    return Err(SnapshotError::SnapshotNotFound(path.display().to_string()));
                }
                record.compressed_content
            }
            Err(error) => return Err(error.into()),
        };
        // Old blobs carry no codec tag. Check the raw hash first so an original
        // gzip file is restored byte-for-byte instead of being decoded twice.
        if stored.len() as u64 == snapshot.metadata.size
            && self.calculate_content_hash(&stored) == snapshot.content_hash
        {
            return Ok(stored);
        }
        let decoded = read_bounded(
            flate2::read::GzDecoder::new(stored.as_slice()),
            snapshot.metadata.size as usize,
            &path,
        )?;
        if decoded.len() as u64 != snapshot.metadata.size
            || self.calculate_content_hash(&decoded) != snapshot.content_hash
        {
            return Err(SnapshotError::ConfigError(format!(
                "Snapshot content failed verification: {}",
                snapshot_id
            )));
        }
        Ok(decoded)
    }

    /// Restores a file to the specified path (reads snapshot directly from disk).
    pub async fn restore_file(&self, snapshot_id: &str, target_path: &Path) -> SnapshotResult<()> {
        info!(
            "Restoring file from snapshot: snapshot_id={} target_path={}",
            snapshot_id,
            target_path.display()
        );

        self.validate_workspace_path(target_path).await?;
        let snapshot = self.load_snapshot_from_disk(snapshot_id).await?;
        let metadata = snapshot.metadata.clone();

        if snapshot.metadata.size > CONTENT_LIMIT as u64 {
            self.restore_large_blob(&snapshot, target_path).await?;
        } else {
            let content = self.restore_snapshot_content(snapshot_id).await?;
            self.workspace_fs()?
                .write_file(target_path.to_string_lossy().as_ref(), &content)
                .await?;
        }

        self.restore_file_metadata(target_path, &metadata).await?;

        info!(
            "File restored successfully: target_path={}",
            target_path.display()
        );
        Ok(())
    }

    /// Large blobs, including historical backups above the capture limit,
    /// Verify them while streaming to a sibling stage before replacing the target.
    async fn restore_large_blob(
        &self,
        snapshot: &FileSnapshot,
        target: &Path,
    ) -> SnapshotResult<()> {
        use std::io::Read;
        use tokio::io::AsyncWriteExt;
        let _permit = CONTENT_JOBS
            .acquire()
            .await
            .expect("snapshot semaphore stays open");
        use std::io::Seek;
        let expected = snapshot.metadata.size;
        let _stage;
        let mut blob = match fs::File::open(self.get_content_path(&snapshot.content_hash)) {
            Ok(file) => file,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                let path = self.get_metadata_path(&snapshot.snapshot_id);
                let directory = self.snapshot_by_hash_dir.clone();
                let (file, stage) = tokio::task::spawn_blocking(move || {
                    super::storage::extract_legacy_blob(&path, &directory, expected)
                })
                .await
                .map_err(|error| {
                    SnapshotError::ConfigError(format!(
                        "Legacy snapshot extraction failed: {error}"
                    ))
                })??;
                _stage = stage;
                file
            }
            Err(err) => return Err(err.into()),
        };
        let mut raw = blob.metadata()?.len() == expected;
        if raw {
            let mut magic = [0; 2];
            blob.read_exact(&mut magic)?;
            blob.rewind()?;
            if magic == [0x1f, 0x8b] {
                // A raw gzip input and a gzip blob of coincidentally identical
                // length share the same prefix. Old references have no codec tag.
                let mut digest = md5::Context::new();
                std::io::copy(&mut blob, &mut digest)?;
                raw = format!("{:x}", digest.compute()) == snapshot.content_hash;
                blob.rewind()?;
            }
        }
        let mut reader: Box<dyn Read + Send> = if raw {
            Box::new(blob)
        } else {
            Box::new(flate2::read::GzDecoder::new(blob))
        };
        let workspace = self.workspace_fs()?;
        let target = target.to_string_lossy();
        let parent = if self.remote_isolation.is_some() {
            target
                .rsplit_once('/')
                .map(|(dir, _)| if dir.is_empty() { "/" } else { dir })
        } else {
            Path::new(target.as_ref()).parent().and_then(Path::to_str)
        }
        .ok_or_else(|| {
            SnapshotError::ConfigError("Snapshot restore target has no parent".into())
        })?;
        workspace.create_dir_all(parent).await?;
        let stage = workspace.join_path(parent, &[&format!(".snapshot-{}.tmp", Uuid::new_v4())]);
        let writer = workspace.open_write_new(&stage).await?;
        let result: SnapshotResult<()> = async {
            let mut writer = writer;
            let mut buffer = vec![0; 64 * 1024];
            let mut digest = md5::Context::new();
            let mut count = 0u64;
            loop {
                let n = reader.read(&mut buffer)?;
                if n == 0 {
                    break;
                }
                count += n as u64;
                if count > expected {
                    return Err(SnapshotError::ConfigError(
                        "Snapshot exceeds recorded size".into(),
                    ));
                }
                digest.consume(&buffer[..n]);
                writer.write_all(&buffer[..n]).await?;
            }
            writer.shutdown().await?;
            drop(writer);
            if count != expected || format!("{:x}", digest.compute()) != snapshot.content_hash {
                return Err(SnapshotError::ConfigError(
                    "Snapshot content failed verification".into(),
                ));
            }
            workspace.atomic_replace(&stage, &target).await?;
            Ok(())
        }
        .await;
        if result.is_err() {
            let _ = workspace.remove_file(&stage).await;
        }
        result
    }

    /// Restores file metadata.
    async fn restore_file_metadata(
        &self,
        file_path: &Path,
        metadata: &FileMetadata,
    ) -> SnapshotResult<()> {
        let workspace_fs = self.workspace_fs()?;
        let path = file_path.to_string_lossy();
        if let Some(permissions) = metadata.permissions {
            workspace_fs.set_permissions(&path, permissions).await?;
        }
        workspace_fs
            .set_modified(&path, metadata.last_modified)
            .await?;
        Ok(())
    }

    // Baselines and legacy records outside the in-memory index may be the last
    // owner of a blob. If any descriptor is unreadable, keep all candidate blobs.
    fn persisted_hashes(&self) -> SnapshotResult<HashSet<String>> {
        let mut hashes = HashSet::new();
        for dir in [
            &self.snapshot_metadata_dir,
            &self.baseline_cache.baseline_dir,
        ] {
            for entry in fs::read_dir(dir)? {
                let path = entry?.path();
                if path.extension().is_some_and(|ext| ext == "json") {
                    hashes.insert(descriptor(&path)?.content_hash);
                }
            }
        }
        Ok(hashes)
    }

    /// Deletes a snapshot.
    pub async fn delete_snapshot(&mut self, snapshot_id: &str) -> SnapshotResult<()> {
        info!("Deleting snapshot: snapshot_id={}", snapshot_id);

        let metadata_path = self.get_metadata_path(snapshot_id);
        let snapshot = self
            .active_snapshots
            .get(snapshot_id)
            .ok_or_else(|| SnapshotError::SnapshotNotFound(snapshot_id.to_owned()))?;
        let hash = snapshot.content_hash.clone();
        fs::remove_file(metadata_path)?;
        self.active_snapshots.remove(snapshot_id);
        if !self
            .active_snapshots
            .values()
            .any(|s| s.content_hash == hash)
        {
            match self.persisted_hashes() {
                Ok(hashes) if !hashes.contains(&hash) => {
                    let path = self.get_content_path(&hash);
                    if path.exists() {
                        fs::remove_file(path)?;
                    }
                    self.hash_to_path.remove(&hash);
                }
                Err(error) => warn!(
                    "Retaining snapshot blob because references could not be verified: {}",
                    error
                ),
                _ => {}
            }
        }

        debug!("Snapshot deleted successfully: snapshot_id={}", snapshot_id);
        Ok(())
    }

    pub(super) fn forget_cleaned_files(&mut self, removed: &super::maintenance::Removed) {
        self.active_snapshots
            .retain(|id, _| !removed.handles.contains(id));
        self.hash_to_path
            .retain(|hash, _| !removed.blobs.contains(hash));
    }

    /// Returns storage statistics.
    pub async fn get_storage_stats(&self) -> SnapshotResult<StorageStats> {
        let mut total_size_bytes = 0;
        let mut compressed_size_bytes = 0;

        let total_snapshots = self.active_snapshots.len();

        let content_dir = self.snapshot_by_hash_dir.clone();
        if content_dir.exists() {
            for entry in fs::read_dir(&content_dir)? {
                let entry = entry?;
                if let Ok(metadata) = entry.metadata() {
                    compressed_size_bytes += metadata.len();
                }
            }
        }

        for snapshot in self.active_snapshots.values() {
            total_size_bytes += snapshot.metadata.size;
        }

        let compression_ratio = if total_size_bytes > 0 {
            compressed_size_bytes as f32 / total_size_bytes as f32
        } else {
            1.0
        };

        let dedup_savings_bytes = if self.dedup_enabled {
            let unique_hashes = self.hash_to_path.len() as u64;
            let total_hashes = total_snapshots as u64;
            if total_hashes > unique_hashes {
                (total_size_bytes * (total_hashes - unique_hashes)) / total_hashes
            } else {
                0
            }
        } else {
            0
        };

        Ok(StorageStats {
            total_snapshots,
            total_size_bytes,
            compressed_size_bytes,
            compression_ratio,
            dedup_savings_bytes,
        })
    }

    /// Cleans up orphaned snapshots.
    pub async fn cleanup_orphaned_snapshots(&mut self) -> SnapshotResult<usize> {
        info!("Cleaning up orphaned snapshots");

        let mut cleaned_count = 0;
        let content_dir = self.snapshot_by_hash_dir.clone();

        if !content_dir.exists() {
            return Ok(0);
        }

        let mut content_files = Vec::new();
        for entry in fs::read_dir(&content_dir)? {
            let entry = entry?;
            if entry.path().extension().and_then(|s| s.to_str()) == Some("snap") {
                content_files.push(entry.path());
            }
        }

        let referenced = self.persisted_hashes()?;
        for content_file in content_files {
            if let Some(file_stem) = content_file.file_stem().and_then(|s| s.to_str()) {
                let content_hash = file_stem;

                if !referenced.contains(content_hash) {
                    fs::remove_file(&content_file)?;
                    self.hash_to_path.remove(content_hash);
                    cleaned_count += 1;
                    debug!(
                        "Deleted orphaned content file: path={}",
                        content_file.display()
                    );
                }
            }
        }

        info!("Cleaned up {} orphaned snapshots", cleaned_count);
        Ok(cleaned_count)
    }

    /// Lists all snapshots.
    pub fn list_snapshots(&self) -> Vec<&FileSnapshot> {
        self.active_snapshots.values().collect()
    }

    /// Returns the snapshot count.
    pub fn snapshot_count(&self) -> usize {
        self.active_snapshots.len()
    }

    /// Gets the baseline snapshot ID for a file.
    ///
    /// Returns: Option<String> - `None` means this file has no baseline
    pub async fn get_baseline_snapshot_id(&self, file_path: &Path) -> Option<String> {
        self.baseline_cache.get(file_path).await
    }

    /// Creates a baseline snapshot.
    ///
    /// Creates a baseline from the specified "before" snapshot.
    /// If a baseline already exists, it will not be created again.
    ///
    /// # Parameters
    /// - file_path: File path
    /// - before_snapshot_id: before snapshot ID
    ///
    /// # Returns
    /// Baseline snapshot ID
    pub async fn create_baseline_from_snapshot(
        &self,
        file_path: &Path,
        before_snapshot_id: &str,
    ) -> SnapshotResult<String> {
        debug!(
            "Creating baseline snapshot: file_path={:?} before_snapshot_id={}",
            file_path, before_snapshot_id
        );

        if let Some(existing_id) = self.get_baseline_snapshot_id(file_path).await {
            debug!(
                "Baseline snapshot already exists: baseline_id={}",
                existing_id
            );
            return Ok(existing_id);
        }

        self.baseline_cache
            .create_from_snapshot(file_path, before_snapshot_id, &self.active_snapshots)
            .await
    }

    /// Creates an empty baseline for files that did not exist before the session.
    pub async fn create_empty_baseline(&mut self, file_path: &Path) -> SnapshotResult<String> {
        let empty_content_hash = self.calculate_content_hash(&[]);
        let content_path = self.get_content_path(&empty_content_hash);

        if !self.hash_to_path.contains_key(&empty_content_hash) {
            self.hash_to_path
                .insert(empty_content_hash.clone(), content_path.clone());
        }

        self.baseline_cache
            .create_empty(file_path, &empty_content_hash, &content_path)
            .await
    }

    /// Checks whether the file has a baseline.
    pub async fn has_baseline(&self, file_path: &Path) -> bool {
        self.get_baseline_snapshot_id(file_path).await.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::workspace_runtime::{WorkspaceRuntimeContext, WorkspaceRuntimeTarget};

    fn test_runtime_context() -> WorkspaceRuntimeContext {
        let runtime_root =
            std::env::temp_dir().join(format!("openbitfun_snapshot_test_{}", Uuid::new_v4()));
        WorkspaceRuntimeContext::new(
            WorkspaceRuntimeTarget::LocalWorkspace {
                workspace_root: runtime_root.join("workspace"),
            },
            runtime_root,
        )
    }

    fn create_runtime_dirs(context: &WorkspaceRuntimeContext) {
        for directory in context.required_directories() {
            fs::create_dir_all(directory).expect("create runtime directory");
        }
    }

    #[tokio::test]
    async fn create_snapshot_reuses_empty_baseline_content_without_panicking() {
        let context = test_runtime_context();
        create_runtime_dirs(&context);

        let file_path = context.runtime_root.join("workspace").join("empty.txt");
        fs::create_dir_all(file_path.parent().expect("file has parent")).expect("create parent");

        let mut snapshot_system = FileSnapshotSystem::new(context.clone());
        snapshot_system
            .initialize()
            .await
            .expect("initialize snapshots");
        snapshot_system
            .create_empty_baseline(&file_path)
            .await
            .expect("create empty baseline");

        fs::write(&file_path, []).expect("write empty file");

        let snapshot_id = snapshot_system
            .create_snapshot(&file_path)
            .await
            .expect("create snapshot");
        let restored = snapshot_system
            .restore_snapshot_content(&snapshot_id)
            .await
            .expect("restore snapshot content");

        assert!(restored.is_empty());

        fs::remove_dir_all(&context.runtime_root).expect("cleanup runtime root");
    }

    #[tokio::test]
    async fn owned_snapshot_handle_does_not_delete_an_existing_deduplicated_snapshot() {
        let context = test_runtime_context();
        create_runtime_dirs(&context);
        let file_path = context.runtime_root.join("workspace").join("shared.txt");
        fs::create_dir_all(file_path.parent().expect("file has parent")).expect("create parent");
        fs::write(&file_path, "same content").expect("write fixture");

        let mut snapshot_system = FileSnapshotSystem::new(context.clone());
        snapshot_system
            .initialize()
            .await
            .expect("initialize snapshots");
        let existing = snapshot_system
            .create_snapshot(&file_path)
            .await
            .expect("create existing snapshot");
        let owned = snapshot_system
            .create_owned_snapshot(&file_path)
            .await
            .expect("create independent checkpoint handle");

        assert_ne!(existing, owned);
        snapshot_system
            .delete_snapshot(&owned)
            .await
            .expect("delete owned handle");
        assert_eq!(
            snapshot_system
                .get_snapshot_content(&existing)
                .await
                .expect("existing snapshot must survive"),
            "same content"
        );

        fs::remove_dir_all(&context.runtime_root).expect("cleanup runtime root");
    }
    #[tokio::test]
    async fn new_metadata_and_baselines_reference_one_blob_after_cleanup_and_restart() {
        let context = test_runtime_context();
        create_runtime_dirs(&context);
        let path = context.runtime_root.join("example");
        let bytes = vec![0x92; 10000];
        fs::write(&path, &bytes).unwrap();
        let mut system = FileSnapshotSystem::new(context.clone());
        let before = system.create_owned_snapshot(&path).await.unwrap();
        let baseline = system
            .create_baseline_from_snapshot(&path, &before)
            .await
            .unwrap();
        for id in [&before, &baseline] {
            let json = fs::read(system.get_metadata_path(id)).unwrap();
            // The old reader requires the content field even for a reference.
            let legacy_reader: FileSnapshot = serde_json::from_slice(&json).unwrap();
            assert!(legacy_reader.compressed_content.is_empty());
            assert!(json.len() < 1024);
        }
        system.delete_snapshot(&before).await.unwrap();
        let mut cold = FileSnapshotSystem::new(context.clone());
        cold.initialize().await.unwrap();
        assert_eq!(cold.cleanup_orphaned_snapshots().await.unwrap(), 0);
        assert_eq!(
            cold.restore_snapshot_content(&baseline).await.unwrap(),
            bytes
        );
        fs::remove_dir_all(context.runtime_root).unwrap();
    }

    #[tokio::test]
    async fn legacy_inline_only_payload_is_loaded_on_demand_and_round_trips() {
        let context = test_runtime_context();
        create_runtime_dirs(&context);
        let path = context.runtime_root.join("binary");
        let bytes = vec![173; 65536];
        let record = FileSnapshot::new(
            path.clone(),
            bytes.clone(),
            FileMetadata {
                size: bytes.len() as u64,
                permissions: None,
                last_modified: SystemTime::now(),
                encoding: "binary".into(),
            },
            SnapshotType::Before,
        );
        let mut system = FileSnapshotSystem::new(context.clone());
        let json = serde_json::to_vec_pretty(&record).unwrap();
        fs::write(system.get_metadata_path(&record.snapshot_id), &json).unwrap();
        system.initialize().await.unwrap();
        assert!(system.list_snapshots()[0].compressed_content.is_empty());
        system
            .restore_file(&record.snapshot_id, &path)
            .await
            .unwrap();
        assert_eq!(fs::read(path).unwrap(), bytes);
        assert_eq!(
            fs::read(system.get_metadata_path(&record.snapshot_id)).unwrap(),
            json
        );
        fs::remove_dir_all(context.runtime_root).unwrap();
    }

    #[tokio::test]
    async fn streamed_legacy_restore_verifies_before_replacing_existing_target() {
        let context = test_runtime_context();
        create_runtime_dirs(&context);
        let path = context.runtime_root.join("target");
        fs::write(&path, b"keep").unwrap();
        let bytes = vec![127; CONTENT_LIMIT + 1];
        let mut record = FileSnapshot::new(
            path.clone(),
            bytes.clone(),
            FileMetadata {
                size: bytes.len() as u64,
                permissions: None,
                last_modified: SystemTime::now(),
                encoding: "binary".into(),
            },
            SnapshotType::Before,
        );
        record.compressed_content.clear();
        let system = FileSnapshotSystem::new(context.clone());
        system.store_snapshot(&record).await.unwrap();
        let blob = system.get_content_path(&record.content_hash);
        fs::write(&blob, system.compress_content(b"short").unwrap()).unwrap();
        assert!(system
            .restore_file(&record.snapshot_id, &path)
            .await
            .is_err());
        assert_eq!(fs::read(&path).unwrap(), b"keep");
        // Legacy baseline with an inline gzip payload and a collected blob.
        fs::remove_file(&blob).unwrap();
        record.compressed_content = system.compress_content(&bytes).unwrap();
        fs::write(
            system.get_metadata_path(&record.snapshot_id),
            serde_json::to_vec_pretty(&record).unwrap(),
        )
        .unwrap();
        system
            .restore_file(&record.snapshot_id, &path)
            .await
            .unwrap();
        assert_eq!(fs::read(path).unwrap(), bytes);
        fs::remove_dir_all(context.runtime_root).unwrap();
    }

    #[tokio::test]
    async fn oversized_source_is_rejected_before_a_snapshot_is_written() {
        let context = test_runtime_context();
        create_runtime_dirs(&context);
        let path = context.runtime_root.join("engine");
        fs::File::create(&path)
            .unwrap()
            .set_len(CONTENT_LIMIT as u64 + 1)
            .unwrap();
        let mut system = FileSnapshotSystem::new(context.clone());
        system.set_capture_budget_mb(8);
        assert!(system
            .create_owned_snapshot(&path)
            .await
            .unwrap_err()
            .to_string()
            .contains("byte limit"));
        assert_eq!(
            fs::read_dir(&system.snapshot_by_hash_dir).unwrap().count(),
            0
        );
        assert_eq!(
            fs::read_dir(&system.snapshot_metadata_dir).unwrap().count(),
            0
        );
        fs::remove_dir_all(context.runtime_root).unwrap();
    }
    #[tokio::test]
    async fn large_file_uses_a_streamed_blob_and_round_trips_after_restart() {
        let context = test_runtime_context();
        create_runtime_dirs(&context);
        let path = context.runtime_root.join("engine");
        let bytes = vec![42; CONTENT_LIMIT + 1];
        fs::write(&path, &bytes).unwrap();
        let mut system = FileSnapshotSystem::new(context.clone());
        let id = system.create_owned_snapshot(&path).await.unwrap();
        drop(system);
        fs::remove_file(&path).unwrap();
        let mut cold = FileSnapshotSystem::new(context.clone());
        cold.initialize().await.unwrap();
        assert_eq!(
            cold.get_snapshot_recorded_size_bytes(&id).await.unwrap(),
            bytes.len() as u64
        );
        assert!(cold.get_snapshot_content(&id).await.is_err());
        cold.restore_file(&id, &path).await.unwrap();
        assert_eq!(fs::read(path).unwrap(), bytes);
        fs::remove_dir_all(context.runtime_root).unwrap();
    }
}
