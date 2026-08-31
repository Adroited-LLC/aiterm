use crate::tabs::{AttachmentId, TabId};
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
use std::os::unix::ffi::OsStrExt;
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;
use uuid::Uuid;
use zune_core::options::DecoderOptions;
use zune_jpeg::JpegDecoder;

pub const MAX_UPLOAD_BYTES: u64 = 12 * 1024 * 1024;
pub const MAX_UPLOAD_CHUNK_BYTES: usize = 256 * 1024;
pub const MAX_IMAGE_EDGE: u32 = 4096;
pub const MAX_UPLOADS_PER_SUBMISSION: usize = 4;
pub const MAX_SUBMISSION_BYTES: u64 = 48 * 1024 * 1024;
pub const ATTACHMENT_TTL: Duration = Duration::from_secs(24 * 60 * 60);
pub const ATTACHMENT_BUDGET_BYTES: u64 = 256 * 1024 * 1024;
pub const MAX_CLOSED_SUBMISSIONS: usize = 64;
const MAX_GIT_EXCLUDE_BYTES: u64 = 1024 * 1024;

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
}

impl AttachmentStore {
    /// Create a store whose fallback path is used for tabs without a known cwd.
    ///
    /// The caller supplies AITerm's cache path, which is canonicalized once and
    /// kept owner-only. Project paths are always supplied separately at begin
    /// time from the authoritative tab registry.
    pub fn new(fallback_cache: PathBuf) -> Result<Self, UploadError> {
        let fallback_cache = StableDirectory::open_or_create_tree(&fallback_cache)?;
        fallback_cache.set_owner_only()?;
        Ok(Self {
            fallback_cache: Arc::new(fallback_cache),
        })
    }

    pub fn system() -> Result<Self, UploadError> {
        let cache = dirs::cache_dir()
            .ok_or_else(|| UploadError::new(UploadErrorKind::Storage, "no cache directory"))?
            .join("aiterm/attachments");
        Self::new(cache)
    }

    pub fn upload_set(&self) -> UploadSet {
        UploadSet {
            store: self.clone(),
            uploads: HashMap::new(),
            submission: None,
            closed_submissions: HashSet::new(),
        }
    }

    fn stage(&self, tab_cwd: Option<&Path>) -> Result<StagedFile, UploadError> {
        let directory = match tab_cwd {
            Some(cwd) => {
                let canonical = canonical_project_cwd(cwd)?;
                let attachments = project_attachment_directory(&canonical)?;
                update_local_git_exclude(&canonical)?;
                attachments
            }
            None => self.fallback_cache.duplicate()?,
        };

        for _ in 0..8 {
            let basename = Uuid::new_v4().hyphenated().to_string();
            let published_name = OsString::from(format!("{basename}.jpg"));
            let part_name = OsString::from(format!("{basename}.jpg.part"));
            let staged_directory = directory.duplicate()?;
            match directory.create_new_file(&part_name) {
                Ok(file) => {
                    let staged = StagedFile {
                        directory: staged_directory,
                        part_name,
                        published_name,
                        file,
                        published: false,
                    };
                    staged.verify_bound_entry()?;
                    return Ok(staged);
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    return Err(UploadError::storage("create staged attachment", error));
                }
            }
        }

        Err(UploadError::new(
            UploadErrorKind::Storage,
            "could not allocate a unique attachment name",
        ))
    }
}

pub struct UploadSet {
    store: AttachmentStore,
    uploads: HashMap<String, ActiveUpload>,
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

        let staged = match self.store.stage(tab_cwd) {
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
        let mut upload = self
            .uploads
            .remove(upload_id)
            .ok_or_else(upload_not_found)?;
        if let Some(group) = self.submission.as_mut() {
            group.active_uploads.remove(upload_id);
        }
        let submission_id = upload.submission_id.clone();

        let result = finish_upload(&mut upload);
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

    /// Cancel one upload and the rest of its submission. A partial submission
    /// cannot be resumed with the same id after any member is cancelled.
    pub fn cancel(&mut self, upload_id: &str) -> Result<(), UploadError> {
        let submission_id = self
            .uploads
            .get(upload_id)
            .ok_or_else(upload_not_found)?
            .submission_id
            .clone();
        self.abort_submission(&submission_id);
        Ok(())
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

struct StagedFile {
    directory: StableDirectory,
    part_name: OsString,
    published_name: OsString,
    file: File,
    published: bool,
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

    fn publish(&mut self, validated: &File) -> Result<PathBuf, UploadError> {
        self.publish_with_hook(validated, || {})
    }

    fn publish_with_hook(
        &mut self,
        validated: &File,
        before_link: impl FnOnce(),
    ) -> Result<PathBuf, UploadError> {
        self.verify_bound_entry()?;
        before_link();
        self.directory.verify_current_path()?;
        self.directory
            .link_held_noreplace(validated, &self.published_name)?;
        let result = (|| {
            self.directory
                .verify_entry_identity(&self.published_name, validated)?;
            match self.directory.unlink(&self.part_name) {
                Ok(()) => {}
                Err(error) if error.kind() == UploadErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
            self.directory.sync()?;
            self.directory.verify_current_path()?;
            self.directory
                .verify_entry_identity(&self.published_name, validated)
        })();
        if let Err(error) = result {
            let _ = self.directory.unlink(&self.published_name);
            let _ = self.directory.sync();
            return Err(error);
        }
        self.published = true;
        Ok(self.directory.path.join(&self.published_name))
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
            if let Err(error) = self.directory.unlink(&self.part_name) {
                if error.kind() != UploadErrorKind::NotFound {
                    tracing::warn!(
                        path = %self.directory.path.join(&self.part_name).display(),
                        error = %error,
                        "failed to remove staged attachment"
                    );
                }
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

fn finish_upload(upload: &mut ActiveUpload) -> Result<PublishedUpload, UploadError> {
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

    let path = upload.staged.publish(&publication)?;
    Ok(PublishedUpload { path })
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
                "Git info/exclude exceeds the 1 MiB safety limit",
            ));
        }
        let mut bytes = Vec::new();
        file.take(max_bytes.saturating_add(1))
            .read_to_end(&mut bytes)
            .map_err(|error| UploadError::storage("read stable optional file", error))?;
        if bytes.len() as u64 > max_bytes {
            return Err(UploadError::new(
                UploadErrorKind::Capacity,
                "Git info/exclude exceeds the 1 MiB safety limit",
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

#[cfg(not(unix))]
fn unsupported_stable_filesystem() -> UploadError {
    UploadError::new(
        UploadErrorKind::Storage,
        "stable attachment storage requires Unix descriptor-relative filesystem operations",
    )
}

type ExcludeLockRegistry = Mutex<HashMap<PathBuf, Weak<Mutex<()>>>>;
static EXCLUDE_UPDATE_LOCKS: OnceLock<ExcludeLockRegistry> = OnceLock::new();

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

    #[test]
    fn publication_links_the_held_inode_when_part_name_changes_after_validation() {
        let root = std::env::temp_dir().join(format!("aiterm-held-publication-{}", Uuid::new_v4()));
        let directory = StableDirectory::open_or_create_tree(&root).unwrap();
        let part_name = OsString::from("attachment.jpg.part");
        let published_name = OsString::from("attachment.jpg");
        let mut file = directory.create_new_file(&part_name).unwrap();
        let original = b"held inode contents";
        file.write_all(original).unwrap();
        file.sync_all().unwrap();
        let part_path = root.join(&part_name);
        let displaced_path = root.join("displaced-original");
        let replacement = b"replacement pathname contents";
        let mut publication = directory.create_anonymous_file().unwrap();
        publication.write_all(original).unwrap();
        publication.sync_all().unwrap();
        let mut staged = StagedFile {
            directory,
            part_name,
            published_name,
            file,
            published: false,
        };

        let published = staged
            .publish_with_hook(&publication, || {
                fs::rename(&part_path, &displaced_path).unwrap();
                fs::write(&part_path, replacement).unwrap();
            })
            .unwrap();

        assert_eq!(fs::read(published).unwrap(), original);
        assert!(!part_path.exists());
        fs::remove_file(displaced_path).unwrap();
        fs::remove_dir_all(root).unwrap();
    }
}
