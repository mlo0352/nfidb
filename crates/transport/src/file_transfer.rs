use std::collections::{HashMap, VecDeque};
use std::fs::File as StdFile;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::body::Body;
use bytes::Bytes;
use futures_util::stream;
use nfidb_core::{Metrics, SessionManager};
use parking_lot::{Mutex, RwLock};
use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::fs::{self, OpenOptions};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio::sync::Mutex as AsyncMutex;
use uuid::Uuid;

pub const UPLOAD_CHUNK_SIZE: u64 = 1024 * 1024;
const DOWNLOAD_CHUNK_SIZE: usize = 64 * 1024;
const MAX_ACTIVE_UPLOADS: usize = 32;
const MAX_OUTBOX_FILES: usize = 1000;
const MAX_RECENT_TRANSFERS: usize = 100;
const MAX_COMPLETED_UPLOAD_TICKETS: usize = 100;
const RATE_WINDOW: Duration = Duration::from_secs(1);

#[derive(Debug, Clone)]
pub struct FileTransferOptions {
    pub enabled: bool,
    pub max_file_size_bytes: u64,
    pub rate_limit_mbps: u32,
    pub pause_while_drawing: bool,
    pub inbox_directory: PathBuf,
    pub staging_directory: PathBuf,
}

#[derive(Debug, Clone, Serialize)]
pub struct OutgoingFile {
    pub id: Uuid,
    pub name: String,
    pub size: u64,
    pub mime: String,
    pub queued_epoch_ms: u64,
    pub sha256: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ActiveUpload {
    pub id: Uuid,
    pub name: String,
    pub size: u64,
    pub received: u64,
    pub started_epoch_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum TransferDirection {
    IpadToWindows,
    WindowsToIpad,
}

#[derive(Debug, Clone, Serialize)]
pub struct CompletedTransfer {
    pub direction: TransferDirection,
    pub name: String,
    pub bytes: u64,
    pub duration_ms: u64,
    pub average_mbps: f64,
    pub sha256: Option<String>,
    pub completed_epoch_ms: u64,
    pub status: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct TransferStats {
    pub upload_bytes: u64,
    pub download_bytes: u64,
    pub uploads_completed: u64,
    pub downloads_completed: u64,
    pub canceled_transfers: u64,
    pub failed_transfers: u64,
    pub active_uploads: u32,
    pub active_downloads: u32,
    pub upload_mbps: f64,
    pub download_mbps: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct FileTransferSnapshot {
    pub enabled: bool,
    pub max_file_size_bytes: u64,
    pub chunk_size_bytes: u64,
    pub rate_limit_mbps: u32,
    pub pause_while_drawing: bool,
    #[serde(skip_serializing)]
    pub inbox_directory: PathBuf,
    pub outbox: Vec<OutgoingFile>,
    pub active_uploads: Vec<ActiveUpload>,
    pub recent: Vec<CompletedTransfer>,
    pub stats: TransferStats,
}

#[derive(Debug, Clone, Serialize)]
pub struct BrowserFileListing {
    pub enabled: bool,
    pub max_file_size_bytes: u64,
    pub chunk_size_bytes: u64,
    pub rate_limit_mbps: u32,
    pub pause_while_drawing: bool,
    pub inbox_name: String,
    pub outbox: Vec<OutgoingFile>,
    pub active_uploads: Vec<ActiveUpload>,
    pub recent: Vec<CompletedTransfer>,
    pub stats: TransferStats,
}

#[derive(Debug, Clone, Serialize)]
pub struct UploadTicket {
    pub upload_id: Uuid,
    pub name: String,
    pub size: u64,
    pub uploaded_bytes: u64,
    pub chunk_size_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct UploadProgress {
    pub upload_id: Uuid,
    pub uploaded_bytes: u64,
    pub total_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct UploadComplete {
    pub upload_id: Uuid,
    pub name: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ByteRange {
    pub start: u64,
    pub end: u64,
}

impl ByteRange {
    #[must_use]
    pub const fn len(self) -> u64 {
        self.end - self.start + 1
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileTransferErrorKind {
    Disabled,
    Unauthorized,
    NotFound,
    Invalid,
    TooLarge,
    Conflict,
    Range,
    Io,
}

#[derive(Debug, Clone)]
pub struct FileTransferError {
    pub kind: FileTransferErrorKind,
    pub message: String,
    pub expected_offset: Option<u64>,
}

impl FileTransferError {
    fn new(kind: FileTransferErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            expected_offset: None,
        }
    }

    fn conflict(message: impl Into<String>, expected_offset: u64) -> Self {
        Self {
            kind: FileTransferErrorKind::Conflict,
            message: message.into(),
            expected_offset: Some(expected_offset),
        }
    }
}

#[derive(Clone)]
pub struct FileTransferManager {
    inner: Arc<Inner>,
}

struct Inner {
    enabled: AtomicBool,
    max_file_size_bytes: AtomicU64,
    rate_limit_mbps: AtomicU32,
    pause_while_drawing: AtomicBool,
    inbox_directory: PathBuf,
    staging_directory: PathBuf,
    session: Arc<SessionManager>,
    metrics: Arc<Metrics>,
    outbox: RwLock<HashMap<Uuid, OutgoingRecord>>,
    uploads: RwLock<HashMap<Uuid, Arc<UploadRecord>>>,
    completed_uploads: Mutex<VecDeque<CompletedUploadRecord>>,
    finalize_lock: AsyncMutex<()>,
    recent: Mutex<VecDeque<CompletedTransfer>>,
    rate: Mutex<VecDeque<RateEvent>>,
    upload_bytes: AtomicU64,
    download_bytes: AtomicU64,
    uploads_completed: AtomicU64,
    downloads_completed: AtomicU64,
    canceled_transfers: AtomicU64,
    failed_transfers: AtomicU64,
    active_downloads: AtomicU32,
    checksum_tx: std::sync::mpsc::Sender<(Uuid, PathBuf)>,
}

#[derive(Clone)]
struct OutgoingRecord {
    public: OutgoingFile,
    path: PathBuf,
    modified: Option<SystemTime>,
}

struct UploadRecord {
    state: AsyncMutex<UploadState>,
    received: AtomicU64,
    id: Uuid,
    session_id: Uuid,
    name: String,
    mime: String,
    size: u64,
    started_epoch_ms: u64,
}

struct CompletedUploadRecord {
    session_id: Uuid,
    response: UploadComplete,
}

struct UploadState {
    part_path: PathBuf,
    received: u64,
    started: Instant,
}

#[derive(Clone, Copy)]
enum RateDirection {
    Upload,
    Download,
}

struct RateEvent {
    at: Instant,
    direction: RateDirection,
    bytes: u64,
}

pub(crate) struct DownloadSource {
    pub body: Body,
    pub file: OutgoingFile,
    pub range: ByteRange,
    pub partial: bool,
}

struct DownloadStreamState {
    file: fs::File,
    remaining: u64,
    guard: DownloadGuard,
}

struct DownloadGuard {
    manager: FileTransferManager,
    session_id: Uuid,
    name: String,
    sha256: Option<String>,
    expected_bytes: u64,
    transferred: u64,
    started: Instant,
    completed: bool,
}

impl FileTransferManager {
    pub fn new(
        options: FileTransferOptions,
        session: Arc<SessionManager>,
        metrics: Arc<Metrics>,
    ) -> Result<Self, String> {
        std::fs::create_dir_all(&options.inbox_directory)
            .map_err(|error| format!("failed to create transfer inbox: {error}"))?;
        std::fs::create_dir_all(&options.staging_directory)
            .map_err(|error| format!("failed to create transfer staging directory: {error}"))?;
        cleanup_staging_directory(&options.staging_directory);
        let (checksum_tx, checksum_rx) = std::sync::mpsc::channel::<(Uuid, PathBuf)>();
        let manager = Self {
            inner: Arc::new(Inner {
                enabled: AtomicBool::new(options.enabled),
                max_file_size_bytes: AtomicU64::new(options.max_file_size_bytes),
                rate_limit_mbps: AtomicU32::new(options.rate_limit_mbps),
                pause_while_drawing: AtomicBool::new(options.pause_while_drawing),
                inbox_directory: options.inbox_directory,
                staging_directory: options.staging_directory,
                session,
                metrics,
                outbox: RwLock::new(HashMap::new()),
                uploads: RwLock::new(HashMap::new()),
                completed_uploads: Mutex::new(VecDeque::new()),
                finalize_lock: AsyncMutex::new(()),
                recent: Mutex::new(VecDeque::new()),
                rate: Mutex::new(VecDeque::new()),
                upload_bytes: AtomicU64::new(0),
                download_bytes: AtomicU64::new(0),
                uploads_completed: AtomicU64::new(0),
                downloads_completed: AtomicU64::new(0),
                canceled_transfers: AtomicU64::new(0),
                failed_transfers: AtomicU64::new(0),
                active_downloads: AtomicU32::new(0),
                checksum_tx,
            }),
        };
        let weak = Arc::downgrade(&manager.inner);
        std::thread::Builder::new()
            .name("nfidb-file-hash".to_owned())
            .spawn(move || {
                while let Ok((id, path)) = checksum_rx.recv() {
                    let hash = hash_file(&path);
                    let Some(inner) = weak.upgrade() else {
                        break;
                    };
                    if let Ok(hash) = hash
                        && let Some(record) = inner.outbox.write().get_mut(&id)
                    {
                        record.public.sha256 = Some(hash);
                    }
                }
            })
            .map_err(|error| format!("failed to start checksum worker: {error}"))?;
        Ok(manager)
    }

    pub fn configure(&self, enabled: bool, max_file_size_bytes: u64, rate_limit_mbps: u32, pause: bool) {
        self.inner.enabled.store(enabled, Ordering::Relaxed);
        self.inner
            .max_file_size_bytes
            .store(max_file_size_bytes, Ordering::Relaxed);
        self.inner.rate_limit_mbps.store(rate_limit_mbps, Ordering::Relaxed);
        self.inner.pause_while_drawing.store(pause, Ordering::Relaxed);
    }

    #[must_use]
    pub fn inbox_directory(&self) -> PathBuf {
        self.inner.inbox_directory.clone()
    }

    pub fn queue_outgoing(&self, path: PathBuf) -> Result<OutgoingFile, String> {
        if !self.inner.enabled.load(Ordering::Relaxed) {
            return Err("file transfer is disabled".to_owned());
        }
        if self.inner.outbox.read().len() >= MAX_OUTBOX_FILES {
            return Err(format!("the iPad queue is limited to {MAX_OUTBOX_FILES} files"));
        }
        let canonical = path
            .canonicalize()
            .map_err(|error| format!("failed to open {}: {error}", path.display()))?;
        let metadata = std::fs::metadata(&canonical)
            .map_err(|error| format!("failed to inspect {}: {error}", canonical.display()))?;
        if !metadata.is_file() {
            return Err(format!("{} is not a regular file", canonical.display()));
        }
        let maximum = self.inner.max_file_size_bytes.load(Ordering::Relaxed);
        if metadata.len() > maximum {
            return Err(format!(
                "{} exceeds the configured {} limit",
                canonical.display(),
                human_bytes(maximum)
            ));
        }
        let raw_name = canonical
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or_else(|| "the selected file name is not valid UTF-8".to_owned())?;
        let name = sanitize_filename(raw_name)?;
        let id = Uuid::new_v4();
        let public = OutgoingFile {
            id,
            mime: mime_guess::from_path(&canonical).first_or_octet_stream().to_string(),
            name,
            size: metadata.len(),
            queued_epoch_ms: epoch_ms(),
            sha256: None,
        };
        self.inner.outbox.write().insert(
            id,
            OutgoingRecord {
                public: public.clone(),
                path: canonical.clone(),
                modified: metadata.modified().ok(),
            },
        );
        if self.inner.checksum_tx.send((id, canonical)).is_err() {
            self.inner.outbox.write().remove(&id);
            return Err("checksum worker is unavailable".to_owned());
        }
        Ok(public)
    }

    pub fn remove_outgoing(&self, id: Uuid) -> bool {
        self.inner.outbox.write().remove(&id).is_some()
    }

    pub fn clear_outgoing(&self) {
        self.inner.outbox.write().clear();
    }

    #[must_use]
    pub fn snapshot(&self) -> FileTransferSnapshot {
        let mut outbox: Vec<_> = self
            .inner
            .outbox
            .read()
            .values()
            .map(|record| record.public.clone())
            .collect();
        outbox.sort_by_key(|file| file.queued_epoch_ms);
        let mut active_uploads: Vec<_> = self
            .inner
            .uploads
            .read()
            .values()
            .map(|upload| upload.public())
            .collect();
        active_uploads.sort_by_key(|upload| upload.started_epoch_ms);
        FileTransferSnapshot {
            enabled: self.inner.enabled.load(Ordering::Relaxed),
            max_file_size_bytes: self.inner.max_file_size_bytes.load(Ordering::Relaxed),
            chunk_size_bytes: UPLOAD_CHUNK_SIZE,
            rate_limit_mbps: self.inner.rate_limit_mbps.load(Ordering::Relaxed),
            pause_while_drawing: self.inner.pause_while_drawing.load(Ordering::Relaxed),
            inbox_directory: self.inner.inbox_directory.clone(),
            outbox,
            active_uploads,
            recent: self.inner.recent.lock().iter().cloned().collect(),
            stats: self.stats(),
        }
    }

    #[must_use]
    pub fn browser_listing(&self, session_id: &Uuid) -> BrowserFileListing {
        let snapshot = self.snapshot();
        BrowserFileListing {
            enabled: snapshot.enabled,
            max_file_size_bytes: snapshot.max_file_size_bytes,
            chunk_size_bytes: snapshot.chunk_size_bytes,
            rate_limit_mbps: snapshot.rate_limit_mbps,
            pause_while_drawing: snapshot.pause_while_drawing,
            inbox_name: self
                .inner
                .inbox_directory
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("NFiDB Inbox")
                .to_owned(),
            outbox: snapshot.outbox,
            active_uploads: snapshot
                .active_uploads
                .into_iter()
                .filter(|upload| {
                    self.inner
                        .uploads
                        .read()
                        .get(&upload.id)
                        .is_some_and(|record| &record.session_id == session_id)
                })
                .collect(),
            recent: snapshot.recent,
            stats: snapshot.stats,
        }
    }

    pub async fn create_upload(
        &self,
        session_id: Uuid,
        requested_id: Option<Uuid>,
        name: &str,
        mime: &str,
        size: u64,
    ) -> Result<UploadTicket, FileTransferError> {
        self.ensure_enabled()?;
        self.ensure_session(&session_id)?;
        let maximum = self.inner.max_file_size_bytes.load(Ordering::Relaxed);
        if size > maximum {
            return Err(FileTransferError::new(
                FileTransferErrorKind::TooLarge,
                format!("file exceeds the configured {} limit", human_bytes(maximum)),
            ));
        }
        let name = sanitize_filename(name)
            .map_err(|message| FileTransferError::new(FileTransferErrorKind::Invalid, message))?;
        let mime = sanitize_mime(mime);
        let id = requested_id.unwrap_or_else(Uuid::new_v4);
        if let Some(record) = self.inner.uploads.read().get(&id) {
            if record.session_id == session_id && record.name == name && record.mime == mime && record.size == size {
                return Ok(record.ticket());
            }
            return Err(FileTransferError::new(
                FileTransferErrorKind::Conflict,
                "upload identifier is already in use",
            ));
        }
        if self.inner.uploads.read().len() >= MAX_ACTIVE_UPLOADS {
            return Err(FileTransferError::new(
                FileTransferErrorKind::Conflict,
                "too many unfinished uploads; cancel one and try again",
            ));
        }
        let part_path = self.inner.staging_directory.join(format!("nfidb-{id}.part"));
        OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&part_path)
            .await
            .map_err(|error| FileTransferError::new(FileTransferErrorKind::Io, error.to_string()))?;
        let started_epoch_ms = epoch_ms();
        let record = Arc::new(UploadRecord {
            state: AsyncMutex::new(UploadState {
                part_path: part_path.clone(),
                received: 0,
                started: Instant::now(),
            }),
            received: AtomicU64::new(0),
            id,
            session_id,
            name: name.clone(),
            mime,
            size,
            started_epoch_ms,
        });
        let ticket = record.ticket();
        self.inner.uploads.write().insert(id, record);
        Ok(ticket)
    }

    pub fn upload_progress(&self, id: Uuid, session_id: &Uuid) -> Result<UploadProgress, FileTransferError> {
        if let Some(completed) = self.completed_upload(id, session_id) {
            return Ok(UploadProgress {
                upload_id: id,
                uploaded_bytes: completed.size,
                total_bytes: completed.size,
            });
        }
        let record = self.upload_record(id, session_id)?;
        Ok(UploadProgress {
            upload_id: id,
            uploaded_bytes: record.received.load(Ordering::Acquire),
            total_bytes: record.size,
        })
    }

    pub async fn write_upload_chunk(
        &self,
        id: Uuid,
        session_id: &Uuid,
        offset: u64,
        bytes: Bytes,
        expected_sha256: &str,
    ) -> Result<UploadProgress, FileTransferError> {
        self.ensure_enabled()?;
        self.ensure_session(session_id)?;
        if bytes.len() as u64 > UPLOAD_CHUNK_SIZE {
            return Err(FileTransferError::new(
                FileTransferErrorKind::TooLarge,
                "upload chunk exceeds the 1 MiB limit",
            ));
        }
        let actual_hash = hex_sha256(&bytes);
        if expected_sha256.len() != 64 || !actual_hash.eq_ignore_ascii_case(expected_sha256) {
            return Err(FileTransferError::conflict(
                "upload chunk checksum did not match",
                offset,
            ));
        }
        let record = self.upload_record(id, session_id)?;
        let mut state = record.state.lock().await;
        if offset != state.received {
            return Err(FileTransferError::conflict(
                "upload offset is stale; resume from the expected offset",
                state.received,
            ));
        }
        let next = state.received.saturating_add(bytes.len() as u64);
        if next > record.size {
            return Err(FileTransferError::new(
                FileTransferErrorKind::Invalid,
                "upload contains more bytes than declared",
            ));
        }
        let mut file = OpenOptions::new()
            .write(true)
            .open(&state.part_path)
            .await
            .map_err(|error| FileTransferError::new(FileTransferErrorKind::Io, error.to_string()))?;
        file.seek(io::SeekFrom::Start(offset))
            .await
            .map_err(|error| FileTransferError::new(FileTransferErrorKind::Io, error.to_string()))?;
        file.write_all(&bytes)
            .await
            .map_err(|error| FileTransferError::new(FileTransferErrorKind::Io, error.to_string()))?;
        file.flush()
            .await
            .map_err(|error| FileTransferError::new(FileTransferErrorKind::Io, error.to_string()))?;
        state.received = next;
        record.received.store(next, Ordering::Release);
        drop(state);
        self.record_bytes(RateDirection::Upload, bytes.len() as u64);
        self.pace(bytes.len() as u64, session_id).await?;
        Ok(UploadProgress {
            upload_id: id,
            uploaded_bytes: next,
            total_bytes: record.size,
        })
    }

    pub async fn complete_upload(&self, id: Uuid, session_id: &Uuid) -> Result<UploadComplete, FileTransferError> {
        self.ensure_enabled()?;
        self.ensure_session(session_id)?;
        let _finalize = self.inner.finalize_lock.lock().await;
        if let Some(completed) = self.completed_upload(id, session_id) {
            return Ok(completed);
        }
        let record = self.upload_record(id, session_id)?;
        let state = record.state.lock().await;
        if state.received != record.size {
            return Err(FileTransferError::conflict("upload is incomplete", state.received));
        }
        let part_path = state.part_path.clone();
        let started = state.started;
        drop(state);
        let hash_path = part_path.clone();
        let sha256 = tokio::task::spawn_blocking(move || hash_file(&hash_path))
            .await
            .map_err(|error| FileTransferError::new(FileTransferErrorKind::Io, error.to_string()))?
            .map_err(|error| FileTransferError::new(FileTransferErrorKind::Io, error.to_string()))?;
        let destination = unique_destination(&self.inner.inbox_directory, &record.name);
        move_completed_file(&part_path, &destination, id, record.size).await?;
        self.inner.uploads.write().remove(&id);
        self.inner.uploads_completed.fetch_add(1, Ordering::Relaxed);
        let saved_name = destination
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(&record.name)
            .to_owned();
        self.record_completed(
            TransferDirection::IpadToWindows,
            saved_name.clone(),
            record.size,
            started.elapsed(),
            Some(sha256.clone()),
            "completed",
        );
        tracing::info!(
            file_name = %record.name,
            bytes = record.size,
            mime = %record.mime,
            "received file from iPad"
        );
        let response = UploadComplete {
            upload_id: id,
            name: saved_name,
            size: record.size,
            sha256,
        };
        let mut completed = self.inner.completed_uploads.lock();
        completed.push_front(CompletedUploadRecord {
            session_id: *session_id,
            response: response.clone(),
        });
        completed.truncate(MAX_COMPLETED_UPLOAD_TICKETS);
        Ok(response)
    }

    pub async fn cancel_upload(&self, id: Uuid, session_id: &Uuid) -> Result<(), FileTransferError> {
        if self.completed_upload(id, session_id).is_some() {
            return Ok(());
        }
        let record = self.upload_record(id, session_id)?;
        self.inner.uploads.write().remove(&id);
        let state = record.state.lock().await;
        let _ = fs::remove_file(&state.part_path).await;
        self.inner.canceled_transfers.fetch_add(1, Ordering::Relaxed);
        self.record_completed(
            TransferDirection::IpadToWindows,
            record.name.clone(),
            state.received,
            state.started.elapsed(),
            None,
            "canceled",
        );
        Ok(())
    }

    pub fn cancel_session_uploads(&self, session_id: &Uuid) {
        let records: Vec<_> = {
            let mut uploads = self.inner.uploads.write();
            let ids: Vec<_> = uploads
                .iter()
                .filter_map(|(id, record)| (&record.session_id == session_id).then_some(*id))
                .collect();
            ids.into_iter().filter_map(|id| uploads.remove(&id)).collect()
        };
        for record in records {
            let part_path = self.inner.staging_directory.join(format!("nfidb-{}.part", record.id));
            let _ = std::fs::remove_file(part_path);
            self.inner.canceled_transfers.fetch_add(1, Ordering::Relaxed);
        }
        self.inner
            .completed_uploads
            .lock()
            .retain(|record| &record.session_id != session_id);
    }

    pub(crate) async fn open_download(
        &self,
        id: Uuid,
        session_id: Uuid,
        range_header: Option<&str>,
    ) -> Result<DownloadSource, FileTransferError> {
        self.ensure_enabled()?;
        self.ensure_session(&session_id)?;
        let record = self
            .inner
            .outbox
            .read()
            .get(&id)
            .cloned()
            .ok_or_else(|| FileTransferError::new(FileTransferErrorKind::NotFound, "file is no longer queued"))?;
        let metadata = fs::metadata(&record.path).await.map_err(|_| {
            FileTransferError::new(FileTransferErrorKind::NotFound, "queued file is no longer available")
        })?;
        if metadata.len() != record.public.size || metadata.modified().ok() != record.modified {
            return Err(FileTransferError::new(
                FileTransferErrorKind::Conflict,
                "queued file changed on disk; remove it and add it again",
            ));
        }
        let requested_range = parse_range(range_header, record.public.size)?;
        let range = requested_range.unwrap_or(ByteRange {
            start: 0,
            end: record.public.size.saturating_sub(1),
        });
        if record.public.size == 0 {
            let body = Body::empty();
            self.inner.downloads_completed.fetch_add(1, Ordering::Relaxed);
            self.record_completed(
                TransferDirection::WindowsToIpad,
                record.public.name.clone(),
                0,
                Duration::ZERO,
                record.public.sha256.clone(),
                "completed",
            );
            return Ok(DownloadSource {
                body,
                file: record.public,
                range: ByteRange { start: 0, end: 0 },
                partial: requested_range.is_some(),
            });
        }
        if range.start >= record.public.size || range.end >= record.public.size || range.start > range.end {
            return Err(FileTransferError::new(
                FileTransferErrorKind::Range,
                "requested byte range is outside the file",
            ));
        }
        let mut file = fs::File::open(&record.path)
            .await
            .map_err(|error| FileTransferError::new(FileTransferErrorKind::Io, error.to_string()))?;
        file.seek(io::SeekFrom::Start(range.start))
            .await
            .map_err(|error| FileTransferError::new(FileTransferErrorKind::Io, error.to_string()))?;
        self.inner.active_downloads.fetch_add(1, Ordering::Relaxed);
        let guard = DownloadGuard {
            manager: self.clone(),
            session_id,
            name: record.public.name.clone(),
            sha256: record.public.sha256.clone(),
            expected_bytes: range.len(),
            transferred: 0,
            started: Instant::now(),
            completed: false,
        };
        let state = DownloadStreamState {
            file,
            remaining: range.len(),
            guard,
        };
        let body = Body::from_stream(stream::unfold(state, |mut state| async move {
            if state.remaining == 0 {
                state.guard.complete();
                return None;
            }
            let amount = state.remaining.min(DOWNLOAD_CHUNK_SIZE as u64) as usize;
            if let Err(error) = state.guard.manager.pace(amount as u64, &state.guard.session_id).await {
                return Some((
                    Err(io::Error::new(io::ErrorKind::PermissionDenied, error.message)),
                    state,
                ));
            }
            let mut buffer = vec![0_u8; amount];
            match state.file.read(&mut buffer).await {
                Ok(0) => Some((
                    Err(io::Error::new(io::ErrorKind::UnexpectedEof, "queued file ended early")),
                    state,
                )),
                Ok(read) => {
                    buffer.truncate(read);
                    state.remaining = state.remaining.saturating_sub(read as u64);
                    state.guard.transferred += read as u64;
                    state.guard.manager.record_bytes(RateDirection::Download, read as u64);
                    Some((Ok::<Bytes, io::Error>(Bytes::from(buffer)), state))
                }
                Err(error) => Some((Err(error), state)),
            }
        }));
        Ok(DownloadSource {
            body,
            file: record.public,
            range,
            partial: requested_range.is_some(),
        })
    }

    fn ensure_enabled(&self) -> Result<(), FileTransferError> {
        if self.inner.enabled.load(Ordering::Relaxed) {
            Ok(())
        } else {
            Err(FileTransferError::new(
                FileTransferErrorKind::Disabled,
                "file transfer is disabled on Windows",
            ))
        }
    }

    fn upload_record(&self, id: Uuid, session_id: &Uuid) -> Result<Arc<UploadRecord>, FileTransferError> {
        let record = self
            .inner
            .uploads
            .read()
            .get(&id)
            .cloned()
            .ok_or_else(|| FileTransferError::new(FileTransferErrorKind::NotFound, "upload was not found"))?;
        if &record.session_id != session_id {
            return Err(FileTransferError::new(
                FileTransferErrorKind::Unauthorized,
                "upload belongs to a different session",
            ));
        }
        Ok(record)
    }

    fn completed_upload(&self, id: Uuid, session_id: &Uuid) -> Option<UploadComplete> {
        self.inner
            .completed_uploads
            .lock()
            .iter()
            .find(|record| record.session_id == *session_id && record.response.upload_id == id)
            .map(|record| record.response.clone())
    }

    async fn pace(&self, bytes: u64, session_id: &Uuid) -> Result<(), FileTransferError> {
        self.ensure_enabled()?;
        while self.inner.pause_while_drawing.load(Ordering::Relaxed) && self.inner.metrics.has_active_pointers() {
            self.ensure_session(session_id)?;
            tokio::time::sleep(Duration::from_millis(35)).await;
        }
        self.ensure_session(session_id)?;
        let megabits = self.inner.rate_limit_mbps.load(Ordering::Relaxed);
        if megabits > 0 && bytes > 0 {
            let seconds = bytes as f64 * 8.0 / (megabits as f64 * 1_000_000.0);
            tokio::time::sleep(Duration::from_secs_f64(seconds)).await;
        }
        self.ensure_session(session_id)
    }

    fn ensure_session(&self, session_id: &Uuid) -> Result<(), FileTransferError> {
        if self.inner.session.session_id() == *session_id && self.inner.session.is_paired() {
            Ok(())
        } else {
            Err(FileTransferError::new(
                FileTransferErrorKind::Unauthorized,
                "paired session changed during transfer",
            ))
        }
    }

    fn record_bytes(&self, direction: RateDirection, bytes: u64) {
        match direction {
            RateDirection::Upload => {
                self.inner.upload_bytes.fetch_add(bytes, Ordering::Relaxed);
            }
            RateDirection::Download => {
                self.inner.download_bytes.fetch_add(bytes, Ordering::Relaxed);
            }
        }
        let now = Instant::now();
        let mut rate = self.inner.rate.lock();
        rate.push_back(RateEvent {
            at: now,
            direction,
            bytes,
        });
        prune_rate_events(&mut rate, now);
    }

    fn stats(&self) -> TransferStats {
        let now = Instant::now();
        let mut rate = self.inner.rate.lock();
        prune_rate_events(&mut rate, now);
        let (upload, download) = rate
            .iter()
            .fold((0_u64, 0_u64), |(upload, download), event| match event.direction {
                RateDirection::Upload => (upload + event.bytes, download),
                RateDirection::Download => (upload, download + event.bytes),
            });
        TransferStats {
            upload_bytes: self.inner.upload_bytes.load(Ordering::Relaxed),
            download_bytes: self.inner.download_bytes.load(Ordering::Relaxed),
            uploads_completed: self.inner.uploads_completed.load(Ordering::Relaxed),
            downloads_completed: self.inner.downloads_completed.load(Ordering::Relaxed),
            canceled_transfers: self.inner.canceled_transfers.load(Ordering::Relaxed),
            failed_transfers: self.inner.failed_transfers.load(Ordering::Relaxed),
            active_uploads: self.inner.uploads.read().len() as u32,
            active_downloads: self.inner.active_downloads.load(Ordering::Relaxed),
            upload_mbps: upload as f64 * 8.0 / 1_000_000.0,
            download_mbps: download as f64 * 8.0 / 1_000_000.0,
        }
    }

    fn record_completed(
        &self,
        direction: TransferDirection,
        name: String,
        bytes: u64,
        duration: Duration,
        sha256: Option<String>,
        status: &str,
    ) {
        let seconds = duration.as_secs_f64().max(0.001);
        let transfer = CompletedTransfer {
            direction,
            name,
            bytes,
            duration_ms: duration.as_millis().min(u128::from(u64::MAX)) as u64,
            average_mbps: bytes as f64 * 8.0 / seconds / 1_000_000.0,
            sha256,
            completed_epoch_ms: epoch_ms(),
            status: status.to_owned(),
        };
        let mut recent = self.inner.recent.lock();
        recent.push_front(transfer);
        recent.truncate(MAX_RECENT_TRANSFERS);
    }
}

impl UploadRecord {
    fn public(&self) -> ActiveUpload {
        ActiveUpload {
            id: self.id,
            name: self.name.clone(),
            size: self.size,
            received: self.received.load(Ordering::Acquire),
            started_epoch_ms: self.started_epoch_ms,
        }
    }

    fn ticket(&self) -> UploadTicket {
        UploadTicket {
            upload_id: self.id,
            name: self.name.clone(),
            size: self.size,
            uploaded_bytes: self.received.load(Ordering::Acquire),
            chunk_size_bytes: UPLOAD_CHUNK_SIZE,
        }
    }
}

impl DownloadGuard {
    fn complete(&mut self) {
        if self.completed {
            return;
        }
        self.completed = true;
        self.manager.inner.downloads_completed.fetch_add(1, Ordering::Relaxed);
        self.manager.record_completed(
            TransferDirection::WindowsToIpad,
            self.name.clone(),
            self.transferred,
            self.started.elapsed(),
            self.sha256.clone(),
            "completed",
        );
    }
}

impl Drop for DownloadGuard {
    fn drop(&mut self) {
        self.manager.inner.active_downloads.fetch_sub(1, Ordering::Relaxed);
        if !self.completed {
            let status = if self.manager.inner.session.session_id() != self.session_id {
                "session-ended"
            } else if self.transferred < self.expected_bytes {
                "interrupted"
            } else {
                "completed"
            };
            if status == "completed" {
                self.manager.inner.downloads_completed.fetch_add(1, Ordering::Relaxed);
            } else {
                self.manager.inner.failed_transfers.fetch_add(1, Ordering::Relaxed);
            }
            self.manager.record_completed(
                TransferDirection::WindowsToIpad,
                self.name.clone(),
                self.transferred,
                self.started.elapsed(),
                self.sha256.clone(),
                status,
            );
        }
    }
}

pub(crate) fn parse_range(value: Option<&str>, size: u64) -> Result<Option<ByteRange>, FileTransferError> {
    let Some(value) = value else {
        return Ok(None);
    };
    if size == 0 {
        return Err(FileTransferError::new(
            FileTransferErrorKind::Range,
            "an empty file has no satisfiable byte range",
        ));
    }
    let spec = value
        .strip_prefix("bytes=")
        .ok_or_else(|| FileTransferError::new(FileTransferErrorKind::Range, "only byte ranges are supported"))?;
    if spec.contains(',') {
        return Err(FileTransferError::new(
            FileTransferErrorKind::Range,
            "multiple byte ranges are not supported",
        ));
    }
    let (start, end) = spec
        .split_once('-')
        .ok_or_else(|| FileTransferError::new(FileTransferErrorKind::Range, "malformed byte range"))?;
    let range = if start.is_empty() {
        let suffix = end
            .parse::<u64>()
            .ok()
            .filter(|value| *value > 0)
            .ok_or_else(|| FileTransferError::new(FileTransferErrorKind::Range, "invalid suffix range"))?;
        let length = suffix.min(size);
        ByteRange {
            start: size - length,
            end: size - 1,
        }
    } else {
        let start = start
            .parse::<u64>()
            .map_err(|_| FileTransferError::new(FileTransferErrorKind::Range, "invalid range start"))?;
        let end = if end.is_empty() {
            size - 1
        } else {
            end.parse::<u64>()
                .map_err(|_| FileTransferError::new(FileTransferErrorKind::Range, "invalid range end"))?
                .min(size - 1)
        };
        ByteRange { start, end }
    };
    if range.start >= size || range.start > range.end {
        return Err(FileTransferError::new(
            FileTransferErrorKind::Range,
            "requested byte range is not satisfiable",
        ));
    }
    Ok(Some(range))
}

#[must_use]
pub(crate) fn content_disposition(name: &str) -> String {
    let fallback: String = name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_' | ' ') {
                character
            } else {
                '_'
            }
        })
        .collect();
    let encoded = name
        .as_bytes()
        .iter()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(*byte, b'-' | b'.' | b'_') {
                (*byte as char).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect::<String>();
    format!("attachment; filename=\"{fallback}\"; filename*=UTF-8''{encoded}")
}

fn sanitize_filename(value: &str) -> Result<String, String> {
    let leaf = Path::new(value)
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "file name is empty".to_owned())?;
    if leaf != value || value.contains(['/', '\\']) {
        return Err("file name may not contain a path".to_owned());
    }
    let mut name: String = leaf
        .chars()
        .filter(|character| !character.is_control())
        .map(|character| {
            if matches!(character, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') {
                '_'
            } else {
                character
            }
        })
        .take(180)
        .collect();
    name = name.trim().trim_end_matches(['.', ' ']).to_owned();
    if name.is_empty() || name == "." || name == ".." {
        return Err("file name is empty after sanitization".to_owned());
    }
    let stem = name.split('.').next().unwrap_or(&name).to_ascii_uppercase();
    if matches!(
        stem.as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    ) {
        name.insert(0, '_');
    }
    Ok(name)
}

fn sanitize_mime(value: &str) -> String {
    let value = value.trim();
    if value.len() <= 127
        && !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_graphic() && !matches!(byte, b'\r' | b'\n'))
    {
        value.to_owned()
    } else {
        "application/octet-stream".to_owned()
    }
}

fn unique_destination(directory: &Path, name: &str) -> PathBuf {
    let candidate = directory.join(name);
    if !candidate.exists() {
        return candidate;
    }
    let path = Path::new(name);
    let stem = path.file_stem().and_then(|value| value.to_str()).unwrap_or("file");
    let extension = path.extension().and_then(|value| value.to_str());
    for suffix in 1..100_000_u32 {
        let name = extension.map_or_else(
            || format!("{stem} ({suffix})"),
            |extension| format!("{stem} ({suffix}).{extension}"),
        );
        let candidate = directory.join(name);
        if !candidate.exists() {
            return candidate;
        }
    }
    directory.join(format!("{}-{name}", Uuid::new_v4()))
}

async fn move_completed_file(
    source: &Path,
    destination: &Path,
    upload_id: Uuid,
    expected_size: u64,
) -> Result<(), FileTransferError> {
    match fs::rename(source, destination).await {
        Ok(()) => return Ok(()),
        Err(rename_error) => {
            // A relocated Downloads folder may be on another volume. Copy to an
            // owned temporary leaf beside the destination, then rename locally so
            // the final user-facing name never exposes a partial file.
            let Some(directory) = destination.parent() else {
                return Err(FileTransferError::new(
                    FileTransferErrorKind::Io,
                    format!("failed to finalize upload: {rename_error}"),
                ));
            };
            let temporary = directory.join(format!(".nfidb-{upload_id}.incoming"));
            let _ = fs::remove_file(&temporary).await;
            let copied = fs::copy(source, &temporary).await.map_err(|copy_error| {
                FileTransferError::new(
                    FileTransferErrorKind::Io,
                    format!("failed to move upload ({rename_error}); fallback copy failed: {copy_error}"),
                )
            })?;
            if copied != expected_size {
                let _ = fs::remove_file(&temporary).await;
                return Err(FileTransferError::new(
                    FileTransferErrorKind::Io,
                    format!("fallback copy wrote {copied} of {expected_size} bytes"),
                ));
            }
            if let Err(error) = fs::rename(&temporary, destination).await {
                let _ = fs::remove_file(&temporary).await;
                return Err(FileTransferError::new(
                    FileTransferErrorKind::Io,
                    format!("failed to publish copied upload: {error}"),
                ));
            }
            if let Err(error) = fs::remove_file(source).await {
                tracing::warn!(%error, path = %source.display(), "finalized upload but could not remove staging file");
            }
        }
    }
    Ok(())
}

fn hash_file(path: &Path) -> io::Result<String> {
    let mut file = StdFile::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn hex_sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn cleanup_staging_directory(directory: &Path) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let removable = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("nfidb-") && name.ends_with(".part"));
        if removable {
            let _ = std::fs::remove_file(path);
        }
    }
}

fn prune_rate_events(events: &mut VecDeque<RateEvent>, now: Instant) {
    while events
        .front()
        .is_some_and(|event| now.duration_since(event.at) > RATE_WINDOW)
    {
        events.pop_front();
    }
}

fn epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

fn human_bytes(bytes: u64) -> String {
    if bytes < 1024 * 1024 * 1024 {
        format!("{:.0} MiB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1} GiB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use nfidb_core::{Metrics, SessionManager};
    use nfidb_protocol::{Action, DeviceType, PointerBatch, PointerSample};
    use tempfile::TempDir;

    use super::*;

    fn manager(temp: &TempDir) -> (FileTransferManager, Arc<SessionManager>, Uuid) {
        let session = Arc::new(SessionManager::new());
        let pin = session.pin();
        let paired = session.pair_with_pin(&pin).expect("pair test session");
        let manager = FileTransferManager::new(
            FileTransferOptions {
                enabled: true,
                max_file_size_bytes: 32 * 1024 * 1024,
                rate_limit_mbps: 0,
                pause_while_drawing: false,
                inbox_directory: temp.path().join("inbox"),
                staging_directory: temp.path().join("staging"),
            },
            Arc::clone(&session),
            Arc::new(Metrics::default()),
        )
        .expect("create manager");
        (manager, session, paired.session_id)
    }

    #[test]
    fn sanitizes_paths_and_windows_reserved_names() {
        assert_eq!(sanitize_filename("drawing?.kra").unwrap(), "drawing_.kra");
        assert_eq!(sanitize_filename("CON.txt").unwrap(), "_CON.txt");
        assert!(sanitize_filename("../secret.txt").is_err());
        assert!(sanitize_filename("folder/file.txt").is_err());
    }

    #[test]
    fn parses_single_open_and_suffix_ranges() {
        assert_eq!(
            parse_range(Some("bytes=4-8"), 20).unwrap(),
            Some(ByteRange { start: 4, end: 8 })
        );
        assert_eq!(
            parse_range(Some("bytes=10-"), 20).unwrap(),
            Some(ByteRange { start: 10, end: 19 })
        );
        assert_eq!(
            parse_range(Some("bytes=-5"), 20).unwrap(),
            Some(ByteRange { start: 15, end: 19 })
        );
        assert!(parse_range(Some("bytes=30-40"), 20).is_err());
        assert!(parse_range(Some("bytes=1-2,5-6"), 20).is_err());
    }

    #[tokio::test]
    async fn uploads_verified_chunks_and_avoids_overwriting_existing_files() {
        let temp = TempDir::new().unwrap();
        let (manager, _session, session_id) = manager(&temp);
        std::fs::write(temp.path().join("inbox").join("drawing.txt"), b"existing").unwrap();
        let upload_id = Uuid::new_v4();
        let ticket = manager
            .create_upload(session_id, Some(upload_id), "drawing.txt", "text/plain", 11)
            .await
            .unwrap();
        let retry_ticket = manager
            .create_upload(session_id, Some(upload_id), "drawing.txt", "text/plain", 11)
            .await
            .unwrap();
        assert_eq!(retry_ticket.upload_id, ticket.upload_id);
        let first = Bytes::from_static(b"hello ");
        manager
            .write_upload_chunk(ticket.upload_id, &session_id, 0, first.clone(), &hex_sha256(&first))
            .await
            .unwrap();
        let second = Bytes::from_static(b"world");
        manager
            .write_upload_chunk(ticket.upload_id, &session_id, 6, second.clone(), &hex_sha256(&second))
            .await
            .unwrap();
        let complete = manager.complete_upload(ticket.upload_id, &session_id).await.unwrap();
        let retry_complete = manager.complete_upload(ticket.upload_id, &session_id).await.unwrap();
        assert_eq!(retry_complete.sha256, complete.sha256);
        assert_eq!(complete.name, "drawing (1).txt");
        assert_eq!(
            std::fs::read(temp.path().join("inbox").join("drawing (1).txt")).unwrap(),
            b"hello world"
        );
        assert_eq!(complete.sha256, hex_sha256(b"hello world"));
        assert_eq!(manager.snapshot().stats.uploads_completed, 1);
    }

    #[tokio::test]
    async fn rejects_corrupt_and_out_of_order_upload_chunks() {
        let temp = TempDir::new().unwrap();
        let (manager, _session, session_id) = manager(&temp);
        let ticket = manager
            .create_upload(session_id, None, "test.bin", "application/octet-stream", 3)
            .await
            .unwrap();
        let corrupt = manager
            .write_upload_chunk(ticket.upload_id, &session_id, 0, Bytes::from_static(b"abc"), "0")
            .await
            .unwrap_err();
        assert_eq!(corrupt.kind, FileTransferErrorKind::Conflict);
        let bytes = Bytes::from_static(b"abc");
        let stale = manager
            .write_upload_chunk(ticket.upload_id, &session_id, 1, bytes.clone(), &hex_sha256(&bytes))
            .await
            .unwrap_err();
        assert_eq!(stale.expected_offset, Some(0));
    }

    #[test]
    fn queues_only_regular_files_without_exposing_the_path() {
        let temp = TempDir::new().unwrap();
        let (manager, _session, _session_id) = manager(&temp);
        let source = temp.path().join("outbound sample.txt");
        std::fs::write(&source, b"sample").unwrap();
        let queued = manager.queue_outgoing(source).unwrap();
        assert_eq!(queued.name, "outbound sample.txt");
        let json = serde_json::to_string(&manager.snapshot().outbox).unwrap();
        assert!(!json.contains(temp.path().to_string_lossy().as_ref()));
        let diagnostic_json = serde_json::to_string(&manager.snapshot()).unwrap();
        assert!(!diagnostic_json.contains(temp.path().to_string_lossy().as_ref()));
    }

    #[tokio::test]
    async fn transfer_pacing_waits_for_an_active_drawing_contact() {
        let temp = TempDir::new().unwrap();
        let session = Arc::new(SessionManager::new());
        let pin = session.pin();
        let paired = session.pair_with_pin(&pin).unwrap();
        let metrics = Arc::new(Metrics::default());
        let manager = FileTransferManager::new(
            FileTransferOptions {
                enabled: true,
                max_file_size_bytes: 1024,
                rate_limit_mbps: 0,
                pause_while_drawing: true,
                inbox_directory: temp.path().join("inbox"),
                staging_directory: temp.path().join("staging"),
            },
            Arc::clone(&session),
            Arc::clone(&metrics),
        )
        .unwrap();
        let sample = |action, sequence| PointerSample {
            device_type: DeviceType::Pen,
            action,
            flags: 0,
            pointer_id: 9,
            sample_sequence: sequence,
            x_norm: 0.5,
            y_norm: 0.5,
            pressure: 0.5,
            tilt_x_deg: 0.0,
            tilt_y_deg: 0.0,
            twist_deg: 0.0,
            client_time_ms: 0.0,
        };
        metrics.input_batch(&PointerBatch {
            batch_sequence: 0,
            client_send_time_ms: 0.0,
            samples: vec![sample(Action::Down, 0)],
        });
        assert!(
            tokio::time::timeout(Duration::from_millis(80), manager.pace(64, &paired.session_id))
                .await
                .is_err()
        );
        metrics.input_batch(&PointerBatch {
            batch_sequence: 1,
            client_send_time_ms: 0.0,
            samples: vec![sample(Action::Up, 1)],
        });
        tokio::time::timeout(Duration::from_millis(150), manager.pace(64, &paired.session_id))
            .await
            .expect("pacing should resume after pen-up")
            .expect("paired session remains valid");
        metrics.input_batch(&PointerBatch {
            batch_sequence: 2,
            client_send_time_ms: 0.0,
            samples: vec![sample(Action::Down, 2)],
        });
        metrics.reset_input_continuity();
        tokio::time::timeout(Duration::from_millis(150), manager.pace(64, &paired.session_id))
            .await
            .expect("pacing should resume after a disconnected contact is reset")
            .expect("paired session remains valid");
    }
}
