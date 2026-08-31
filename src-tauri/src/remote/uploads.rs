use crate::tabs::{AttachmentId, TabId};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
#[cfg(unix)]
use std::ffi::CString;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs::{self, File};
use std::io::{Cursor, Read, Seek, SeekFrom, Write};
#[cfg(unix)]
use std::os::fd::{AsRawFd, FromRawFd};
#[cfg(unix)]
use std::os::unix::ffi::{OsStrExt, OsStringExt};
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use uuid::Uuid;
use zune_core::options::DecoderOptions;
use zune_jpeg::JpegDecoder;

pub const MAX_UPLOAD_BYTES: u64 = 12 * 1024 * 1024;
pub const MAX_UPLOAD_CHUNK_BYTES: usize = 256 * 1024;
pub const MAX_IMAGE_EDGE: u32 = 4096;
pub const MAX_UPLOADS_PER_SUBMISSION: usize = 4;
pub const MAX_SUBMISSION_BYTES: u64 = 48 * 1024 * 1024;
pub const ATTACHMENT_TTL: Duration = Duration::from_secs(24 * 60 * 60);
pub const PARTIAL_ATTACHMENT_TTL: Duration = Duration::from_secs(15 * 60);
pub const ATTACHMENT_BUDGET_BYTES: u64 = 256 * 1024 * 1024;
pub const MAX_CLOSED_SUBMISSIONS: usize = 64;
const MAX_GIT_EXCLUDE_BYTES: u64 = 1024 * 1024;
const MANIFEST_VERSION: u8 = 1;
const MANIFEST_NAME: &str = "attachments.json";
const MANIFEST_LOCK_NAME: &str = ".attachments.lock";
const MAX_MANIFEST_BYTES: u64 = 4 * 1024 * 1024;
const MAX_MANIFEST_RECORDS: usize = 4096;
static NEVER_CANCELLED: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Debug)]
pub struct UploadBegin {
    pub tab_id: TabId,
    pub attachment_id: AttachmentId,
    pub submission_id: String,
    pub submission_count: u8,
    pub submission_bytes: u64,
    pub length: u64,
    pub sha256: [u8; 32],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UploadBegan {
    pub upload_id: String,
    pub next_chunk: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UploadChunk {
    pub upload_id: String,
    pub index: u32,
    pub data: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublishedUpload {
    pub path: PathBuf,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UploadErrorKind {
    Cancelled,
    NotFound,
    TooLarge,
    OutOfOrder,
    LengthMismatch,
    DigestMismatch,
    InvalidImage,
    InvalidSubmission,
    ClosedSubmission,
    Busy,
    Capacity,
    UnsafePath,
    Storage,
}

#[derive(Debug)]
pub struct UploadError {
    kind: UploadErrorKind,
    message: String,
}

impl UploadError {
    pub fn kind(&self) -> UploadErrorKind {
        self.kind
    }

    fn new(kind: UploadErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    fn storage(action: &str, error: impl fmt::Display) -> Self {
        Self::new(UploadErrorKind::Storage, format!("{action}: {error}"))
    }
}

impl fmt::Display for UploadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for UploadError {}

#[derive(Clone, Debug)]
pub struct AttachmentStore {
    fallback_cache: Arc<StableDirectory>,
    manifest: Arc<ManifestStorage>,
}

impl AttachmentStore {
    /// Create a store whose fallback path is used for tabs without a known cwd.
    ///
    /// The caller supplies AITerm's cache root. The private remote-attachments
    /// child holds both the fallback files and the persistent ownership manifest.
    pub fn new(cache_root: PathBuf) -> Result<Self, UploadError> {
        Self::new_at(cache_root, SystemTime::now())
    }

    pub fn new_at(cache_root: PathBuf, now: SystemTime) -> Result<Self, UploadError> {
        let fallback_cache =
            StableDirectory::open_or_create_tree(&cache_root.join("remote-attachments"))?;
        fallback_cache.set_owner_only()?;
        let manifest = Arc::new(ManifestStorage::new(fallback_cache.duplicate()?)?);
        let store = Self {
            fallback_cache: Arc::new(fallback_cache),
            manifest,
        };
        store.maintain(now)?;
        Ok(store)
    }

    pub fn system() -> Result<Self, UploadError> {
        let cache = dirs::cache_dir()
            .ok_or_else(|| UploadError::new(UploadErrorKind::Storage, "no cache directory"))?
            .join("aiterm");
        Self::new(cache)
    }

    pub fn maintain(&self, now: SystemTime) -> Result<(), UploadError> {
        self.manifest.maintain(now)
    }

    pub fn upload_set(&self) -> UploadSet {
        UploadSet {
            store: self.clone(),
            uploads: HashMap::new(),
            upload_members: HashMap::new(),
            submission: None,
            closed_submissions: HashSet::new(),
        }
    }

    fn stage(
        &self,
        tab_cwd: Option<&Path>,
        length: u64,
        now: SystemTime,
    ) -> Result<StagedFile, UploadError> {
        let directory = match tab_cwd {
            Some(cwd) => {
                let canonical = canonical_project_cwd(cwd)?;
                let attachments = project_attachment_directory(&canonical)?;
                update_local_git_exclude(&canonical)?;
                attachments
            }
            None => self.fallback_cache.duplicate()?,
        };

        let basename = Uuid::new_v4().hyphenated().to_string();
        let published_name = OsString::from(format!("{basename}.jpg"));
        let part_name = OsString::from(format!("{basename}.jpg.part"));
        let staged_directory = directory.duplicate()?;
        let file = directory
            .create_anonymous_file()
            .map_err(|error| UploadError::storage("create anonymous staged attachment", error))?;
        file.sync_all()
            .map_err(|error| UploadError::storage("sync anonymous staged attachment", error))?;
        let identity = FileIdentity::of(&file)?;
        let mut staged = StagedFile {
            directory: staged_directory,
            manifest: self.manifest.clone(),
            record_id: basename,
            part_name,
            published_name,
            file,
            published: false,
            manifested: false,
        };
        self.manifest.record_partial_and_link(
            &staged.record_id,
            &staged.directory.path,
            &staged.part_name,
            length,
            now,
            identity,
            &staged.directory,
            &staged.file,
        )?;
        staged.manifested = true;
        staged.verify_bound_entry()?;
        Ok(staged)
    }
}

pub struct UploadSet {
    store: AttachmentStore,
    uploads: HashMap<String, ActiveUpload>,
    upload_members: HashMap<String, UploadMember>,
    submission: Option<SubmissionState>,
    closed_submissions: HashSet<String>,
}

impl UploadSet {
    /// Begin one upload using only the cwd resolved by the desktop tab registry.
    pub fn begin(
        &mut self,
        tab_cwd: Option<&Path>,
        request: UploadBegin,
    ) -> Result<UploadBegan, UploadError> {
        self.begin_at(tab_cwd, request, SystemTime::now())
    }

    pub fn begin_at(
        &mut self,
        tab_cwd: Option<&Path>,
        request: UploadBegin,
        now: SystemTime,
    ) -> Result<UploadBegan, UploadError> {
        self.store.maintain(now)?;
        if self.closed_submissions.len() >= MAX_CLOSED_SUBMISSIONS {
            return Err(UploadError::new(
                UploadErrorKind::Capacity,
                "this connection has reached its completed submission limit",
            ));
        }
        if self
            .submission
            .as_ref()
            .is_some_and(|group| group.submission_id != request.submission_id)
        {
            return Err(UploadError::new(
                UploadErrorKind::Busy,
                "this connection already has an active submission",
            ));
        }
        if let Err(error) = validate_begin(&request) {
            if self.submission.is_some() {
                self.abort_submission(&request.submission_id);
            }
            return Err(error);
        }
        if self.closed_submissions.contains(&request.submission_id) {
            return Err(UploadError::new(
                UploadErrorKind::ClosedSubmission,
                "submission id is already closed",
            ));
        }

        let prospective = match self.submission.as_ref() {
            Some(group) => {
                if !group.matches(&request) {
                    let submission_id = request.submission_id.clone();
                    self.abort_submission(&submission_id);
                    return Err(UploadError::new(
                        UploadErrorKind::InvalidSubmission,
                        "submission metadata changed between images",
                    ));
                }
                let next_count = group.begun_count + 1;
                let next_bytes =
                    group
                        .begun_bytes
                        .checked_add(request.length)
                        .ok_or_else(|| {
                            UploadError::new(
                                UploadErrorKind::TooLarge,
                                "submission byte count overflowed",
                            )
                        })?;
                if next_count > group.declared_count as usize
                    || next_bytes > group.declared_bytes
                    || (next_count == group.declared_count as usize
                        && next_bytes != group.declared_bytes)
                {
                    let submission_id = request.submission_id.clone();
                    self.abort_submission(&submission_id);
                    return Err(UploadError::new(
                        UploadErrorKind::InvalidSubmission,
                        "image declarations do not match the submission total",
                    ));
                }
                (next_count, next_bytes)
            }
            None => {
                if request.submission_count == 1 && request.length != request.submission_bytes {
                    return Err(UploadError::new(
                        UploadErrorKind::InvalidSubmission,
                        "image declaration does not match the submission total",
                    ));
                }
                (1, request.length)
            }
        };

        let staged = match self.store.stage(tab_cwd, request.length, now) {
            Ok(staged) => staged,
            Err(error) => {
                if self.submission.is_some() {
                    self.abort_submission(&request.submission_id);
                }
                return Err(error);
            }
        };
        let upload_id = Uuid::new_v4().hyphenated().to_string();
        let active = ActiveUpload {
            tab_id: request.tab_id.clone(),
            attachment_id: request.attachment_id.clone(),
            submission_id: request.submission_id.clone(),
            declared_length: request.length,
            declared_digest: request.sha256,
            next_chunk: 0,
            written: 0,
            hasher: Sha256::new(),
            staged,
        };

        let group = self
            .submission
            .get_or_insert_with(|| SubmissionState::new(&request));
        group.begun_count = prospective.0;
        group.begun_bytes = prospective.1;
        group.active_uploads.insert(upload_id.clone());
        self.upload_members.insert(
            upload_id.clone(),
            UploadMember {
                submission_id: request.submission_id,
                tab_id: request.tab_id,
                attachment_id: request.attachment_id,
            },
        );
        self.uploads.insert(upload_id.clone(), active);

        Ok(UploadBegan {
            upload_id,
            next_chunk: 0,
        })
    }

    pub fn chunk(&mut self, upload_id: &str, index: u32, data: &[u8]) -> Result<u32, UploadError> {
        let submission_id = self
            .uploads
            .get(upload_id)
            .ok_or_else(upload_not_found)?
            .submission_id
            .clone();

        let result = (|| {
            let upload = self
                .uploads
                .get_mut(upload_id)
                .expect("upload checked above");
            if index != upload.next_chunk {
                return Err(UploadError::new(
                    UploadErrorKind::OutOfOrder,
                    format!(
                        "expected upload chunk {}, received {index}",
                        upload.next_chunk
                    ),
                ));
            }
            if data.is_empty() {
                return Err(UploadError::new(
                    UploadErrorKind::LengthMismatch,
                    "upload chunks must not be empty",
                ));
            }
            if data.len() > MAX_UPLOAD_CHUNK_BYTES {
                return Err(UploadError::new(
                    UploadErrorKind::TooLarge,
                    "upload chunk exceeds 256 KiB",
                ));
            }
            let next_length = upload
                .written
                .checked_add(data.len() as u64)
                .ok_or_else(|| {
                    UploadError::new(UploadErrorKind::TooLarge, "upload length overflowed")
                })?;
            if next_length > upload.declared_length {
                return Err(UploadError::new(
                    UploadErrorKind::LengthMismatch,
                    "upload contains more bytes than declared",
                ));
            }
            upload
                .staged
                .file
                .write_all(data)
                .map_err(|error| UploadError::storage("write upload chunk", error))?;
            upload.hasher.update(data);
            upload.written = next_length;
            upload.next_chunk = upload.next_chunk.checked_add(1).ok_or_else(|| {
                UploadError::new(UploadErrorKind::TooLarge, "upload chunk index overflowed")
            })?;
            Ok(upload.next_chunk)
        })();

        if result.is_err() {
            self.abort_submission(&submission_id);
        }
        result
    }

    pub fn finish(&mut self, upload_id: &str) -> Result<PublishedUpload, UploadError> {
        self.finish_cancellable_at(upload_id, SystemTime::now(), &NEVER_CANCELLED)
    }

    pub fn finish_at(
        &mut self,
        upload_id: &str,
        now: SystemTime,
    ) -> Result<PublishedUpload, UploadError> {
        self.finish_cancellable_at(upload_id, now, &NEVER_CANCELLED)
    }

    /// Complete an upload unless its owning connection has been cancelled.
    ///
    /// The cancellation predicate is checked through the exact-inode
    /// publication transaction. A caller that signals it and waits for this
    /// method to return is guaranteed that an in-flight completion did not
    /// leave a newly published attachment behind.
    pub fn finish_cancellable(
        &mut self,
        upload_id: &str,
        cancelled: &AtomicBool,
    ) -> Result<PublishedUpload, UploadError> {
        self.finish_cancellable_at(upload_id, SystemTime::now(), cancelled)
    }

    pub fn finish_cancellable_at(
        &mut self,
        upload_id: &str,
        now: SystemTime,
        cancelled: &AtomicBool,
    ) -> Result<PublishedUpload, UploadError> {
        let mut upload = self
            .uploads
            .remove(upload_id)
            .ok_or_else(upload_not_found)?;
        if let Some(group) = self.submission.as_mut() {
            group.active_uploads.remove(upload_id);
        }
        let submission_id = upload.submission_id.clone();

        let result = finish_upload_cancellable(&mut upload, now, cancelled).and_then(|published| {
            if cancelled.load(Ordering::Acquire) {
                upload.staged.remove_published()?;
                return Err(upload_cancelled());
            }
            if let Err(error) = self.store.maintain(now) {
                let _ = upload.staged.remove_published();
                return Err(error);
            }
            if cancelled.load(Ordering::Acquire) {
                upload.staged.remove_published()?;
                return Err(upload_cancelled());
            }
            Ok(published)
        });
        match result {
            Ok(published) => {
                let complete = if let Some(group) = self.submission.as_mut() {
                    group.finished_count += 1;
                    group.finished_count == group.declared_count as usize
                        && group.begun_count == group.declared_count as usize
                } else {
                    false
                };
                if complete {
                    self.close_submission(submission_id);
                }
                Ok(published)
            }
            Err(error) => {
                self.abort_submission(&submission_id);
                Err(error)
            }
        }
    }

    /// Cancel one upload and the rest of its submission. Finished member ids
    /// remain valid cancellation handles for the bounded connection lifetime,
    /// and repeated cancellation is an idempotent success. A partial
    /// submission cannot be resumed with the same id after any member is
    /// cancelled.
    pub fn cancel(&mut self, upload_id: &str) -> Result<(), UploadError> {
        let submission_id = self
            .upload_members
            .get(upload_id)
            .ok_or_else(upload_not_found)?
            .submission_id
            .clone();
        if self
            .submission
            .as_ref()
            .is_some_and(|group| group.submission_id == submission_id)
        {
            self.abort_submission(&submission_id);
            return Ok(());
        }
        if self.closed_submissions.contains(&submission_id) {
            return Ok(());
        }
        Err(upload_not_found())
    }

    pub fn cancel_all(&mut self) {
        if let Some(submission_id) = self
            .submission
            .as_ref()
            .map(|group| group.submission_id.clone())
        {
            self.abort_submission(&submission_id);
        }
    }

    pub fn target(&self, upload_id: &str) -> Option<(&TabId, &AttachmentId)> {
        self.uploads
            .get(upload_id)
            .map(|upload| (&upload.tab_id, &upload.attachment_id))
    }

    pub fn cancel_target(&self, upload_id: &str) -> Option<(&TabId, &AttachmentId)> {
        self.upload_members
            .get(upload_id)
            .map(|upload| (&upload.tab_id, &upload.attachment_id))
    }

    fn abort_submission(&mut self, submission_id: &str) {
        if let Some(group) = self.submission.take() {
            if group.submission_id != submission_id {
                self.submission = Some(group);
                return;
            }
            for upload_id in group.active_uploads {
                self.uploads.remove(&upload_id);
            }
        }
        self.close_submission(submission_id.to_string());
    }

    fn close_submission(&mut self, submission_id: String) {
        self.submission = None;
        if self.closed_submissions.len() < MAX_CLOSED_SUBMISSIONS {
            self.closed_submissions.insert(submission_id);
        }
    }
}

impl Drop for UploadSet {
    fn drop(&mut self) {
        self.cancel_all();
    }
}

struct ActiveUpload {
    tab_id: TabId,
    attachment_id: AttachmentId,
    submission_id: String,
    declared_length: u64,
    declared_digest: [u8; 32],
    next_chunk: u32,
    written: u64,
    hasher: Sha256,
    staged: StagedFile,
}

struct UploadMember {
    submission_id: String,
    tab_id: TabId,
    attachment_id: AttachmentId,
}

struct StagedFile {
    directory: StableDirectory,
    manifest: Arc<ManifestStorage>,
    record_id: String,
    part_name: OsString,
    published_name: OsString,
    file: File,
    published: bool,
    manifested: bool,
}

impl StagedFile {
    fn verify_bound_entry(&self) -> Result<(), UploadError> {
        self.directory.verify_current_path()?;
        self.directory
            .verify_entry_identity(&self.part_name, &self.file)
    }

    fn read_contents(&self, declared_length: u64) -> Result<Vec<u8>, UploadError> {
        read_exact_file(&self.file, declared_length)
    }

    #[cfg(target_os = "linux")]
    fn publication_snapshot(&self, contents: &[u8]) -> Result<File, UploadError> {
        let mut file = self
            .directory
            .create_anonymous_file()
            .map_err(|error| path_operation_error("create anonymous publication inode", error))?;
        file.write_all(contents)
            .map_err(|error| UploadError::storage("write publication inode", error))?;
        file.flush()
            .map_err(|error| UploadError::storage("flush publication inode", error))?;
        file.sync_all()
            .map_err(|error| UploadError::storage("sync publication inode", error))?;
        Ok(file)
    }

    #[cfg(not(target_os = "linux"))]
    fn publication_snapshot(&self, _contents: &[u8]) -> Result<File, UploadError> {
        Err(UploadError::new(
            UploadErrorKind::Storage,
            "anonymous held-inode publication is only supported on Linux",
        ))
    }

    fn remove_published(&mut self) -> Result<(), UploadError> {
        if !self.published {
            return Ok(());
        }
        let manifest = self.manifest.clone();
        manifest.remove_published(self)
    }
}

fn read_exact_file(file: &File, declared_length: u64) -> Result<Vec<u8>, UploadError> {
    let metadata_length = file
        .metadata()
        .map_err(|error| UploadError::storage("inspect held attachment", error))?
        .len();
    if metadata_length != declared_length {
        return Err(UploadError::new(
            UploadErrorKind::LengthMismatch,
            format!("declared {declared_length} bytes but staged file contains {metadata_length}"),
        ));
    }
    let mut file = file
        .try_clone()
        .map_err(|error| UploadError::storage("clone held attachment", error))?;
    file.seek(SeekFrom::Start(0))
        .map_err(|error| UploadError::storage("rewind held attachment", error))?;
    let capacity = usize::try_from(declared_length).map_err(|_| {
        UploadError::new(
            UploadErrorKind::TooLarge,
            "held attachment cannot fit in memory on this platform",
        )
    })?;
    let mut contents = Vec::with_capacity(capacity);
    file.take(declared_length.saturating_add(1))
        .read_to_end(&mut contents)
        .map_err(|error| UploadError::storage("read held attachment", error))?;
    if contents.len() != capacity {
        return Err(UploadError::new(
            UploadErrorKind::LengthMismatch,
            format!(
                "declared {declared_length} bytes but read {} held bytes",
                contents.len()
            ),
        ));
    }
    Ok(contents)
}

impl Drop for StagedFile {
    fn drop(&mut self) {
        if !self.published {
            let remove = if self.manifested {
                self.manifest.cancel_staged(self)
            } else {
                self.directory
                    .verify_entry_identity(&self.part_name, &self.file)
                    .and_then(|()| self.directory.unlink(&self.part_name))
            };
            if let Err(error) = remove {
                tracing::warn!(
                    path = %self.directory.path.join(&self.part_name).display(),
                    error = %error,
                    "staged attachment entry was absent or replaced during cleanup"
                );
            }
        }
    }
}

struct SubmissionState {
    submission_id: String,
    tab_id: TabId,
    attachment_id: AttachmentId,
    declared_count: u8,
    declared_bytes: u64,
    begun_count: usize,
    begun_bytes: u64,
    finished_count: usize,
    active_uploads: HashSet<String>,
}

impl SubmissionState {
    fn new(request: &UploadBegin) -> Self {
        Self {
            submission_id: request.submission_id.clone(),
            tab_id: request.tab_id.clone(),
            attachment_id: request.attachment_id.clone(),
            declared_count: request.submission_count,
            declared_bytes: request.submission_bytes,
            begun_count: 0,
            begun_bytes: 0,
            finished_count: 0,
            active_uploads: HashSet::new(),
        }
    }

    fn matches(&self, request: &UploadBegin) -> bool {
        self.tab_id == request.tab_id
            && self.attachment_id == request.attachment_id
            && self.declared_count == request.submission_count
            && self.declared_bytes == request.submission_bytes
    }
}

fn validate_begin(request: &UploadBegin) -> Result<(), UploadError> {
    if request.submission_id.is_empty() || request.submission_id.len() > 128 {
        return Err(UploadError::new(
            UploadErrorKind::InvalidSubmission,
            "submission id is empty or too long",
        ));
    }
    if request.submission_count == 0 || request.submission_bytes == 0 || request.length == 0 {
        return Err(UploadError::new(
            UploadErrorKind::InvalidSubmission,
            "submission count and byte declarations must be nonzero",
        ));
    }
    if request.submission_count as usize > MAX_UPLOADS_PER_SUBMISSION {
        return Err(UploadError::new(
            UploadErrorKind::TooLarge,
            "submission must contain one to four images",
        ));
    }
    if request.submission_bytes > MAX_SUBMISSION_BYTES {
        return Err(UploadError::new(
            UploadErrorKind::TooLarge,
            "submission exceeds the 48 MiB limit",
        ));
    }
    if request.length > MAX_UPLOAD_BYTES {
        return Err(UploadError::new(
            UploadErrorKind::TooLarge,
            "image exceeds the 12 MiB limit",
        ));
    }
    if request.length > request.submission_bytes {
        return Err(UploadError::new(
            UploadErrorKind::InvalidSubmission,
            "image length exceeds its submission total",
        ));
    }
    Ok(())
}

fn finish_upload_cancellable(
    upload: &mut ActiveUpload,
    now: SystemTime,
    cancelled: &AtomicBool,
) -> Result<PublishedUpload, UploadError> {
    finish_upload_with_hook_cancellable(upload, now, cancelled, |_| Ok(()))
}

#[cfg(test)]
fn finish_upload_with_hook(
    upload: &mut ActiveUpload,
    now: SystemTime,
    hook: impl FnMut(PublicationBoundary) -> Result<(), UploadError>,
) -> Result<PublishedUpload, UploadError> {
    finish_upload_with_hook_cancellable(upload, now, &NEVER_CANCELLED, hook)
}

fn finish_upload_with_hook_cancellable(
    upload: &mut ActiveUpload,
    now: SystemTime,
    cancelled: &AtomicBool,
    hook: impl FnMut(PublicationBoundary) -> Result<(), UploadError>,
) -> Result<PublishedUpload, UploadError> {
    finish_upload_with_hooks_cancellable(upload, now, cancelled, hook, |_| Ok(()))
}

#[cfg(test)]
fn finish_upload_with_hooks(
    upload: &mut ActiveUpload,
    now: SystemTime,
    hook: impl FnMut(PublicationBoundary) -> Result<(), UploadError>,
    faults: impl FnMut(StorageFaultPoint) -> Result<(), UploadError>,
) -> Result<PublishedUpload, UploadError> {
    finish_upload_with_hooks_cancellable(upload, now, &NEVER_CANCELLED, hook, faults)
}

fn finish_upload_with_hooks_cancellable(
    upload: &mut ActiveUpload,
    now: SystemTime,
    cancelled: &AtomicBool,
    hook: impl FnMut(PublicationBoundary) -> Result<(), UploadError>,
    faults: impl FnMut(StorageFaultPoint) -> Result<(), UploadError>,
) -> Result<PublishedUpload, UploadError> {
    ensure_not_cancelled(cancelled)?;
    upload
        .staged
        .file
        .flush()
        .map_err(|error| UploadError::storage("flush staged attachment", error))?;
    upload
        .staged
        .file
        .sync_all()
        .map_err(|error| UploadError::storage("sync staged attachment", error))?;
    ensure_not_cancelled(cancelled)?;

    if upload.written != upload.declared_length {
        return Err(UploadError::new(
            UploadErrorKind::LengthMismatch,
            format!(
                "declared {} bytes but received {}",
                upload.declared_length, upload.written
            ),
        ));
    }
    upload.staged.verify_bound_entry()?;
    let contents = upload.staged.read_contents(upload.declared_length)?;
    let actual_digest: [u8; 32] = Sha256::digest(&contents).into();
    if actual_digest != upload.declared_digest {
        return Err(UploadError::new(
            UploadErrorKind::DigestMismatch,
            "attachment SHA-256 does not match its declaration",
        ));
    }
    let streamed_digest: [u8; 32] = upload.hasher.clone().finalize().into();
    if streamed_digest != actual_digest {
        return Err(UploadError::new(
            UploadErrorKind::DigestMismatch,
            "staged attachment changed after its chunks were received",
        ));
    }

    validate_strict_jpeg(&contents)?;
    ensure_not_cancelled(cancelled)?;
    let publication = upload.staged.publication_snapshot(&contents)?;
    let publication_contents = read_exact_file(&publication, upload.declared_length)?;
    let publication_digest: [u8; 32] = Sha256::digest(&publication_contents).into();
    if publication_digest != upload.declared_digest {
        return Err(UploadError::new(
            UploadErrorKind::DigestMismatch,
            "anonymous publication inode does not match the declared attachment",
        ));
    }
    validate_strict_jpeg(&publication_contents)?;
    ensure_not_cancelled(cancelled)?;

    let manifest = upload.staged.manifest.clone();
    manifest.publish_staged_with_hooks(
        &mut upload.staged,
        &publication,
        now,
        cancelled,
        hook,
        faults,
    )?;
    let path = upload
        .staged
        .directory
        .path
        .join(&upload.staged.published_name);
    Ok(PublishedUpload { path })
}

fn upload_cancelled() -> UploadError {
    UploadError::new(
        UploadErrorKind::Cancelled,
        "upload completion was cancelled before publication",
    )
}

fn ensure_not_cancelled(cancelled: &AtomicBool) -> Result<(), UploadError> {
    (!cancelled.load(Ordering::Acquire))
        .then_some(())
        .ok_or_else(upload_cancelled)
}

fn validate_strict_jpeg(contents: &[u8]) -> Result<(), UploadError> {
    let strict_dimensions = validate_complete_baseline_jpeg(contents)?;
    let mut cursor = Cursor::new(contents);
    let options = DecoderOptions::default()
        .set_strict_mode(true)
        .set_max_width(MAX_IMAGE_EDGE as usize)
        .set_max_height(MAX_IMAGE_EDGE as usize);
    let info = {
        let mut decoder = JpegDecoder::new_with_options(&mut cursor, options);
        decoder.decode().map_err(|_| {
            UploadError::new(
                UploadErrorKind::InvalidImage,
                "attachment is not a complete strict JPEG image",
            )
        })?;
        decoder.info().ok_or_else(|| {
            UploadError::new(
                UploadErrorKind::InvalidImage,
                "attachment JPEG has no image metadata",
            )
        })?
    };
    if cursor.position() != contents.len() as u64 {
        return Err(UploadError::new(
            UploadErrorKind::InvalidImage,
            "attachment contains data after its JPEG end marker",
        ));
    }
    let width = u32::from(info.width);
    let height = u32::from(info.height);
    if (width, height) != strict_dimensions {
        return Err(UploadError::new(
            UploadErrorKind::InvalidImage,
            "JPEG decoders disagree about image dimensions",
        ));
    }
    if width == 0 || height == 0 || width > MAX_IMAGE_EDGE || height > MAX_IMAGE_EDGE {
        return Err(UploadError::new(
            UploadErrorKind::InvalidImage,
            "JPEG dimensions must be between 1 and 4096 pixels",
        ));
    }
    Ok(())
}

#[derive(Clone)]
struct BaselineComponent {
    id: u8,
    horizontal_sampling: u8,
    vertical_sampling: u8,
}

#[derive(Clone, Default)]
struct StrictHuffmanTable {
    counts: [u8; 16],
    symbols: Vec<u8>,
}

impl StrictHuffmanTable {
    fn decode(&self, reader: &mut EntropyReader<'_>) -> Result<u8, UploadError> {
        let mut code = 0_u32;
        let mut first_code = 0_u32;
        let mut symbol_offset = 0_usize;
        for (index, count) in self.counts.iter().copied().enumerate() {
            code = (code << 1) | u32::from(reader.bit()?);
            let count = u32::from(count);
            if code >= first_code && code < first_code + count {
                return self
                    .symbols
                    .get(symbol_offset + (code - first_code) as usize)
                    .copied()
                    .ok_or_else(invalid_jpeg);
            }
            symbol_offset += count as usize;
            first_code = (first_code + count) << 1;
            if first_code > (1_u32 << (index + 2)) {
                return Err(invalid_jpeg());
            }
        }
        Err(invalid_jpeg())
    }
}

struct EntropyReader<'a> {
    bytes: &'a [u8],
    position: usize,
    current: u8,
    bits_left: u8,
}

impl<'a> EntropyReader<'a> {
    fn new(bytes: &'a [u8], position: usize) -> Self {
        Self {
            bytes,
            position,
            current: 0,
            bits_left: 0,
        }
    }

    fn bit(&mut self) -> Result<u8, UploadError> {
        if self.bits_left == 0 {
            self.current = *self.bytes.get(self.position).ok_or_else(invalid_jpeg)?;
            self.position += 1;
            if self.current == 0xff {
                let stuffed = *self.bytes.get(self.position).ok_or_else(invalid_jpeg)?;
                if stuffed != 0x00 {
                    return Err(invalid_jpeg());
                }
                self.position += 1;
            }
            self.bits_left = 8;
        }
        self.bits_left -= 1;
        Ok((self.current >> self.bits_left) & 1)
    }

    fn skip_bits(&mut self, count: u8) -> Result<(), UploadError> {
        for _ in 0..count {
            self.bit()?;
        }
        Ok(())
    }

    fn align_with_ones(&mut self) -> Result<(), UploadError> {
        if self.bits_left > 0 {
            let mask = (1_u16 << self.bits_left) - 1;
            if u16::from(self.current) & mask != mask {
                return Err(invalid_jpeg());
            }
            self.bits_left = 0;
        }
        Ok(())
    }

    fn marker(&mut self) -> Result<u8, UploadError> {
        self.align_with_ones()?;
        if self.bytes.get(self.position) != Some(&0xff) {
            return Err(invalid_jpeg());
        }
        while self.bytes.get(self.position) == Some(&0xff) {
            self.position += 1;
        }
        let marker = *self.bytes.get(self.position).ok_or_else(invalid_jpeg)?;
        self.position += 1;
        if marker == 0x00 {
            return Err(invalid_jpeg());
        }
        Ok(marker)
    }
}

fn validate_complete_baseline_jpeg(contents: &[u8]) -> Result<(u32, u32), UploadError> {
    if !contents.starts_with(&[0xff, 0xd8]) {
        return Err(invalid_jpeg());
    }
    let mut position = 2_usize;
    let mut dimensions = None;
    let mut components = Vec::<BaselineComponent>::new();
    let mut dc_tables: [Option<StrictHuffmanTable>; 4] = Default::default();
    let mut ac_tables: [Option<StrictHuffmanTable>; 4] = Default::default();
    let mut restart_interval = 0_u16;

    loop {
        let marker = read_jpeg_marker(contents, &mut position)?;
        if marker == 0xd9 {
            return Err(invalid_jpeg());
        }
        if matches!(marker, 0xd0..=0xd7 | 0x01) {
            return Err(invalid_jpeg());
        }
        let segment = read_jpeg_segment(contents, &mut position)?;
        match marker {
            0xc0 => {
                let (parsed_dimensions, parsed_components) = parse_baseline_frame(segment)?;
                if dimensions.replace(parsed_dimensions).is_some() {
                    return Err(invalid_jpeg());
                }
                components = parsed_components;
            }
            0xc1..=0xcf if !matches!(marker, 0xc4 | 0xc8 | 0xcc) => return Err(invalid_jpeg()),
            0xc4 => parse_huffman_tables(segment, &mut dc_tables, &mut ac_tables)?,
            0xdd => {
                if segment.len() != 2 {
                    return Err(invalid_jpeg());
                }
                restart_interval = u16::from_be_bytes([segment[0], segment[1]]);
            }
            0xda => {
                let dimensions = dimensions.ok_or_else(invalid_jpeg)?;
                let scan_components = parse_baseline_scan(segment, &components)?;
                validate_baseline_entropy(
                    contents,
                    position,
                    dimensions,
                    &components,
                    &scan_components,
                    &dc_tables,
                    &ac_tables,
                    restart_interval,
                )?;
                return Ok(dimensions);
            }
            0xdb | 0xe0..=0xef | 0xfe => {}
            _ => return Err(invalid_jpeg()),
        }
    }
}

fn read_jpeg_marker(contents: &[u8], position: &mut usize) -> Result<u8, UploadError> {
    if contents.get(*position) != Some(&0xff) {
        return Err(invalid_jpeg());
    }
    while contents.get(*position) == Some(&0xff) {
        *position += 1;
    }
    let marker = *contents.get(*position).ok_or_else(invalid_jpeg)?;
    *position += 1;
    if marker == 0x00 || marker == 0xff {
        return Err(invalid_jpeg());
    }
    Ok(marker)
}

fn read_jpeg_segment<'a>(
    contents: &'a [u8],
    position: &mut usize,
) -> Result<&'a [u8], UploadError> {
    let length_bytes = contents
        .get(*position..position.saturating_add(2))
        .ok_or_else(invalid_jpeg)?;
    let length = usize::from(u16::from_be_bytes([length_bytes[0], length_bytes[1]]));
    if length < 2 {
        return Err(invalid_jpeg());
    }
    let start = position.saturating_add(2);
    let end = position.saturating_add(length);
    let segment = contents.get(start..end).ok_or_else(invalid_jpeg)?;
    *position = end;
    Ok(segment)
}

fn parse_baseline_frame(
    segment: &[u8],
) -> Result<((u32, u32), Vec<BaselineComponent>), UploadError> {
    if segment.len() < 6 || segment[0] != 8 {
        return Err(invalid_jpeg());
    }
    let height = u32::from(u16::from_be_bytes([segment[1], segment[2]]));
    let width = u32::from(u16::from_be_bytes([segment[3], segment[4]]));
    let count = usize::from(segment[5]);
    if !(1..=MAX_IMAGE_EDGE).contains(&width)
        || !(1..=MAX_IMAGE_EDGE).contains(&height)
        || !matches!(count, 1 | 3)
        || segment.len() != 6 + count * 3
    {
        return Err(invalid_jpeg());
    }
    let mut components = Vec::with_capacity(count);
    for bytes in segment[6..].chunks_exact(3) {
        let horizontal_sampling = bytes[1] >> 4;
        let vertical_sampling = bytes[1] & 0x0f;
        if horizontal_sampling == 0
            || vertical_sampling == 0
            || horizontal_sampling > 4
            || vertical_sampling > 4
            || components
                .iter()
                .any(|component: &BaselineComponent| component.id == bytes[0])
        {
            return Err(invalid_jpeg());
        }
        components.push(BaselineComponent {
            id: bytes[0],
            horizontal_sampling,
            vertical_sampling,
        });
    }
    let aggregate_sampling: u32 = components
        .iter()
        .map(|component| {
            u32::from(component.horizontal_sampling) * u32::from(component.vertical_sampling)
        })
        .sum();
    if aggregate_sampling > 10 {
        return Err(invalid_jpeg());
    }
    Ok(((width, height), components))
}

fn parse_huffman_tables(
    segment: &[u8],
    dc_tables: &mut [Option<StrictHuffmanTable>; 4],
    ac_tables: &mut [Option<StrictHuffmanTable>; 4],
) -> Result<(), UploadError> {
    let mut position = 0_usize;
    while position < segment.len() {
        let selector = *segment.get(position).ok_or_else(invalid_jpeg)?;
        position += 1;
        let class = selector >> 4;
        let index = usize::from(selector & 0x0f);
        if class > 1 || index >= 4 {
            return Err(invalid_jpeg());
        }
        let counts_slice = segment
            .get(position..position.saturating_add(16))
            .ok_or_else(invalid_jpeg)?;
        position += 16;
        let mut counts = [0_u8; 16];
        counts.copy_from_slice(counts_slice);
        let mut available_codes = 1_i32;
        for count in counts {
            available_codes = available_codes * 2 - i32::from(count);
            if available_codes < 0 {
                return Err(invalid_jpeg());
            }
        }
        let symbol_count: usize = counts.iter().map(|count| usize::from(*count)).sum();
        if symbol_count == 0 || symbol_count > 256 {
            return Err(invalid_jpeg());
        }
        let symbols = segment
            .get(position..position.saturating_add(symbol_count))
            .ok_or_else(invalid_jpeg)?
            .to_vec();
        position += symbol_count;
        let table = StrictHuffmanTable { counts, symbols };
        if class == 0 {
            dc_tables[index] = Some(table);
        } else {
            ac_tables[index] = Some(table);
        }
    }
    Ok(())
}

fn parse_baseline_scan(
    segment: &[u8],
    frame_components: &[BaselineComponent],
) -> Result<Vec<(usize, usize)>, UploadError> {
    let count = usize::from(*segment.first().ok_or_else(invalid_jpeg)?);
    if count != frame_components.len() || segment.len() != 1 + count * 2 + 3 {
        return Err(invalid_jpeg());
    }
    let mut tables = vec![(usize::MAX, usize::MAX); frame_components.len()];
    for bytes in segment[1..1 + count * 2].chunks_exact(2) {
        let component = frame_components
            .iter()
            .position(|component| component.id == bytes[0])
            .ok_or_else(invalid_jpeg)?;
        if tables[component].0 != usize::MAX {
            return Err(invalid_jpeg());
        }
        let dc = usize::from(bytes[1] >> 4);
        let ac = usize::from(bytes[1] & 0x0f);
        if dc >= 4 || ac >= 4 {
            return Err(invalid_jpeg());
        }
        tables[component] = (dc, ac);
    }
    if segment[1 + count * 2..] != [0, 63, 0] {
        return Err(invalid_jpeg());
    }
    Ok(tables)
}

#[allow(clippy::too_many_arguments)]
fn validate_baseline_entropy(
    contents: &[u8],
    position: usize,
    dimensions: (u32, u32),
    components: &[BaselineComponent],
    scan_tables: &[(usize, usize)],
    dc_tables: &[Option<StrictHuffmanTable>; 4],
    ac_tables: &[Option<StrictHuffmanTable>; 4],
    restart_interval: u16,
) -> Result<(), UploadError> {
    let max_horizontal = u32::from(
        components
            .iter()
            .map(|component| component.horizontal_sampling)
            .max()
            .ok_or_else(invalid_jpeg)?,
    );
    let max_vertical = u32::from(
        components
            .iter()
            .map(|component| component.vertical_sampling)
            .max()
            .ok_or_else(invalid_jpeg)?,
    );
    let mcu_width = 8 * max_horizontal;
    let mcu_height = 8 * max_vertical;
    let across = dimensions.0.div_ceil(mcu_width);
    let down = dimensions.1.div_ceil(mcu_height);
    let total_mcus = across.checked_mul(down).ok_or_else(invalid_jpeg)?;
    let mut reader = EntropyReader::new(contents, position);
    let mut restart_number = 0_u8;

    for mcu in 0..total_mcus {
        for (component_index, component) in components.iter().enumerate() {
            let (dc_index, ac_index) = scan_tables[component_index];
            let dc = dc_tables[dc_index].as_ref().ok_or_else(invalid_jpeg)?;
            let ac = ac_tables[ac_index].as_ref().ok_or_else(invalid_jpeg)?;
            let blocks =
                u32::from(component.horizontal_sampling) * u32::from(component.vertical_sampling);
            for _ in 0..blocks {
                let dc_bits = dc.decode(&mut reader)?;
                if dc_bits > 11 {
                    return Err(invalid_jpeg());
                }
                reader.skip_bits(dc_bits)?;
                let mut coefficient = 1_u8;
                while coefficient < 64 {
                    let symbol = ac.decode(&mut reader)?;
                    let run = symbol >> 4;
                    let bits = symbol & 0x0f;
                    if bits == 0 {
                        if run == 0 {
                            break;
                        }
                        if run != 15 || coefficient > 48 {
                            return Err(invalid_jpeg());
                        }
                        coefficient += 16;
                    } else {
                        if bits > 10 {
                            return Err(invalid_jpeg());
                        }
                        coefficient = coefficient
                            .checked_add(run)
                            .filter(|value| *value < 64)
                            .ok_or_else(invalid_jpeg)?;
                        reader.skip_bits(bits)?;
                        coefficient += 1;
                    }
                }
            }
        }
        if restart_interval > 0
            && (mcu + 1) % u32::from(restart_interval) == 0
            && mcu + 1 < total_mcus
        {
            if reader.marker()? != 0xd0 + restart_number {
                return Err(invalid_jpeg());
            }
            restart_number = (restart_number + 1) % 8;
        }
    }

    if reader.marker()? != 0xd9 || reader.position != contents.len() {
        return Err(invalid_jpeg());
    }
    Ok(())
}

fn invalid_jpeg() -> UploadError {
    UploadError::new(
        UploadErrorKind::InvalidImage,
        "attachment is not a complete normalized baseline JPEG image",
    )
}

fn canonical_project_cwd(cwd: &Path) -> Result<PathBuf, UploadError> {
    let canonical = fs::canonicalize(cwd)
        .map_err(|error| UploadError::storage("canonicalize tab working directory", error))?;
    let metadata = fs::symlink_metadata(&canonical)
        .map_err(|error| UploadError::storage("inspect tab working directory", error))?;
    if !metadata.is_dir() {
        return Err(UploadError::new(
            UploadErrorKind::UnsafePath,
            "tab working directory is not a real directory",
        ));
    }
    Ok(canonical)
}

fn project_attachment_directory(canonical_cwd: &Path) -> Result<StableDirectory, UploadError> {
    let cwd = StableDirectory::open_existing_absolute(canonical_cwd)?;
    let aiterm = cwd.open_or_create_child(OsStr::new(".aiterm"), true)?;
    aiterm.open_or_create_child(OsStr::new("attachments"), true)
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum ManifestState {
    Partial,
    Complete,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StageBoundary {
    IntentPersisted,
    PartLinked,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PublicationBoundary {
    PartUnlinked,
    IntentPersisted,
    PublishedLinked,
    CompletePersisted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CleanupContext {
    PartialLinkRollback,
    FinalLinkRollback,
    CompletePersistRollback,
    Cancel,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StorageFaultPoint {
    PartUnlinkDirectorySync,
    PostLinkDirectorySync(CleanupContext),
    CompleteManifestPersist,
    CleanupUnlink(CleanupContext),
    CleanupDirectorySync(CleanupContext),
    CleanupManifestPersist(CleanupContext),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ManifestRecord {
    id: String,
    root: Vec<u8>,
    path: Vec<u8>,
    length: u64,
    created_unix_millis: u64,
    state: ManifestState,
    device: u64,
    inode: u64,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct AttachmentManifest {
    version: u8,
    records: Vec<ManifestRecord>,
}

impl Default for AttachmentManifest {
    fn default() -> Self {
        Self {
            version: MANIFEST_VERSION,
            records: Vec::new(),
        }
    }
}

#[derive(Debug)]
struct ManifestStorage {
    directory: StableDirectory,
    process_lock: Arc<Mutex<()>>,
}

impl ManifestStorage {
    fn new(directory: StableDirectory) -> Result<Self, UploadError> {
        let process_lock = manifest_lock_for(&directory.path);
        Ok(Self {
            directory,
            process_lock,
        })
    }

    fn maintain(&self, now: SystemTime) -> Result<(), UploadError> {
        self.with_manifest(|manifest| {
            let now = system_time_millis(now)?;
            let mut remove = Vec::new();
            for record in &manifest.records {
                let ttl = match record.state {
                    ManifestState::Partial => PARTIAL_ATTACHMENT_TTL,
                    ManifestState::Complete => ATTACHMENT_TTL,
                };
                let expired =
                    now.saturating_sub(record.created_unix_millis) > duration_millis(ttl)?;
                let exists = manifest_record_exists(record)?;
                if !exists {
                    // Publication replaces the manifest's `.part` inode with a
                    // held, validated `.jpg` inode before updating this record.
                    // Keep that short-lived missing partial binding until its
                    // finish transaction or normal expiry resolves it.
                    if record.state == ManifestState::Complete || expired {
                        remove.push(record.id.clone());
                    }
                    continue;
                }
                if expired {
                    delete_manifest_file(record)?;
                    remove.push(record.id.clone());
                }
            }
            manifest
                .records
                .retain(|record| !remove.iter().any(|id| id == &record.id));

            let mut total = manifest_total(manifest)?;
            if total > ATTACHMENT_BUDGET_BYTES {
                let mut candidates: Vec<_> = manifest
                    .records
                    .iter()
                    .filter(|record| record.state == ManifestState::Complete)
                    .map(|record| (record.created_unix_millis, record.id.clone()))
                    .collect();
                candidates.sort();
                for (_, id) in candidates {
                    if total <= ATTACHMENT_BUDGET_BYTES {
                        break;
                    }
                    if let Some(index) = manifest.records.iter().position(|record| record.id == id)
                    {
                        let record = &manifest.records[index];
                        delete_manifest_file(record)?;
                        total = total.saturating_sub(record.length);
                        manifest.records.remove(index);
                    }
                }
            }
            if total > ATTACHMENT_BUDGET_BYTES {
                let mut partials: Vec<_> = manifest
                    .records
                    .iter()
                    .filter(|record| record.state == ManifestState::Partial)
                    .map(|record| (record.created_unix_millis, record.id.clone()))
                    .collect();
                partials.sort();
                for (_, id) in partials {
                    if total <= ATTACHMENT_BUDGET_BYTES {
                        break;
                    }
                    if let Some(index) = manifest.records.iter().position(|record| record.id == id)
                    {
                        let record = &manifest.records[index];
                        delete_manifest_file(record)?;
                        total = total.saturating_sub(record.length);
                        manifest.records.remove(index);
                    }
                }
            }
            Ok(())
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn record_partial_and_link(
        &self,
        id: &str,
        root: &Path,
        name: &OsStr,
        length: u64,
        now: SystemTime,
        identity: FileIdentity,
        staged_directory: &StableDirectory,
        staged_file: &File,
    ) -> Result<(), UploadError> {
        self.record_partial_and_link_with_hook(
            id,
            root,
            name,
            length,
            now,
            identity,
            staged_directory,
            staged_file,
            |_| Ok(()),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn record_partial_and_link_with_hook(
        &self,
        id: &str,
        root: &Path,
        name: &OsStr,
        length: u64,
        now: SystemTime,
        identity: FileIdentity,
        staged_directory: &StableDirectory,
        staged_file: &File,
        hook: impl FnMut(StageBoundary) -> Result<(), UploadError>,
    ) -> Result<(), UploadError> {
        self.record_partial_and_link_with_hooks(
            id,
            root,
            name,
            length,
            now,
            identity,
            staged_directory,
            staged_file,
            hook,
            |_| Ok(()),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn record_partial_and_link_with_hooks(
        &self,
        id: &str,
        root: &Path,
        name: &OsStr,
        length: u64,
        now: SystemTime,
        identity: FileIdentity,
        staged_directory: &StableDirectory,
        staged_file: &File,
        mut hook: impl FnMut(StageBoundary) -> Result<(), UploadError>,
        mut faults: impl FnMut(StorageFaultPoint) -> Result<(), UploadError>,
    ) -> Result<(), UploadError> {
        let _process_guard = self
            .process_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _file_guard = self.directory.lock_manifest()?;
        self.directory.verify_current_path()?;
        let (mut manifest, mut expected) = self.load_or_rebuild()?;
        insert_partial_record(&mut manifest, id, root, name, length, now, identity)?;
        self.persist_manifest(&manifest, &mut expected)?;
        hook(StageBoundary::IntentPersisted)?;

        staged_directory.verify_current_path()?;
        let link_result = staged_directory.link_held_noreplace(staged_file, name);
        if let Err(error) = link_result {
            manifest.records.retain(|record| record.id != id);
            self.persist_manifest(&manifest, &mut expected)?;
            return Err(error);
        }
        let linked = staged_directory
            .verify_entry_identity(name, staged_file)
            .and_then(|()| {
                faults(StorageFaultPoint::PostLinkDirectorySync(
                    CleanupContext::PartialLinkRollback,
                ))
            })
            .and_then(|()| staged_directory.sync());
        if let Err(error) = linked {
            cleanup_owned_entry_with_faults(
                staged_directory,
                name,
                staged_file,
                CleanupContext::PartialLinkRollback,
                &mut faults,
            )?;
            self.remove_record_after_cleanup(
                &mut manifest,
                id,
                &mut expected,
                CleanupContext::PartialLinkRollback,
                &mut faults,
            )?;
            return Err(error);
        }
        hook(StageBoundary::PartLinked)
    }

    fn publish_staged_with_hooks(
        &self,
        staged: &mut StagedFile,
        validated: &File,
        now: SystemTime,
        cancelled: &AtomicBool,
        mut hook: impl FnMut(PublicationBoundary) -> Result<(), UploadError>,
        mut faults: impl FnMut(StorageFaultPoint) -> Result<(), UploadError>,
    ) -> Result<(), UploadError> {
        ensure_not_cancelled(cancelled)?;
        let held_publication = validated
            .try_clone()
            .map_err(|error| UploadError::storage("hold published attachment", error))?;
        let publication_identity = FileIdentity::of(validated)?;
        let _process_guard = self
            .process_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _file_guard = self.directory.lock_manifest()?;
        self.directory.verify_current_path()?;
        let (mut manifest, mut expected) = self.load_or_rebuild()?;
        let record_index = manifest
            .records
            .iter()
            .position(|record| record.id == staged.record_id)
            .ok_or_else(|| {
                UploadError::new(
                    UploadErrorKind::Storage,
                    "staged attachment is missing from the manifest",
                )
            })?;
        let record = &manifest.records[record_index];
        if record.state != ManifestState::Partial
            || path_from_bytes(&record.root) != staged.directory.path
            || path_from_bytes(&record.path) != staged.directory.path.join(&staged.part_name)
            || record.device != FileIdentity::of(&staged.file)?.device
            || record.inode != FileIdentity::of(&staged.file)?.inode
        {
            return Err(UploadError::new(
                UploadErrorKind::UnsafePath,
                "staged attachment manifest binding changed",
            ));
        }

        staged
            .directory
            .require_entry_absent(&staged.published_name)?;
        staged.verify_bound_entry()?;
        ensure_not_cancelled(cancelled)?;
        staged.directory.unlink(&staged.part_name)?;
        let part_unlink_sync = faults(StorageFaultPoint::PartUnlinkDirectorySync)
            .and_then(|()| staged.directory.sync());
        if let Err(error) = part_unlink_sync {
            // The unlink is not durable until the containing directory syncs.
            // Leave Drop disarmed so the partial intent can cover a name that
            // may reappear after a crash and maintenance can retry safely.
            staged.published = true;
            return Err(error);
        }
        hook(PublicationBoundary::PartUnlinked)?;

        let record = &mut manifest.records[record_index];
        record.path = path_bytes(&staged.directory.path.join(&staged.published_name));
        record.device = publication_identity.device;
        record.inode = publication_identity.inode;
        self.persist_manifest(&manifest, &mut expected)?;
        hook(PublicationBoundary::IntentPersisted)?;
        if cancelled.load(Ordering::Acquire) {
            return self.rollback_cancelled_unpublished_intent(
                staged,
                record_index,
                &mut manifest,
                &mut expected,
            );
        }

        staged.directory.verify_current_path()?;
        ensure_not_cancelled(cancelled)?;
        if let Err(error) = staged
            .directory
            .link_held_noreplace(validated, &staged.published_name)
        {
            manifest.records.remove(record_index);
            self.persist_manifest(&manifest, &mut expected)?;
            return Err(error);
        }
        let linked = staged
            .directory
            .verify_entry_identity(&staged.published_name, validated)
            .and_then(|()| {
                faults(StorageFaultPoint::PostLinkDirectorySync(
                    CleanupContext::FinalLinkRollback,
                ))
            })
            .and_then(|()| staged.directory.sync());
        if let Err(error) = linked {
            if let Err(rollback_error) = cleanup_owned_entry_with_faults(
                &staged.directory,
                &staged.published_name,
                validated,
                CleanupContext::FinalLinkRollback,
                &mut faults,
            ) {
                staged.published = true;
                return Err(rollback_error);
            }
            if let Err(rollback_error) = self.remove_record_after_cleanup(
                &mut manifest,
                &staged.record_id,
                &mut expected,
                CleanupContext::FinalLinkRollback,
                &mut faults,
            ) {
                staged.published = true;
                return Err(rollback_error);
            }
            return Err(error);
        }
        hook(PublicationBoundary::PublishedLinked)?;
        if cancelled.load(Ordering::Acquire) {
            return self.rollback_cancelled_publication(
                staged,
                validated,
                &mut manifest,
                &mut expected,
            );
        }

        manifest.records[record_index].state = ManifestState::Complete;
        manifest.records[record_index].created_unix_millis = system_time_millis(now)?;
        let complete_persist = faults(StorageFaultPoint::CompleteManifestPersist)
            .and_then(|()| self.persist_manifest(&manifest, &mut expected));
        if let Err(error) = complete_persist {
            if let Err(rollback_error) = cleanup_owned_entry_with_faults(
                &staged.directory,
                &staged.published_name,
                validated,
                CleanupContext::CompletePersistRollback,
                &mut faults,
            ) {
                staged.published = true;
                return Err(rollback_error);
            }
            if let Err(rollback_error) = self.remove_record_after_cleanup(
                &mut manifest,
                &staged.record_id,
                &mut expected,
                CleanupContext::CompletePersistRollback,
                &mut faults,
            ) {
                staged.published = true;
                return Err(rollback_error);
            }
            return Err(error);
        }
        hook(PublicationBoundary::CompletePersisted)?;
        if cancelled.load(Ordering::Acquire) {
            return self.rollback_cancelled_publication(
                staged,
                validated,
                &mut manifest,
                &mut expected,
            );
        }

        staged.file = held_publication;
        staged.published = true;
        Ok(())
    }

    fn rollback_cancelled_publication(
        &self,
        staged: &mut StagedFile,
        validated: &File,
        manifest: &mut AttachmentManifest,
        expected: &mut Option<FileIdentity>,
    ) -> Result<(), UploadError> {
        let mut no_faults = |_| Ok(());
        if let Err(error) = cleanup_owned_entry_with_faults(
            &staged.directory,
            &staged.published_name,
            validated,
            CleanupContext::FinalLinkRollback,
            &mut no_faults,
        ) {
            // The durable final intent remains for maintenance when an exact
            // cleanup cannot be completed. Drop must not mistake it for a
            // partial `.part` entry.
            staged.published = true;
            return Err(error);
        }
        if let Err(error) = self.remove_record_after_cleanup(
            manifest,
            &staged.record_id,
            expected,
            CleanupContext::FinalLinkRollback,
            &mut no_faults,
        ) {
            staged.published = true;
            return Err(error);
        }
        Err(upload_cancelled())
    }

    fn rollback_cancelled_unpublished_intent(
        &self,
        staged: &mut StagedFile,
        record_index: usize,
        manifest: &mut AttachmentManifest,
        expected: &mut Option<FileIdentity>,
    ) -> Result<(), UploadError> {
        manifest.records.remove(record_index);
        if let Err(error) = self.persist_manifest(manifest, expected) {
            // The final intent is deliberately retained for maintenance if it
            // cannot be durably removed. There is no linked final inode yet.
            staged.published = true;
            return Err(error);
        }
        staged.published = true;
        Err(upload_cancelled())
    }

    fn cancel_staged(&self, staged: &StagedFile) -> Result<(), UploadError> {
        self.cancel_staged_with_faults(staged, |_| Ok(()))
    }

    fn cancel_staged_with_faults(
        &self,
        staged: &StagedFile,
        mut faults: impl FnMut(StorageFaultPoint) -> Result<(), UploadError>,
    ) -> Result<(), UploadError> {
        let _process_guard = self
            .process_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _file_guard = self.directory.lock_manifest()?;
        self.directory.verify_current_path()?;
        let (mut manifest, mut expected) = self.load_or_rebuild()?;
        cleanup_owned_entry_with_faults(
            &staged.directory,
            &staged.part_name,
            &staged.file,
            CleanupContext::Cancel,
            &mut faults,
        )?;
        self.remove_record_after_cleanup(
            &mut manifest,
            &staged.record_id,
            &mut expected,
            CleanupContext::Cancel,
            &mut faults,
        )
    }

    fn remove_record_after_cleanup(
        &self,
        manifest: &mut AttachmentManifest,
        id: &str,
        expected: &mut Option<FileIdentity>,
        context: CleanupContext,
        faults: &mut impl FnMut(StorageFaultPoint) -> Result<(), UploadError>,
    ) -> Result<(), UploadError> {
        manifest.records.retain(|record| record.id != id);
        faults(StorageFaultPoint::CleanupManifestPersist(context))?;
        self.persist_manifest(manifest, expected)
    }

    fn remove_published(&self, staged: &mut StagedFile) -> Result<(), UploadError> {
        let _process_guard = self
            .process_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _file_guard = self.directory.lock_manifest()?;
        self.directory.verify_current_path()?;
        let (mut manifest, mut expected) = self.load_or_rebuild()?;
        staged
            .directory
            .verify_entry_identity(&staged.published_name, &staged.file)?;
        staged.directory.unlink(&staged.published_name)?;
        staged.directory.sync()?;
        manifest
            .records
            .retain(|record| record.id != staged.record_id);
        self.persist_manifest(&manifest, &mut expected)?;
        staged.published = false;
        Ok(())
    }

    fn with_manifest<T>(
        &self,
        operation: impl FnOnce(&mut AttachmentManifest) -> Result<T, UploadError>,
    ) -> Result<T, UploadError> {
        let _process_guard = self
            .process_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _file_guard = self.directory.lock_manifest()?;
        self.directory.verify_current_path()?;
        let (mut manifest, mut expected) = self.load_or_rebuild()?;
        let result = operation(&mut manifest)?;
        self.persist_manifest(&manifest, &mut expected)?;
        Ok(result)
    }

    fn persist_manifest(
        &self,
        manifest: &AttachmentManifest,
        expected: &mut Option<FileIdentity>,
    ) -> Result<(), UploadError> {
        validate_manifest(manifest, &self.directory.path)?;
        let mut bytes = serde_json::to_vec(&manifest)
            .map_err(|error| UploadError::storage("serialize attachment manifest", error))?;
        bytes.push(b'\n');
        if bytes.len() as u64 > MAX_MANIFEST_BYTES {
            return Err(UploadError::new(
                UploadErrorKind::Capacity,
                "attachment manifest exceeds its safety limit",
            ));
        }
        self.directory
            .write_atomic_replacing(OsStr::new(MANIFEST_NAME), &bytes, *expected)?;
        let manifest_file = self
            .directory
            .open_file(OsStr::new(MANIFEST_NAME))
            .map_err(|error| path_operation_error("reopen attachment manifest", error))?;
        *expected = Some(FileIdentity::of(&manifest_file)?);
        Ok(())
    }

    fn load_or_rebuild(&self) -> Result<(AttachmentManifest, Option<FileIdentity>), UploadError> {
        let loaded = self
            .directory
            .read_optional_file(OsStr::new(MANIFEST_NAME), MAX_MANIFEST_BYTES);
        match loaded {
            Ok((_, None)) => Ok((AttachmentManifest::default(), None)),
            Ok((bytes, Some(identity))) => {
                let parsed = serde_json::from_slice::<AttachmentManifest>(&bytes)
                    .map_err(|error| UploadError::storage("parse attachment manifest", error))
                    .and_then(|manifest| {
                        validate_manifest(&manifest, &self.directory.path)?;
                        Ok(manifest)
                    });
                match parsed {
                    Ok(manifest) => Ok((manifest, Some(identity))),
                    Err(error) => {
                        self.quarantine_manifest()?;
                        tracing::warn!(error = %error, "quarantined invalid attachment manifest");
                        Ok((AttachmentManifest::default(), None))
                    }
                }
            }
            Err(error) => {
                if matches!(
                    error.kind(),
                    UploadErrorKind::UnsafePath | UploadErrorKind::Capacity
                ) {
                    self.quarantine_manifest()?;
                    tracing::warn!(error = %error, "quarantined unsafe attachment manifest");
                    Ok((AttachmentManifest::default(), None))
                } else {
                    Err(error)
                }
            }
        }
    }

    fn quarantine_manifest(&self) -> Result<(), UploadError> {
        self.directory.verify_current_path()?;
        self.directory.rename_replacing(
            OsStr::new(MANIFEST_NAME),
            OsStr::new("attachments.corrupt.json"),
        )?;
        self.directory.sync()
    }
}

fn cleanup_owned_entry_with_faults(
    directory: &StableDirectory,
    name: &OsStr,
    held: &File,
    context: CleanupContext,
    faults: &mut impl FnMut(StorageFaultPoint) -> Result<(), UploadError>,
) -> Result<(), UploadError> {
    match directory.verify_entry_identity(name, held) {
        Ok(()) => {}
        Err(error)
            if matches!(
                error.kind(),
                UploadErrorKind::NotFound | UploadErrorKind::UnsafePath
            ) =>
        {
            return Ok(())
        }
        Err(error) => return Err(error),
    }

    faults(StorageFaultPoint::CleanupUnlink(context))?;
    match directory.unlink(name) {
        Ok(()) => {}
        Err(error) if error.kind() == UploadErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    }
    faults(StorageFaultPoint::CleanupDirectorySync(context))?;
    directory.sync()
}

fn manifest_total(manifest: &AttachmentManifest) -> Result<u64, UploadError> {
    manifest.records.iter().try_fold(0_u64, |total, record| {
        total.checked_add(record.length).ok_or_else(|| {
            UploadError::new(UploadErrorKind::Capacity, "attachment budget overflowed")
        })
    })
}

#[allow(clippy::too_many_arguments)]
fn insert_partial_record(
    manifest: &mut AttachmentManifest,
    id: &str,
    root: &Path,
    name: &OsStr,
    length: u64,
    now: SystemTime,
    identity: FileIdentity,
) -> Result<(), UploadError> {
    if manifest.records.len() >= MAX_MANIFEST_RECORDS {
        return Err(UploadError::new(
            UploadErrorKind::Capacity,
            "attachment manifest record limit reached",
        ));
    }
    if manifest.records.iter().any(|record| record.id == id) {
        return Err(UploadError::new(
            UploadErrorKind::Storage,
            "attachment id already exists in the manifest",
        ));
    }
    let partial_total = manifest
        .records
        .iter()
        .filter(|record| record.state == ManifestState::Partial)
        .try_fold(length, |total, record| total.checked_add(record.length))
        .ok_or_else(|| {
            UploadError::new(UploadErrorKind::Capacity, "attachment budget overflowed")
        })?;
    if partial_total > ATTACHMENT_BUDGET_BYTES {
        return Err(UploadError::new(
            UploadErrorKind::Capacity,
            "active attachments exhaust the global storage budget",
        ));
    }

    let mut total = manifest_total(manifest)?
        .checked_add(length)
        .ok_or_else(|| {
            UploadError::new(UploadErrorKind::Capacity, "attachment budget overflowed")
        })?;
    if total > ATTACHMENT_BUDGET_BYTES {
        let mut completed: Vec<_> = manifest
            .records
            .iter()
            .filter(|record| record.state == ManifestState::Complete)
            .map(|record| (record.created_unix_millis, record.id.clone()))
            .collect();
        completed.sort();
        for (_, completed_id) in completed {
            if total <= ATTACHMENT_BUDGET_BYTES {
                break;
            }
            if let Some(index) = manifest
                .records
                .iter()
                .position(|record| record.id == completed_id)
            {
                let record = &manifest.records[index];
                delete_manifest_file(record)?;
                total = total.saturating_sub(record.length);
                manifest.records.remove(index);
            }
        }
    }

    let path = root.join(name);
    manifest.records.push(ManifestRecord {
        id: id.to_string(),
        root: path_bytes(root),
        path: path_bytes(&path),
        length,
        created_unix_millis: system_time_millis(now)?,
        state: ManifestState::Partial,
        device: identity.device,
        inode: identity.inode,
    });
    Ok(())
}

fn validate_manifest(
    manifest: &AttachmentManifest,
    fallback_root: &Path,
) -> Result<(), UploadError> {
    if manifest.version != MANIFEST_VERSION || manifest.records.len() > MAX_MANIFEST_RECORDS {
        return Err(UploadError::new(
            UploadErrorKind::UnsafePath,
            "attachment manifest version or record count is invalid",
        ));
    }
    let mut ids = HashSet::new();
    let mut paths = HashSet::new();
    for record in &manifest.records {
        let parsed_id = Uuid::parse_str(&record.id).map_err(|_| {
            UploadError::new(UploadErrorKind::UnsafePath, "manifest id is not a UUID")
        })?;
        if parsed_id.hyphenated().to_string() != record.id
            || !ids.insert(record.id.clone())
            || record.length == 0
            || record.length > MAX_UPLOAD_BYTES
            || record.device == 0
            || record.inode == 0
        {
            return Err(UploadError::new(
                UploadErrorKind::UnsafePath,
                "attachment manifest record is invalid",
            ));
        }
        let root = path_from_bytes(&record.root);
        let path = path_from_bytes(&record.path);
        if !root.is_absolute() || !paths.insert(path.clone()) {
            return Err(UploadError::new(
                UploadErrorKind::UnsafePath,
                "attachment manifest path is invalid",
            ));
        }
        let allowed_project_root = root.file_name() == Some(OsStr::new("attachments"))
            && root.parent().and_then(Path::file_name) == Some(OsStr::new(".aiterm"));
        if root != fallback_root && !allowed_project_root {
            return Err(UploadError::new(
                UploadErrorKind::UnsafePath,
                "attachment manifest confinement root is invalid",
            ));
        }
        let part_path = root.join(format!("{}.jpg.part", record.id));
        let published_path = root.join(format!("{}.jpg", record.id));
        let path_is_valid = match record.state {
            ManifestState::Partial => path == part_path || path == published_path,
            ManifestState::Complete => path == published_path,
        };
        if !path_is_valid {
            return Err(UploadError::new(
                UploadErrorKind::UnsafePath,
                "attachment manifest path escapes its confinement root",
            ));
        }
        match fs::symlink_metadata(&root) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() || !metadata.is_dir() {
                    return Err(UploadError::new(
                        UploadErrorKind::UnsafePath,
                        "attachment manifest root is not a real directory",
                    ));
                }
                let canonical = fs::canonicalize(&root)
                    .map_err(|error| UploadError::storage("canonicalize manifest root", error))?;
                if canonical != root {
                    return Err(UploadError::new(
                        UploadErrorKind::UnsafePath,
                        "attachment manifest root is not canonical",
                    ));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(UploadError::storage("inspect manifest root", error)),
        }
        match fs::symlink_metadata(&path) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() || !metadata.is_file() {
                    return Err(UploadError::new(
                        UploadErrorKind::UnsafePath,
                        "manifest attachment was replaced with an unsafe entry",
                    ));
                }
                let directory = StableDirectory::open_existing_absolute(&root)?;
                let file = match directory.open_file(
                    path.file_name()
                        .expect("validated attachment path has a name"),
                ) {
                    Ok(file) => file,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                    Err(error) => {
                        return Err(path_operation_error("open manifested attachment", error))
                    }
                };
                let identity = FileIdentity::of(&file)?;
                if identity.device != record.device
                    || identity.inode != record.inode
                    || !identity.is_regular()
                    || (record.state == ManifestState::Complete && metadata.len() != record.length)
                    || (record.state == ManifestState::Partial && metadata.len() > record.length)
                {
                    return Err(UploadError::new(
                        UploadErrorKind::UnsafePath,
                        "manifest attachment identity or length changed",
                    ));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(UploadError::storage("inspect manifested attachment", error)),
        }
    }
    Ok(())
}

fn manifest_record_exists(record: &ManifestRecord) -> Result<bool, UploadError> {
    match fs::symlink_metadata(path_from_bytes(&record.path)) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            Err(UploadError::new(
                UploadErrorKind::UnsafePath,
                "manifested attachment is unsafe",
            ))
        }
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(UploadError::storage("inspect manifested attachment", error)),
    }
}

fn delete_manifest_file(record: &ManifestRecord) -> Result<(), UploadError> {
    let root = path_from_bytes(&record.root);
    let path = path_from_bytes(&record.path);
    let directory = StableDirectory::open_existing_absolute(&root)?;
    directory.verify_current_path()?;
    let name = path
        .file_name()
        .ok_or_else(|| UploadError::new(UploadErrorKind::UnsafePath, "attachment has no name"))?;
    let file = directory
        .open_file(name)
        .map_err(|error| path_operation_error("open attachment for cleanup", error))?;
    let identity = FileIdentity::of(&file)?;
    if !identity.is_regular() || identity.device != record.device || identity.inode != record.inode
    {
        return Err(UploadError::new(
            UploadErrorKind::UnsafePath,
            "attachment changed before cleanup",
        ));
    }
    directory.verify_entry_identity(name, &file)?;
    directory.unlink(name)?;
    directory.sync()
}

fn duration_millis(duration: Duration) -> Result<u64, UploadError> {
    u64::try_from(duration.as_millis()).map_err(|_| {
        UploadError::new(
            UploadErrorKind::Storage,
            "attachment duration is out of range",
        )
    })
}

fn system_time_millis(time: SystemTime) -> Result<u64, UploadError> {
    duration_millis(time.duration_since(UNIX_EPOCH).map_err(|_| {
        UploadError::new(
            UploadErrorKind::Storage,
            "attachment timestamp predates the Unix epoch",
        )
    })?)
}

#[cfg(unix)]
fn path_bytes(path: &Path) -> Vec<u8> {
    path.as_os_str().as_bytes().to_vec()
}

#[cfg(unix)]
fn path_from_bytes(bytes: &[u8]) -> PathBuf {
    PathBuf::from(OsString::from_vec(bytes.to_vec()))
}

#[derive(Debug)]
struct StableDirectory {
    #[cfg(unix)]
    file: File,
    path: PathBuf,
}

#[cfg(unix)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FileIdentity {
    device: u64,
    inode: u64,
    kind: u32,
}

#[cfg(unix)]
impl FileIdentity {
    fn of(file: &File) -> Result<Self, UploadError> {
        let metadata = file
            .metadata()
            .map_err(|error| UploadError::storage("inspect open file", error))?;
        Ok(Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            kind: metadata.mode() & libc::S_IFMT,
        })
    }

    fn is_regular(self) -> bool {
        self.kind == libc::S_IFREG
    }

    fn is_directory(self) -> bool {
        self.kind == libc::S_IFDIR
    }
}

impl StableDirectory {
    #[cfg(unix)]
    fn open_existing_absolute(path: &Path) -> Result<Self, UploadError> {
        use std::path::Component;

        if !path.is_absolute() {
            return Err(UploadError::new(
                UploadErrorKind::UnsafePath,
                "stable directory path is not absolute",
            ));
        }
        let root = unsafe {
            libc::open(
                c"/".as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if root < 0 {
            return Err(UploadError::storage(
                "open filesystem root",
                std::io::Error::last_os_error(),
            ));
        }
        let mut current = unsafe { File::from_raw_fd(root) };
        for component in path.components() {
            let name = match component {
                Component::RootDir => continue,
                Component::Normal(name) => name,
                _ => {
                    return Err(UploadError::new(
                        UploadErrorKind::UnsafePath,
                        "stable directory contains a non-normal component",
                    ))
                }
            };
            current = open_directory_at(&current, name)?;
        }
        let identity = FileIdentity::of(&current)?;
        if !identity.is_directory() {
            return Err(UploadError::new(
                UploadErrorKind::UnsafePath,
                "stable path is not a directory",
            ));
        }
        Ok(Self {
            file: current,
            path: path.to_path_buf(),
        })
    }

    #[cfg(not(unix))]
    fn open_existing_absolute(_path: &Path) -> Result<Self, UploadError> {
        Err(unsupported_stable_filesystem())
    }

    fn open_or_create_tree(path: &Path) -> Result<Self, UploadError> {
        let absolute = if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()
                .map_err(|error| UploadError::storage("resolve current directory", error))?
                .join(path)
        };
        Self::open_or_create_tree_inner(&absolute, true)
    }

    fn open_or_create_tree_inner(
        path: &Path,
        enforce_leaf_mode: bool,
    ) -> Result<Self, UploadError> {
        match fs::symlink_metadata(path) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() || !metadata.is_dir() {
                    return Err(UploadError::new(
                        UploadErrorKind::UnsafePath,
                        format!("{} is a symlink or non-directory", path.display()),
                    ));
                }
                let canonical = fs::canonicalize(path).map_err(|error| {
                    UploadError::storage("canonicalize stable directory", error)
                })?;
                let directory = Self::open_existing_absolute(&canonical)?;
                if enforce_leaf_mode {
                    directory.set_owner_only()?;
                }
                Ok(directory)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let parent = path.parent().ok_or_else(|| {
                    UploadError::new(
                        UploadErrorKind::UnsafePath,
                        "stable directory has no parent",
                    )
                })?;
                let name = path.file_name().ok_or_else(|| {
                    UploadError::new(
                        UploadErrorKind::UnsafePath,
                        "stable directory has no leaf name",
                    )
                })?;
                let parent = Self::open_or_create_tree_inner(parent, false)?;
                parent.open_or_create_child(name, true)
            }
            Err(error) => Err(UploadError::storage("inspect stable directory", error)),
        }
    }

    #[cfg(unix)]
    fn duplicate(&self) -> Result<Self, UploadError> {
        Ok(Self {
            file: self
                .file
                .try_clone()
                .map_err(|error| UploadError::storage("clone stable directory", error))?,
            path: self.path.clone(),
        })
    }

    #[cfg(not(unix))]
    fn duplicate(&self) -> Result<Self, UploadError> {
        Err(unsupported_stable_filesystem())
    }

    #[cfg(unix)]
    fn open_or_create_child(&self, name: &OsStr, owner_only: bool) -> Result<Self, UploadError> {
        let name_c = cstring(name, "directory name")?;
        let created = unsafe { libc::mkdirat(self.file.as_raw_fd(), name_c.as_ptr(), 0o700) };
        if created != 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::AlreadyExists {
                return Err(path_operation_error("create stable directory", error));
            }
        }
        let child = open_directory_at(&self.file, name)?;
        let directory = Self {
            file: child,
            path: self.path.join(name),
        };
        if owner_only {
            directory.set_owner_only()?;
        }
        directory.verify_current_path()?;
        Ok(directory)
    }

    #[cfg(not(unix))]
    fn open_or_create_child(&self, _name: &OsStr, _owner_only: bool) -> Result<Self, UploadError> {
        Err(unsupported_stable_filesystem())
    }

    #[cfg(unix)]
    fn set_owner_only(&self) -> Result<(), UploadError> {
        if unsafe { libc::fchmod(self.file.as_raw_fd(), 0o700) } == 0 {
            Ok(())
        } else {
            Err(UploadError::storage(
                "set stable directory permissions",
                std::io::Error::last_os_error(),
            ))
        }
    }

    #[cfg(not(unix))]
    fn set_owner_only(&self) -> Result<(), UploadError> {
        Err(unsupported_stable_filesystem())
    }

    #[cfg(unix)]
    fn create_new_file(&self, name: &OsStr) -> std::io::Result<File> {
        let name = CString::new(name.as_bytes())
            .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
        let descriptor = unsafe {
            libc::openat(
                self.file.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDWR | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                0o600,
            )
        };
        if descriptor < 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(unsafe { File::from_raw_fd(descriptor) })
        }
    }

    #[cfg(unix)]
    fn lock_manifest(&self) -> Result<File, UploadError> {
        let name = cstring(OsStr::new(MANIFEST_LOCK_NAME), "manifest lock name")?;
        let descriptor = unsafe {
            libc::openat(
                self.file.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDWR
                    | libc::O_CREAT
                    | libc::O_NOFOLLOW
                    | libc::O_CLOEXEC
                    | libc::O_NONBLOCK,
                0o600,
            )
        };
        if descriptor < 0 {
            return Err(path_operation_error(
                "open attachment manifest lock",
                std::io::Error::last_os_error(),
            ));
        }
        let file = unsafe { File::from_raw_fd(descriptor) };
        if !FileIdentity::of(&file)?.is_regular() {
            return Err(UploadError::new(
                UploadErrorKind::UnsafePath,
                "attachment manifest lock is not a regular file",
            ));
        }
        if unsafe { libc::fchmod(file.as_raw_fd(), 0o600) } != 0 {
            return Err(UploadError::storage(
                "set attachment manifest lock permissions",
                std::io::Error::last_os_error(),
            ));
        }
        #[cfg(test)]
        manifest_lock_test_before_flock()?;
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
            return Err(UploadError::storage(
                "lock attachment manifest",
                std::io::Error::last_os_error(),
            ));
        }
        #[cfg(test)]
        manifest_lock_test_after_flock()?;
        Ok(file)
    }

    #[cfg(not(unix))]
    fn lock_manifest(&self) -> Result<File, UploadError> {
        Err(unsupported_stable_filesystem())
    }

    #[cfg(not(unix))]
    fn create_new_file(&self, _name: &OsStr) -> std::io::Result<File> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "stable attachment storage requires Unix",
        ))
    }

    #[cfg(target_os = "linux")]
    fn create_anonymous_file(&self) -> std::io::Result<File> {
        let descriptor = unsafe {
            libc::openat(
                self.file.as_raw_fd(),
                c".".as_ptr(),
                libc::O_RDWR | libc::O_TMPFILE | libc::O_CLOEXEC,
                0o600,
            )
        };
        if descriptor < 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(unsafe { File::from_raw_fd(descriptor) })
        }
    }

    #[cfg(unix)]
    fn open_file(&self, name: &OsStr) -> std::io::Result<File> {
        let name = CString::new(name.as_bytes())
            .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
        let descriptor = unsafe {
            libc::openat(
                self.file.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
            )
        };
        if descriptor < 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(unsafe { File::from_raw_fd(descriptor) })
        }
    }

    #[cfg(unix)]
    fn require_entry_absent(&self, name: &OsStr) -> Result<(), UploadError> {
        let name = cstring(name, "attachment destination name")?;
        let mut metadata = std::mem::MaybeUninit::<libc::stat>::uninit();
        let result = unsafe {
            libc::fstatat(
                self.file.as_raw_fd(),
                name.as_ptr(),
                metadata.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };
        if result == 0 {
            return Err(UploadError::new(
                UploadErrorKind::Storage,
                "attachment publication destination already exists",
            ));
        }
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::NotFound {
            Ok(())
        } else {
            Err(path_operation_error(
                "inspect attachment publication destination",
                error,
            ))
        }
    }

    #[cfg(not(unix))]
    fn require_entry_absent(&self, _name: &OsStr) -> Result<(), UploadError> {
        Err(unsupported_stable_filesystem())
    }

    #[cfg(unix)]
    fn verify_current_path(&self) -> Result<(), UploadError> {
        let current = Self::open_existing_absolute(&self.path)?;
        if FileIdentity::of(&current.file)? == FileIdentity::of(&self.file)? {
            Ok(())
        } else {
            Err(UploadError::new(
                UploadErrorKind::UnsafePath,
                "stable directory was replaced",
            ))
        }
    }

    #[cfg(not(unix))]
    fn verify_current_path(&self) -> Result<(), UploadError> {
        Err(unsupported_stable_filesystem())
    }

    #[cfg(unix)]
    fn verify_entry_identity(&self, name: &OsStr, held: &File) -> Result<(), UploadError> {
        let current = self
            .open_file(name)
            .map_err(|error| path_operation_error("open stable file entry", error))?;
        let current_identity = FileIdentity::of(&current)?;
        let held_identity = FileIdentity::of(held)?;
        if !current_identity.is_regular() || current_identity != held_identity {
            return Err(UploadError::new(
                UploadErrorKind::UnsafePath,
                "staged attachment entry was replaced",
            ));
        }
        Ok(())
    }

    #[cfg(not(unix))]
    fn verify_entry_identity(&self, _name: &OsStr, _held: &File) -> Result<(), UploadError> {
        Err(unsupported_stable_filesystem())
    }

    #[cfg(unix)]
    fn read_optional_file(
        &self,
        name: &OsStr,
        max_bytes: u64,
    ) -> Result<(Vec<u8>, Option<FileIdentity>), UploadError> {
        let file = match self.open_file(name) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok((Vec::new(), None))
            }
            Err(error) => return Err(path_operation_error("open stable optional file", error)),
        };
        let identity = FileIdentity::of(&file)?;
        if !identity.is_regular() {
            return Err(UploadError::new(
                UploadErrorKind::UnsafePath,
                "stable optional entry is not a regular file",
            ));
        }
        let metadata_length = file
            .metadata()
            .map_err(|error| UploadError::storage("inspect stable optional file", error))?
            .len();
        if metadata_length > max_bytes {
            return Err(UploadError::new(
                UploadErrorKind::Capacity,
                "stable optional file exceeds its safety limit",
            ));
        }
        let mut bytes = Vec::new();
        file.take(max_bytes.saturating_add(1))
            .read_to_end(&mut bytes)
            .map_err(|error| UploadError::storage("read stable optional file", error))?;
        if bytes.len() as u64 > max_bytes {
            return Err(UploadError::new(
                UploadErrorKind::Capacity,
                "stable optional file exceeds its safety limit",
            ));
        }
        Ok((bytes, Some(identity)))
    }

    #[cfg(not(unix))]
    fn read_optional_file(
        &self,
        _name: &OsStr,
        _max_bytes: u64,
    ) -> Result<(Vec<u8>, Option<()>), UploadError> {
        Err(unsupported_stable_filesystem())
    }

    #[cfg(unix)]
    fn verify_expected_entry(
        &self,
        name: &OsStr,
        expected: Option<FileIdentity>,
    ) -> Result<(), UploadError> {
        match (self.open_file(name), expected) {
            (Err(error), None) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            (Ok(file), Some(expected)) if FileIdentity::of(&file)? == expected => Ok(()),
            (Err(error), Some(_)) => Err(path_operation_error("reopen stable entry", error)),
            _ => Err(UploadError::new(
                UploadErrorKind::UnsafePath,
                "stable entry changed during atomic update",
            )),
        }
    }

    #[cfg(unix)]
    fn write_atomic_replacing(
        &self,
        name: &OsStr,
        bytes: &[u8],
        expected: Option<FileIdentity>,
    ) -> Result<(), UploadError> {
        let temporary = OsString::from(format!(".aiterm-exclude-{}.tmp", Uuid::new_v4()));
        let mut file = self
            .create_new_file(&temporary)
            .map_err(|error| path_operation_error("create atomic exclude temporary", error))?;
        let result = (|| {
            file.write_all(bytes)
                .map_err(|error| UploadError::storage("write atomic exclude temporary", error))?;
            file.flush()
                .map_err(|error| UploadError::storage("flush atomic exclude temporary", error))?;
            file.sync_all()
                .map_err(|error| UploadError::storage("sync atomic exclude temporary", error))?;
            self.verify_current_path()?;
            self.verify_expected_entry(name, expected)?;
            self.rename_replacing(&temporary, name)?;
            self.sync()?;
            self.verify_current_path()
        })();
        if result.is_err() {
            let _ = self.unlink(&temporary);
        }
        result
    }

    #[cfg(not(unix))]
    fn write_atomic_replacing(
        &self,
        _name: &OsStr,
        _bytes: &[u8],
        _expected: Option<()>,
    ) -> Result<(), UploadError> {
        Err(unsupported_stable_filesystem())
    }

    #[cfg(unix)]
    fn unlink(&self, name: &OsStr) -> Result<(), UploadError> {
        let name = cstring(name, "file name")?;
        if unsafe { libc::unlinkat(self.file.as_raw_fd(), name.as_ptr(), 0) } == 0 {
            Ok(())
        } else {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::NotFound {
                Err(UploadError::new(
                    UploadErrorKind::NotFound,
                    "stable entry was not found",
                ))
            } else {
                Err(path_operation_error("unlink stable entry", error))
            }
        }
    }

    #[cfg(not(unix))]
    fn unlink(&self, _name: &OsStr) -> Result<(), UploadError> {
        Err(unsupported_stable_filesystem())
    }

    #[cfg(target_os = "linux")]
    fn link_held_noreplace(&self, held: &File, new: &OsStr) -> Result<(), UploadError> {
        let new = cstring(new, "new file name")?;
        let mut result = unsafe {
            libc::linkat(
                held.as_raw_fd(),
                c"".as_ptr(),
                self.file.as_raw_fd(),
                new.as_ptr(),
                libc::AT_EMPTY_PATH,
            )
        };
        if result != 0 {
            let error = std::io::Error::last_os_error();
            if matches!(
                error.raw_os_error(),
                Some(libc::EPERM) | Some(libc::EINVAL) | Some(libc::ENOENT)
            ) {
                let proc_path = CString::new(format!("/proc/self/fd/{}", held.as_raw_fd()))
                    .expect("file descriptor path cannot contain NUL");
                result = unsafe {
                    libc::linkat(
                        libc::AT_FDCWD,
                        proc_path.as_ptr(),
                        self.file.as_raw_fd(),
                        new.as_ptr(),
                        libc::AT_SYMLINK_FOLLOW,
                    )
                };
            }
        }
        if result == 0 {
            Ok(())
        } else {
            Err(path_operation_error(
                "publish held attachment inode",
                std::io::Error::last_os_error(),
            ))
        }
    }

    #[cfg(all(unix, not(target_os = "linux")))]
    fn link_held_noreplace(&self, _held: &File, _new: &OsStr) -> Result<(), UploadError> {
        Err(UploadError::new(
            UploadErrorKind::Storage,
            "held-inode no-replace attachment publication is only supported on Linux",
        ))
    }

    #[cfg(not(unix))]
    fn link_held_noreplace(&self, _held: &File, _new: &OsStr) -> Result<(), UploadError> {
        Err(unsupported_stable_filesystem())
    }

    #[cfg(unix)]
    fn rename_replacing(&self, old: &OsStr, new: &OsStr) -> Result<(), UploadError> {
        let old = cstring(old, "old file name")?;
        let new = cstring(new, "new file name")?;
        if unsafe {
            libc::renameat(
                self.file.as_raw_fd(),
                old.as_ptr(),
                self.file.as_raw_fd(),
                new.as_ptr(),
            )
        } == 0
        {
            Ok(())
        } else {
            Err(path_operation_error(
                "replace stable entry",
                std::io::Error::last_os_error(),
            ))
        }
    }

    #[cfg(unix)]
    fn sync(&self) -> Result<(), UploadError> {
        self.file
            .sync_all()
            .map_err(|error| UploadError::storage("sync stable directory", error))
    }

    #[cfg(not(unix))]
    fn sync(&self) -> Result<(), UploadError> {
        Err(unsupported_stable_filesystem())
    }
}

#[cfg(unix)]
fn open_directory_at(parent: &File, name: &OsStr) -> Result<File, UploadError> {
    let name = cstring(name, "directory name")?;
    let descriptor = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if descriptor < 0 {
        Err(path_operation_error(
            "open stable directory",
            std::io::Error::last_os_error(),
        ))
    } else {
        Ok(unsafe { File::from_raw_fd(descriptor) })
    }
}

#[cfg(unix)]
fn cstring(value: &OsStr, label: &str) -> Result<CString, UploadError> {
    CString::new(value.as_bytes()).map_err(|_| {
        UploadError::new(
            UploadErrorKind::UnsafePath,
            format!("{label} contains a NUL byte"),
        )
    })
}

fn path_operation_error(action: &str, error: std::io::Error) -> UploadError {
    match error.raw_os_error() {
        Some(libc::ELOOP | libc::ENOTDIR) => UploadError::new(
            UploadErrorKind::UnsafePath,
            format!("{action}: path was replaced or contains a symlink"),
        ),
        _ => UploadError::storage(action, error),
    }
}

#[cfg(test)]
fn manifest_lock_test_before_flock() -> Result<(), UploadError> {
    let Some(before) = std::env::var_os("AITERM_UPLOAD_UNIT_LOCK_BEFORE") else {
        return Ok(());
    };
    let allow = std::env::var_os("AITERM_UPLOAD_UNIT_LOCK_ALLOW").ok_or_else(|| {
        UploadError::new(
            UploadErrorKind::Storage,
            "manifest lock test boundary is missing its release path",
        )
    })?;
    fs::write(before, b"at flock boundary")
        .map_err(|error| UploadError::storage("signal manifest lock test boundary", error))?;
    let allow = PathBuf::from(allow);
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        if allow.exists() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    Err(UploadError::new(
        UploadErrorKind::Storage,
        "timed out at manifest lock test boundary",
    ))
}

#[cfg(test)]
fn manifest_lock_test_after_flock() -> Result<(), UploadError> {
    let Some(acquired) = std::env::var_os("AITERM_UPLOAD_UNIT_LOCK_ACQUIRED") else {
        return Ok(());
    };
    fs::write(acquired, b"inside flock boundary")
        .map_err(|error| UploadError::storage("signal acquired manifest test lock", error))
}

#[cfg(not(unix))]
fn unsupported_stable_filesystem() -> UploadError {
    UploadError::new(
        UploadErrorKind::Storage,
        "stable attachment storage requires Unix descriptor-relative filesystem operations",
    )
}

type ExcludeLockRegistry = Mutex<HashMap<PathBuf, Weak<Mutex<()>>>>;
static EXCLUDE_UPDATE_LOCKS: OnceLock<ExcludeLockRegistry> = OnceLock::new();
type ManifestLockRegistry = Mutex<HashMap<PathBuf, Weak<Mutex<()>>>>;
static MANIFEST_UPDATE_LOCKS: OnceLock<ManifestLockRegistry> = OnceLock::new();

fn manifest_lock_for(path: &Path) -> Arc<Mutex<()>> {
    let registry = MANIFEST_UPDATE_LOCKS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut locks = registry
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    locks.retain(|_, lock| lock.strong_count() > 0);
    if let Some(lock) = locks.get(path).and_then(Weak::upgrade) {
        return lock;
    }
    let lock = Arc::new(Mutex::new(()));
    locks.insert(path.to_path_buf(), Arc::downgrade(&lock));
    lock
}

fn exclude_update_lock(path: &Path) -> Result<Arc<Mutex<()>>, UploadError> {
    let registry = EXCLUDE_UPDATE_LOCKS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut locks = registry.lock().map_err(|_| {
        UploadError::new(
            UploadErrorKind::Storage,
            "Git exclude update lock registry is poisoned",
        )
    })?;
    locks.retain(|_, lock| lock.strong_count() > 0);
    if let Some(lock) = locks.get(path).and_then(Weak::upgrade) {
        return Ok(lock);
    }
    let lock = Arc::new(Mutex::new(()));
    locks.insert(path.to_path_buf(), Arc::downgrade(&lock));
    Ok(lock)
}

fn update_local_git_exclude(cwd: &Path) -> Result<(), UploadError> {
    let repository = match git2::Repository::discover(cwd) {
        Ok(repository) => repository,
        Err(error) if error.code() == git2::ErrorCode::NotFound => return Ok(()),
        Err(error) => return Err(UploadError::storage("discover Git repository", error)),
    };
    let workdir = repository.workdir().ok_or_else(|| {
        UploadError::new(
            UploadErrorKind::UnsafePath,
            "Git repository has no working tree",
        )
    })?;
    let workdir = fs::canonicalize(workdir)
        .map_err(|error| UploadError::storage("canonicalize Git worktree", error))?;
    let relative = cwd.strip_prefix(&workdir).map_err(|_| {
        UploadError::new(
            UploadErrorKind::UnsafePath,
            "tab working directory is outside the discovered Git worktree",
        )
    })?;
    let entry = git_exclude_entry(relative)?;

    let commondir = fs::canonicalize(repository.commondir())
        .map_err(|error| UploadError::storage("canonicalize Git common directory", error))?;
    let exclude_path = commondir.join("info/exclude");
    let update_lock = exclude_update_lock(&exclude_path)?;
    let _update_guard = update_lock.lock().map_err(|_| {
        UploadError::new(
            UploadErrorKind::Storage,
            "Git exclude update lock is poisoned",
        )
    })?;
    let common = StableDirectory::open_existing_absolute(&commondir)?;
    let info = common.open_or_create_child(OsStr::new("info"), false)?;
    let (mut bytes, original_identity) =
        info.read_optional_file(OsStr::new("exclude"), MAX_GIT_EXCLUDE_BYTES)?;

    if bytes.split(|byte| *byte == b'\n').any(|line| {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        normalized_git_pattern(trim_ascii(line)) == normalized_git_pattern(&entry)
    }) {
        return Ok(());
    }

    let newline: &[u8] = if bytes.windows(2).any(|pair| pair == b"\r\n") {
        b"\r\n"
    } else {
        b"\n"
    };
    if !bytes.is_empty() && !bytes.ends_with(b"\n") {
        bytes.extend_from_slice(newline);
    }
    bytes.extend_from_slice(&entry);
    bytes.extend_from_slice(newline);
    info.write_atomic_replacing(OsStr::new("exclude"), &bytes, original_identity)
}

fn normalized_git_pattern(mut pattern: &[u8]) -> &[u8] {
    if pattern.first() == Some(&b'/') {
        pattern = &pattern[1..];
    }
    pattern
}

fn git_exclude_entry(relative_cwd: &Path) -> Result<Vec<u8>, UploadError> {
    use std::path::Component;

    let mut entry = vec![b'/'];
    for component in relative_cwd.components() {
        let Component::Normal(name) = component else {
            return Err(UploadError::new(
                UploadErrorKind::UnsafePath,
                "Git-relative attachment path contains a non-normal component",
            ));
        };
        append_escaped_git_component(&mut entry, name)?;
        entry.push(b'/');
    }
    entry.extend_from_slice(b".aiterm/attachments/");
    Ok(entry)
}

#[cfg(unix)]
fn append_escaped_git_component(
    output: &mut Vec<u8>,
    component: &OsStr,
) -> Result<(), UploadError> {
    for byte in component.as_bytes() {
        if matches!(byte, b'\n' | b'\r' | 0) {
            return Err(UploadError::new(
                UploadErrorKind::UnsafePath,
                "Git-relative attachment path cannot be represented safely",
            ));
        }
        if matches!(byte, b'\\' | b'!' | b'#' | b'[' | b']' | b'*' | b'?' | b' ') {
            output.push(b'\\');
        }
        output.push(*byte);
    }
    Ok(())
}

#[cfg(not(unix))]
fn append_escaped_git_component(
    output: &mut Vec<u8>,
    component: &OsStr,
) -> Result<(), UploadError> {
    for byte in component.to_string_lossy().bytes() {
        if matches!(byte, b'\n' | b'\r' | 0) {
            return Err(UploadError::new(
                UploadErrorKind::UnsafePath,
                "Git-relative attachment path cannot be represented safely",
            ));
        }
        if matches!(byte, b'\\' | b'!' | b'#' | b'[' | b']' | b'*' | b'?' | b' ') {
            output.push(b'\\');
        }
        output.push(byte);
    }
    Ok(())
}

fn trim_ascii(mut bytes: &[u8]) -> &[u8] {
    while bytes.first().is_some_and(u8::is_ascii_whitespace) {
        bytes = &bytes[1..];
    }
    while bytes.last().is_some_and(u8::is_ascii_whitespace) {
        bytes = &bytes[..bytes.len() - 1];
    }
    bytes
}

fn upload_not_found() -> UploadError {
    UploadError::new(UploadErrorKind::NotFound, "upload id was not found")
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    fn append_segment(output: &mut Vec<u8>, marker: u8, payload: &[u8]) {
        output.extend_from_slice(&[0xff, marker]);
        output.extend_from_slice(&u16::try_from(payload.len() + 2).unwrap().to_be_bytes());
        output.extend_from_slice(payload);
    }

    fn compact_baseline_jpeg(width: u16, restart_interval: Option<u16>) -> Vec<u8> {
        let mut jpeg = vec![0xff, 0xd8];
        append_segment(
            &mut jpeg,
            0xc0,
            &[8, 0, 1, (width >> 8) as u8, width as u8, 1, 1, 0x11, 0],
        );
        let mut huffman = Vec::new();
        huffman.push(0x00);
        huffman.extend_from_slice(&[1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        huffman.push(0x00);
        huffman.push(0x10);
        huffman.extend_from_slice(&[1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        huffman.push(0x00);
        append_segment(&mut jpeg, 0xc4, &huffman);
        if let Some(interval) = restart_interval {
            append_segment(&mut jpeg, 0xdd, &interval.to_be_bytes());
        }
        append_segment(&mut jpeg, 0xda, &[1, 1, 0, 0, 63, 0]);
        let mcu_count = u32::from(width).div_ceil(8);
        for mcu in 0..mcu_count {
            jpeg.push(0x3f);
            if let Some(interval) = restart_interval {
                if (mcu + 1) % u32::from(interval) == 0 && mcu + 1 < mcu_count {
                    jpeg.extend_from_slice(&[0xff, 0xd0 + ((mcu / u32::from(interval)) % 8) as u8]);
                }
            }
        }
        jpeg.extend_from_slice(&[0xff, 0xd9]);
        jpeg
    }

    #[test]
    fn oversized_sof_is_rejected_before_entropy_tables_are_needed() {
        let oversized_sof = [8, 0, 1, 0x10, 0x01, 1, 1, 0x11, 0];

        let Err(error) = parse_baseline_frame(&oversized_sof) else {
            panic!("oversized SOF was accepted")
        };

        assert_eq!(error.kind(), UploadErrorKind::InvalidImage);
    }

    #[test]
    fn aggregate_sampling_factor_above_baseline_limit_is_rejected() {
        let oversized_sampling = [8, 0, 1, 0, 1, 3, 1, 0x22, 0, 2, 0x22, 0, 3, 0x22, 0];

        let Err(error) = parse_baseline_frame(&oversized_sampling) else {
            panic!("oversized aggregate sampling was accepted")
        };

        assert_eq!(error.kind(), UploadErrorKind::InvalidImage);
    }

    #[test]
    fn oversubscribed_huffman_table_is_rejected_during_dht_parsing() {
        let mut malformed = vec![0x00, 3];
        malformed.extend_from_slice(&[0; 15]);
        malformed.extend_from_slice(&[0, 1, 2]);
        let mut dc_tables: [Option<StrictHuffmanTable>; 4] = Default::default();
        let mut ac_tables: [Option<StrictHuffmanTable>; 4] = Default::default();

        let error = parse_huffman_tables(&malformed, &mut dc_tables, &mut ac_tables).unwrap_err();

        assert_eq!(error.kind(), UploadErrorKind::InvalidImage);
    }

    #[test]
    fn malformed_sof_and_sos_segments_fail_closed() {
        let Err(frame_error) = parse_baseline_frame(&[8, 0, 1, 0, 1, 2]) else {
            panic!("malformed SOF was accepted")
        };
        assert_eq!(frame_error.kind(), UploadErrorKind::InvalidImage);
        let frame = vec![BaselineComponent {
            id: 1,
            horizontal_sampling: 1,
            vertical_sampling: 1,
        }];
        assert_eq!(
            parse_baseline_scan(&[1, 1, 0, 1, 63, 0], &frame)
                .unwrap_err()
                .kind(),
            UploadErrorKind::InvalidImage
        );
    }

    #[test]
    fn restart_markers_must_follow_the_declared_sequence() {
        let valid = compact_baseline_jpeg(9, Some(1));
        validate_complete_baseline_jpeg(&valid).unwrap();
        let mut wrong_restart = valid;
        let restart = wrong_restart
            .windows(2)
            .position(|pair| pair == [0xff, 0xd0])
            .unwrap();
        wrong_restart[restart + 1] = 0xd1;

        assert_eq!(
            validate_complete_baseline_jpeg(&wrong_restart)
                .unwrap_err()
                .kind(),
            UploadErrorKind::InvalidImage
        );
    }

    #[test]
    fn non_one_entropy_padding_is_rejected() {
        let mut malformed = compact_baseline_jpeg(1, None);
        let entropy = malformed.len() - 3;
        assert_eq!(malformed[entropy], 0x3f);
        malformed[entropy] = 0x00;

        assert_eq!(
            validate_complete_baseline_jpeg(&malformed)
                .unwrap_err()
                .kind(),
            UploadErrorKind::InvalidImage
        );
    }

    #[test]
    fn sos_referencing_undefined_huffman_tables_is_rejected() {
        let mut malformed = compact_baseline_jpeg(1, None);
        let sos = malformed
            .windows(2)
            .position(|pair| pair == [0xff, 0xda])
            .unwrap();
        malformed[sos + 6] = 0x11;

        assert_eq!(
            validate_complete_baseline_jpeg(&malformed)
                .unwrap_err()
                .kind(),
            UploadErrorKind::InvalidImage
        );
    }

    fn normalized_test_jpeg() -> Vec<u8> {
        use image::{DynamicImage, ImageBuffer, ImageFormat, Rgb};

        let image = ImageBuffer::from_pixel(16, 16, Rgb([19_u8, 71_u8, 113_u8]));
        let mut bytes = Cursor::new(Vec::new());
        DynamicImage::ImageRgb8(image)
            .write_to(&mut bytes, ImageFormat::Jpeg)
            .unwrap();
        bytes.into_inner()
    }

    fn manifest_record_count(cache: &Path) -> usize {
        let bytes = fs::read(cache.join("remote-attachments/attachments.json")).unwrap();
        serde_json::from_slice::<AttachmentManifest>(&bytes)
            .unwrap()
            .records
            .len()
    }

    fn injected_crash() -> UploadError {
        UploadError::new(UploadErrorKind::Storage, "injected crash boundary")
    }

    fn injected_storage_fault(point: StorageFaultPoint) -> UploadError {
        UploadError::new(
            UploadErrorKind::Storage,
            format!("injected storage fault at {point:?}"),
        )
    }

    fn cleanup_faults(context: CleanupContext) -> [StorageFaultPoint; 3] {
        [
            StorageFaultPoint::CleanupUnlink(context),
            StorageFaultPoint::CleanupDirectorySync(context),
            StorageFaultPoint::CleanupManifestPersist(context),
        ]
    }

    #[test]
    fn staging_link_rollback_failures_retain_the_partial_intent_for_maintenance() {
        let context = CleanupContext::PartialLinkRollback;
        for cleanup_fault in cleanup_faults(context) {
            let root = std::env::temp_dir().join(format!(
                "aiterm-stage-rollback-{cleanup_fault:?}-{}",
                Uuid::new_v4()
            ));
            let cwd = root.join("project");
            let cache = root.join("cache");
            fs::create_dir_all(&cwd).unwrap();
            let store = AttachmentStore::new_at(cache.clone(), UNIX_EPOCH).unwrap();
            let canonical = canonical_project_cwd(&cwd).unwrap();
            let directory = project_attachment_directory(&canonical).unwrap();
            let file = directory.create_anonymous_file().unwrap();
            file.sync_all().unwrap();
            let id = Uuid::new_v4().hyphenated().to_string();
            let part_name = OsString::from(format!("{id}.jpg.part"));
            let part_path = directory.path.join(&part_name);

            let error = store
                .manifest
                .record_partial_and_link_with_hooks(
                    &id,
                    &directory.path,
                    &part_name,
                    1,
                    UNIX_EPOCH,
                    FileIdentity::of(&file).unwrap(),
                    &directory,
                    &file,
                    |_| Ok(()),
                    |point| {
                        if point == StorageFaultPoint::PostLinkDirectorySync(context)
                            || point == cleanup_fault
                        {
                            Err(injected_storage_fault(point))
                        } else {
                            Ok(())
                        }
                    },
                )
                .unwrap_err();

            assert_eq!(error.kind(), UploadErrorKind::Storage);
            assert_eq!(manifest_record_count(&cache), 1);
            drop(file);
            store
                .maintain(UNIX_EPOCH + PARTIAL_ATTACHMENT_TTL + Duration::from_secs(1))
                .unwrap();
            assert!(!part_path.exists());
            assert_eq!(manifest_record_count(&cache), 0);
            fs::remove_dir_all(root).unwrap();
        }
    }

    fn rollback_upload_fixture(
        label: &str,
    ) -> (
        PathBuf,
        PathBuf,
        AttachmentStore,
        UploadSet,
        ActiveUpload,
        PathBuf,
    ) {
        let root = std::env::temp_dir().join(format!("aiterm-{label}-{}", Uuid::new_v4()));
        let cwd = root.join("project");
        let cache = root.join("cache");
        fs::create_dir_all(&cwd).unwrap();
        let store = AttachmentStore::new_at(cache.clone(), UNIX_EPOCH).unwrap();
        let jpeg = normalized_test_jpeg();
        let mut uploads = store.upload_set();
        let request = UploadBegin {
            tab_id: TabId::new(),
            attachment_id: AttachmentId::new(),
            submission_id: Uuid::new_v4().hyphenated().to_string(),
            submission_count: 1,
            submission_bytes: jpeg.len() as u64,
            length: jpeg.len() as u64,
            sha256: Sha256::digest(&jpeg).into(),
        };
        let began = uploads.begin_at(Some(&cwd), request, UNIX_EPOCH).unwrap();
        uploads.chunk(&began.upload_id, 0, &jpeg).unwrap();
        let upload = uploads.uploads.remove(&began.upload_id).unwrap();
        let published = upload
            .staged
            .directory
            .path
            .join(&upload.staged.published_name);
        (root, cache, store, uploads, upload, published)
    }

    #[test]
    fn publication_rollbacks_retain_the_final_intent_for_maintenance() {
        for context in [
            CleanupContext::FinalLinkRollback,
            CleanupContext::CompletePersistRollback,
        ] {
            for cleanup_fault in cleanup_faults(context) {
                let (root, cache, store, uploads, mut upload, published) = rollback_upload_fixture(
                    &format!("publish-rollback-{context:?}-{cleanup_fault:?}"),
                );
                let primary_fault = match context {
                    CleanupContext::FinalLinkRollback => {
                        StorageFaultPoint::PostLinkDirectorySync(context)
                    }
                    CleanupContext::CompletePersistRollback => {
                        StorageFaultPoint::CompleteManifestPersist
                    }
                    _ => unreachable!(),
                };

                let error = finish_upload_with_hooks(
                    &mut upload,
                    UNIX_EPOCH,
                    |_| Ok(()),
                    |point| {
                        if point == primary_fault || point == cleanup_fault {
                            Err(injected_storage_fault(point))
                        } else {
                            Ok(())
                        }
                    },
                )
                .unwrap_err();

                assert_eq!(error.kind(), UploadErrorKind::Storage);
                assert_eq!(manifest_record_count(&cache), 1);
                upload.staged.published = true;
                drop(upload);
                drop(uploads);
                store
                    .maintain(UNIX_EPOCH + PARTIAL_ATTACHMENT_TTL + Duration::from_secs(1))
                    .unwrap();
                assert!(!published.exists());
                assert_eq!(manifest_record_count(&cache), 0);
                fs::remove_dir_all(root).unwrap();
            }
        }
    }

    #[test]
    fn part_unlink_sync_failure_disarms_drop_and_retains_the_partial_intent() {
        let (root, cache, store, uploads, mut upload, _published) =
            rollback_upload_fixture("part-unlink-sync");
        let part = upload.staged.directory.path.join(&upload.staged.part_name);

        let error = finish_upload_with_hooks(
            &mut upload,
            UNIX_EPOCH,
            |_| Ok(()),
            |point| {
                if point == StorageFaultPoint::PartUnlinkDirectorySync {
                    Err(injected_storage_fault(point))
                } else {
                    Ok(())
                }
            },
        )
        .unwrap_err();

        assert_eq!(error.kind(), UploadErrorKind::Storage);
        assert!(!part.exists());
        drop(upload);
        drop(uploads);
        assert_eq!(manifest_record_count(&cache), 1);
        store
            .maintain(UNIX_EPOCH + PARTIAL_ATTACHMENT_TTL + Duration::from_secs(1))
            .unwrap();
        assert_eq!(manifest_record_count(&cache), 0);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cancellation_cleanup_failures_retain_the_partial_intent_for_maintenance() {
        let context = CleanupContext::Cancel;
        for cleanup_fault in cleanup_faults(context) {
            let root = std::env::temp_dir().join(format!(
                "aiterm-cancel-rollback-{cleanup_fault:?}-{}",
                Uuid::new_v4()
            ));
            let cwd = root.join("project");
            let cache = root.join("cache");
            fs::create_dir_all(&cwd).unwrap();
            let store = AttachmentStore::new_at(cache.clone(), UNIX_EPOCH).unwrap();
            let mut staged = store.stage(Some(&cwd), 1, UNIX_EPOCH).unwrap();
            let part_path = staged.directory.path.join(&staged.part_name);

            let error = store
                .manifest
                .cancel_staged_with_faults(&staged, |point| {
                    if point == cleanup_fault {
                        Err(injected_storage_fault(point))
                    } else {
                        Ok(())
                    }
                })
                .unwrap_err();

            assert_eq!(error.kind(), UploadErrorKind::Storage);
            assert_eq!(manifest_record_count(&cache), 1);
            staged.published = true;
            drop(staged);
            store
                .maintain(UNIX_EPOCH + PARTIAL_ATTACHMENT_TTL + Duration::from_secs(1))
                .unwrap();
            assert!(!part_path.exists());
            assert_eq!(manifest_record_count(&cache), 0);
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn stage_crash_boundaries_leave_only_manifest_owned_partial_state() {
        for boundary in [StageBoundary::IntentPersisted, StageBoundary::PartLinked] {
            let root = std::env::temp_dir().join(format!(
                "aiterm-stage-crash-{boundary:?}-{}",
                Uuid::new_v4()
            ));
            let cwd = root.join("project");
            let cache = root.join("cache");
            fs::create_dir_all(&cwd).unwrap();
            let store = AttachmentStore::new_at(cache.clone(), UNIX_EPOCH).unwrap();
            let canonical = canonical_project_cwd(&cwd).unwrap();
            let directory = project_attachment_directory(&canonical).unwrap();
            let file = directory.create_anonymous_file().unwrap();
            file.sync_all().unwrap();
            let id = Uuid::new_v4().hyphenated().to_string();
            let part_name = OsString::from(format!("{id}.jpg.part"));
            let part_path = directory.path.join(&part_name);

            let error = store
                .manifest
                .record_partial_and_link_with_hook(
                    &id,
                    &directory.path,
                    &part_name,
                    1,
                    UNIX_EPOCH,
                    FileIdentity::of(&file).unwrap(),
                    &directory,
                    &file,
                    |observed| {
                        if observed == boundary {
                            Err(injected_crash())
                        } else {
                            Ok(())
                        }
                    },
                )
                .unwrap_err();

            assert_eq!(error.kind(), UploadErrorKind::Storage);
            assert_eq!(manifest_record_count(&cache), 1);
            assert_eq!(part_path.exists(), boundary == StageBoundary::PartLinked);
            drop(file);
            store
                .maintain(UNIX_EPOCH + PARTIAL_ATTACHMENT_TTL + Duration::from_secs(1))
                .unwrap();
            assert!(!part_path.exists());
            assert_eq!(manifest_record_count(&cache), 0);
            fs::remove_dir_all(root).unwrap();
        }
    }

    fn publication_crash_fixture(
        boundary: PublicationBoundary,
    ) -> (PathBuf, PathBuf, PathBuf, AttachmentStore) {
        let root = std::env::temp_dir().join(format!(
            "aiterm-publication-crash-{boundary:?}-{}",
            Uuid::new_v4()
        ));
        let cwd = root.join("project");
        let cache = root.join("cache");
        fs::create_dir_all(&cwd).unwrap();
        let store = AttachmentStore::new_at(cache.clone(), UNIX_EPOCH).unwrap();
        let jpeg = normalized_test_jpeg();
        let mut uploads = store.upload_set();
        let request = UploadBegin {
            tab_id: TabId::new(),
            attachment_id: AttachmentId::new(),
            submission_id: Uuid::new_v4().hyphenated().to_string(),
            submission_count: 1,
            submission_bytes: jpeg.len() as u64,
            length: jpeg.len() as u64,
            sha256: Sha256::digest(&jpeg).into(),
        };
        let began = uploads.begin_at(Some(&cwd), request, UNIX_EPOCH).unwrap();
        uploads.chunk(&began.upload_id, 0, &jpeg).unwrap();
        let mut upload = uploads.uploads.remove(&began.upload_id).unwrap();
        let part = upload.staged.directory.path.join(&upload.staged.part_name);
        let published = upload
            .staged
            .directory
            .path
            .join(&upload.staged.published_name);

        let error = finish_upload_with_hook(&mut upload, UNIX_EPOCH, |observed| {
            if observed == boundary {
                Err(injected_crash())
            } else {
                Ok(())
            }
        })
        .unwrap_err();
        assert_eq!(error.kind(), UploadErrorKind::Storage);
        upload.staged.published = true;
        drop(upload);
        drop(uploads);
        (root, part, published, store)
    }

    #[test]
    fn cancellation_at_the_prepublication_boundary_leaves_no_final_attachment() {
        let root =
            std::env::temp_dir().join(format!("aiterm-publication-cancel-{}", Uuid::new_v4()));
        let cwd = root.join("project");
        let cache = root.join("cache");
        fs::create_dir_all(&cwd).unwrap();
        let store = AttachmentStore::new_at(cache.clone(), UNIX_EPOCH).unwrap();
        let jpeg = normalized_test_jpeg();
        let mut uploads = store.upload_set();
        let request = UploadBegin {
            tab_id: TabId::new(),
            attachment_id: AttachmentId::new(),
            submission_id: Uuid::new_v4().hyphenated().to_string(),
            submission_count: 1,
            submission_bytes: jpeg.len() as u64,
            length: jpeg.len() as u64,
            sha256: Sha256::digest(&jpeg).into(),
        };
        let began = uploads.begin_at(Some(&cwd), request, UNIX_EPOCH).unwrap();
        uploads.chunk(&began.upload_id, 0, &jpeg).unwrap();
        let mut upload = uploads.uploads.remove(&began.upload_id).unwrap();
        let published = upload
            .staged
            .directory
            .path
            .join(&upload.staged.published_name);
        let cancelled = Arc::new(AtomicBool::new(false));
        let boundary_reached = Arc::new(std::sync::Barrier::new(2));
        let release = Arc::new(std::sync::Barrier::new(2));
        let signal = std::thread::spawn({
            let cancelled = cancelled.clone();
            let boundary_reached = boundary_reached.clone();
            let release = release.clone();
            move || {
                boundary_reached.wait();
                cancelled.store(true, Ordering::Release);
                release.wait();
            }
        });

        let error =
            finish_upload_with_hook_cancellable(&mut upload, UNIX_EPOCH, &cancelled, |observed| {
                if observed == PublicationBoundary::IntentPersisted {
                    boundary_reached.wait();
                    release.wait();
                }
                Ok(())
            })
            .unwrap_err();
        signal.join().unwrap();

        assert_eq!(error.kind(), UploadErrorKind::Cancelled);
        assert!(!published.exists());
        assert_eq!(manifest_record_count(&cache), 0);
        drop(upload);
        drop(uploads);
        store.maintain(UNIX_EPOCH).unwrap();
        assert!(!published.exists());
        assert_eq!(manifest_record_count(&cache), 0);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn publication_crash_boundaries_expire_every_partial_intent_or_link() {
        for boundary in [
            PublicationBoundary::PartUnlinked,
            PublicationBoundary::IntentPersisted,
            PublicationBoundary::PublishedLinked,
        ] {
            let (root, part, published, store) = publication_crash_fixture(boundary);
            assert!(!part.exists());
            assert_eq!(
                published.exists(),
                boundary == PublicationBoundary::PublishedLinked
            );
            assert_eq!(manifest_record_count(&root.join("cache")), 1);

            store
                .maintain(UNIX_EPOCH + PARTIAL_ATTACHMENT_TTL + Duration::from_secs(1))
                .unwrap();

            assert!(!part.exists());
            assert!(!published.exists());
            assert_eq!(manifest_record_count(&root.join("cache")), 0);
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn crash_after_complete_manifest_keeps_the_file_for_the_completed_ttl() {
        let (root, part, published, store) =
            publication_crash_fixture(PublicationBoundary::CompletePersisted);
        assert!(!part.exists());
        assert!(published.exists());

        store
            .maintain(UNIX_EPOCH + PARTIAL_ATTACHMENT_TTL + Duration::from_secs(1))
            .unwrap();
        assert!(published.exists());
        store
            .maintain(UNIX_EPOCH + ATTACHMENT_TTL + Duration::from_secs(1))
            .unwrap();
        assert!(!published.exists());
        assert_eq!(manifest_record_count(&root.join("cache")), 0);
        fs::remove_dir_all(root).unwrap();
    }

    fn wait_for_path_for(path: &Path, duration: Duration) -> bool {
        let deadline = std::time::Instant::now() + duration;
        while std::time::Instant::now() < deadline {
            if path.exists() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        path.exists()
    }

    #[test]
    fn subprocess_writer_reaches_the_production_lock_boundary_but_not_the_inside_while_held() {
        use std::process::{Command, Stdio};

        let root =
            std::env::temp_dir().join(format!("aiterm-manifest-lock-boundary-{}", Uuid::new_v4()));
        let cache = root.join("cache");
        let ready = root.join("holder-ready");
        let release = root.join("holder-release");
        let before = root.join("writer-before-flock");
        let allow = root.join("writer-allow-flock");
        let acquired = root.join("writer-acquired-flock");
        fs::create_dir_all(&root).unwrap();
        AttachmentStore::new(cache.clone()).unwrap();
        let executable = std::env::current_exe().unwrap();
        let lock = cache.join("remote-attachments/.attachments.lock");
        let holder = Command::new(&executable)
            .arg("--exact")
            .arg("remote::uploads::tests::manifest_lock_boundary_holder_helper")
            .arg("--ignored")
            .env("AITERM_UPLOAD_UNIT_LOCK_PATH", &lock)
            .env("AITERM_UPLOAD_UNIT_LOCK_READY", &ready)
            .env("AITERM_UPLOAD_UNIT_LOCK_RELEASE", &release)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        assert!(wait_for_path_for(&ready, Duration::from_secs(5)));
        let writer = Command::new(&executable)
            .arg("--exact")
            .arg("remote::uploads::tests::manifest_lock_boundary_writer_helper")
            .arg("--ignored")
            .env("AITERM_UPLOAD_UNIT_LOCK_CACHE", &cache)
            .env("AITERM_UPLOAD_UNIT_LOCK_BEFORE", &before)
            .env("AITERM_UPLOAD_UNIT_LOCK_ALLOW", &allow)
            .env("AITERM_UPLOAD_UNIT_LOCK_ACQUIRED", &acquired)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();

        let reached_boundary = wait_for_path_for(&before, Duration::from_secs(5));
        fs::write(&allow, b"enter flock").unwrap();
        let bypassed_lock = wait_for_path_for(&acquired, Duration::from_millis(500));
        fs::write(&release, b"release flock").unwrap();
        let holder_output = holder.wait_with_output().unwrap();
        let writer_output = writer.wait_with_output().unwrap();

        assert!(
            reached_boundary,
            "writer never reached the production manifest lock boundary: {}",
            String::from_utf8_lossy(&writer_output.stderr)
        );
        assert!(
            !bypassed_lock,
            "writer crossed the production manifest lock boundary while another process held it"
        );
        assert!(
            holder_output.status.success(),
            "lock holder failed: {}",
            String::from_utf8_lossy(&holder_output.stderr)
        );
        assert!(
            writer_output.status.success(),
            "lock writer failed: {}",
            String::from_utf8_lossy(&writer_output.stderr)
        );
        assert!(acquired.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[ignore = "subprocess helper for the production manifest lock-boundary test"]
    fn manifest_lock_boundary_holder_helper() {
        use std::os::fd::AsRawFd;

        let Some(lock_path) = std::env::var_os("AITERM_UPLOAD_UNIT_LOCK_PATH") else {
            return;
        };
        let ready = PathBuf::from(std::env::var_os("AITERM_UPLOAD_UNIT_LOCK_READY").unwrap());
        let release = PathBuf::from(std::env::var_os("AITERM_UPLOAD_UNIT_LOCK_RELEASE").unwrap());
        let lock = File::options()
            .read(true)
            .write(true)
            .open(lock_path)
            .unwrap();
        assert_eq!(unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) }, 0);
        fs::write(ready, b"locked").unwrap();
        assert!(wait_for_path_for(&release, Duration::from_secs(10)));
        assert_eq!(unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_UN) }, 0);
    }

    #[test]
    #[ignore = "subprocess helper for the production manifest lock-boundary test"]
    fn manifest_lock_boundary_writer_helper() {
        let Some(cache) = std::env::var_os("AITERM_UPLOAD_UNIT_LOCK_CACHE") else {
            return;
        };
        AttachmentStore::new(PathBuf::from(cache)).unwrap();
    }
}
