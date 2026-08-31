use crate::tabs::{AttachmentId, TabId};
use image::{ImageFormat, ImageReader};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
#[cfg(unix)]
use std::ffi::CString;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs::{self, File};
use std::io::{BufReader, Read, Seek, SeekFrom, Write};
#[cfg(unix)]
use std::os::fd::{AsRawFd, FromRawFd};
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

pub const MAX_UPLOAD_BYTES: u64 = 12 * 1024 * 1024;
pub const MAX_UPLOAD_CHUNK_BYTES: usize = 256 * 1024;
pub const MAX_IMAGE_EDGE: u32 = 4096;
pub const MAX_UPLOADS_PER_SUBMISSION: usize = 4;
pub const MAX_SUBMISSION_BYTES: u64 = 48 * 1024 * 1024;
pub const ATTACHMENT_TTL: Duration = Duration::from_secs(24 * 60 * 60);
pub const ATTACHMENT_BUDGET_BYTES: u64 = 256 * 1024 * 1024;
pub const MAX_CLOSED_SUBMISSIONS: usize = 64;

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

    fn reader(&self) -> Result<BufReader<File>, UploadError> {
        let mut file = self
            .file
            .try_clone()
            .map_err(|error| UploadError::storage("clone staged attachment", error))?;
        file.seek(SeekFrom::Start(0))
            .map_err(|error| UploadError::storage("rewind staged attachment", error))?;
        Ok(BufReader::new(file))
    }

    fn has_jpeg_end_marker(&self) -> Result<bool, UploadError> {
        let mut file = self
            .file
            .try_clone()
            .map_err(|error| UploadError::storage("clone staged attachment", error))?;
        if file
            .metadata()
            .map_err(|error| UploadError::storage("inspect staged attachment tail", error))?
            .len()
            < 2
        {
            return Ok(false);
        }
        file.seek(SeekFrom::End(-2))
            .map_err(|error| UploadError::storage("seek staged attachment tail", error))?;
        let mut marker = [0_u8; 2];
        file.read_exact(&mut marker)
            .map_err(|error| UploadError::storage("read staged attachment tail", error))?;
        Ok(marker == [0xff, 0xd9])
    }

    fn publish(&mut self) -> Result<PathBuf, UploadError> {
        self.verify_bound_entry()?;
        self.directory
            .rename_noreplace(&self.part_name, &self.published_name)?;
        if let Err(error) = self
            .directory
            .sync()
            .and_then(|_| self.directory.verify_current_path())
        {
            let _ = self.directory.unlink(&self.published_name);
            let _ = self.directory.sync();
            return Err(error);
        }
        self.published = true;
        Ok(self.directory.path.join(&self.published_name))
    }
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
    let actual_digest: [u8; 32] = upload.hasher.clone().finalize().into();
    if actual_digest != upload.declared_digest {
        return Err(UploadError::new(
            UploadErrorKind::DigestMismatch,
            "attachment SHA-256 does not match its declaration",
        ));
    }

    upload.staged.verify_bound_entry()?;
    let dimensions = ImageReader::with_format(upload.staged.reader()?, ImageFormat::Jpeg)
        .into_dimensions()
        .map_err(|_| {
            UploadError::new(
                UploadErrorKind::InvalidImage,
                "attachment is not a complete JPEG image",
            )
        })?;
    if dimensions.0 == 0
        || dimensions.1 == 0
        || dimensions.0 > MAX_IMAGE_EDGE
        || dimensions.1 > MAX_IMAGE_EDGE
    {
        return Err(UploadError::new(
            UploadErrorKind::InvalidImage,
            "JPEG dimensions must be between 1 and 4096 pixels",
        ));
    }

    ImageReader::with_format(upload.staged.reader()?, ImageFormat::Jpeg)
        .decode()
        .map_err(|_| {
            UploadError::new(
                UploadErrorKind::InvalidImage,
                "attachment is not a complete JPEG image",
            )
        })?;
    if !upload.staged.has_jpeg_end_marker()? {
        return Err(UploadError::new(
            UploadErrorKind::InvalidImage,
            "attachment JPEG stream is truncated",
        ));
    }

    let path = upload.staged.publish()?;
    Ok(PublishedUpload { path })
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

    #[cfg(unix)]
    fn open_file(&self, name: &OsStr) -> std::io::Result<File> {
        let name = CString::new(name.as_bytes())
            .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
        let descriptor = unsafe {
            libc::openat(
                self.file.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
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
    ) -> Result<(Vec<u8>, Option<FileIdentity>), UploadError> {
        let mut file = match self.open_file(name) {
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
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)
            .map_err(|error| UploadError::storage("read stable optional file", error))?;
        Ok((bytes, Some(identity)))
    }

    #[cfg(not(unix))]
    fn read_optional_file(&self, _name: &OsStr) -> Result<(Vec<u8>, Option<()>), UploadError> {
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

    #[cfg(unix)]
    fn rename_noreplace(&self, old: &OsStr, new: &OsStr) -> Result<(), UploadError> {
        let old = cstring(old, "old file name")?;
        let new = cstring(new, "new file name")?;
        #[cfg(target_os = "linux")]
        let result = unsafe {
            libc::renameat2(
                self.file.as_raw_fd(),
                old.as_ptr(),
                self.file.as_raw_fd(),
                new.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        };
        #[cfg(not(target_os = "linux"))]
        let result = unsafe {
            libc::renameat(
                self.file.as_raw_fd(),
                old.as_ptr(),
                self.file.as_raw_fd(),
                new.as_ptr(),
            )
        };
        if result == 0 {
            Ok(())
        } else {
            Err(path_operation_error(
                "publish stable attachment",
                std::io::Error::last_os_error(),
            ))
        }
    }

    #[cfg(not(unix))]
    fn rename_noreplace(&self, _old: &OsStr, _new: &OsStr) -> Result<(), UploadError> {
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
    let common = StableDirectory::open_existing_absolute(&commondir)?;
    let info = common.open_or_create_child(OsStr::new("info"), false)?;
    let (mut bytes, original_identity) = info.read_optional_file(OsStr::new("exclude"))?;

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
