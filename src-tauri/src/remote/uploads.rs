use crate::tabs::{AttachmentId, TabId};
use image::{ImageFormat, ImageReader};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;
use uuid::Uuid;

pub const MAX_UPLOAD_BYTES: u64 = 12 * 1024 * 1024;
pub const MAX_UPLOAD_CHUNK_BYTES: usize = 256 * 1024;
pub const MAX_IMAGE_EDGE: u32 = 4096;
pub const MAX_UPLOADS_PER_SUBMISSION: usize = 4;
pub const MAX_SUBMISSION_BYTES: u64 = 48 * 1024 * 1024;
pub const ATTACHMENT_TTL: Duration = Duration::from_secs(24 * 60 * 60);
pub const ATTACHMENT_BUDGET_BYTES: u64 = 256 * 1024 * 1024;

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
    fallback_cache: PathBuf,
}

impl AttachmentStore {
    /// Create a store whose fallback path is used for tabs without a known cwd.
    ///
    /// The caller supplies AITerm's cache path, which is canonicalized once and
    /// kept owner-only. Project paths are always supplied separately at begin
    /// time from the authoritative tab registry.
    pub fn new(fallback_cache: PathBuf) -> Result<Self, UploadError> {
        ensure_secure_directory(&fallback_cache)?;
        let fallback_cache = fs::canonicalize(&fallback_cache)
            .map_err(|error| UploadError::storage("canonicalize attachment cache", error))?;
        Ok(Self { fallback_cache })
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
            submissions: HashMap::new(),
            closed_submissions: HashSet::new(),
        }
    }

    fn stage(&self, tab_cwd: Option<&Path>) -> Result<StagedFile, UploadError> {
        let (directory, project_cwd) = match tab_cwd {
            Some(cwd) => {
                let canonical = canonical_project_cwd(cwd)?;
                let attachments = project_attachment_directory(&canonical)?;
                update_local_git_exclude(&canonical)?;
                // A replaced cwd must not silently redirect an upload after
                // the path checks above.
                let revalidated = canonical_project_cwd(cwd)?;
                if revalidated != canonical {
                    return Err(UploadError::new(
                        UploadErrorKind::UnsafePath,
                        "tab working directory changed during upload staging",
                    ));
                }
                (attachments, Some(canonical))
            }
            None => {
                revalidate_directory(&self.fallback_cache, &self.fallback_cache)?;
                (self.fallback_cache.clone(), None)
            }
        };

        for _ in 0..8 {
            let basename = Uuid::new_v4().hyphenated().to_string();
            let published_path = directory.join(format!("{basename}.jpg"));
            let part_path = directory.join(format!("{basename}.jpg.part"));
            match create_owner_only_file(&part_path) {
                Ok(file) => {
                    let revalidation = if let Some(cwd) = project_cwd.as_ref() {
                        revalidate_project_tree(cwd, &directory)
                    } else {
                        revalidate_directory(&self.fallback_cache, &directory)
                    };
                    if let Err(error) = revalidation {
                        drop(file);
                        remove_file_if_present(&part_path);
                        return Err(error);
                    }
                    return Ok(StagedFile {
                        part_path,
                        published_path,
                        file,
                    });
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
    submissions: HashMap<String, SubmissionState>,
    closed_submissions: HashSet<String>,
}

impl UploadSet {
    /// Begin one upload using only the cwd resolved by the desktop tab registry.
    pub fn begin(
        &mut self,
        tab_cwd: Option<&Path>,
        request: UploadBegin,
    ) -> Result<UploadBegan, UploadError> {
        if let Err(error) = validate_begin(&request) {
            if self.submissions.contains_key(&request.submission_id) {
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

        let prospective = match self.submissions.get(&request.submission_id) {
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
                    self.closed_submissions
                        .insert(request.submission_id.clone());
                    return Err(UploadError::new(
                        UploadErrorKind::InvalidSubmission,
                        "image declaration does not match the submission total",
                    ));
                }
                (1, request.length)
            }
        };

        let staged = self.store.stage(tab_cwd)?;
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
            .submissions
            .entry(request.submission_id.clone())
            .or_insert_with(|| SubmissionState::new(&request));
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
        if let Some(group) = self.submissions.get_mut(&upload.submission_id) {
            group.active_uploads.remove(upload_id);
        }
        let submission_id = upload.submission_id.clone();

        let result = finish_upload(&mut upload);
        match result {
            Ok(published) => {
                let complete = if let Some(group) = self.submissions.get_mut(&submission_id) {
                    group.finished_count += 1;
                    group.finished_count == group.declared_count as usize
                        && group.begun_count == group.declared_count as usize
                } else {
                    false
                };
                if complete {
                    self.submissions.remove(&submission_id);
                    self.closed_submissions.insert(submission_id);
                }
                Ok(published)
            }
            Err(error) => {
                remove_file_if_present(&upload.staged.part_path);
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
        let submission_ids: Vec<String> = self.submissions.keys().cloned().collect();
        for submission_id in submission_ids {
            self.abort_submission(&submission_id);
        }
    }

    pub fn target(&self, upload_id: &str) -> Option<(&TabId, &AttachmentId)> {
        self.uploads
            .get(upload_id)
            .map(|upload| (&upload.tab_id, &upload.attachment_id))
    }

    fn abort_submission(&mut self, submission_id: &str) {
        if let Some(group) = self.submissions.remove(submission_id) {
            for upload_id in group.active_uploads {
                if let Some(upload) = self.uploads.remove(&upload_id) {
                    remove_file_if_present(&upload.staged.part_path);
                }
            }
        }
        self.closed_submissions.insert(submission_id.to_string());
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
    part_path: PathBuf,
    published_path: PathBuf,
    file: File,
}

struct SubmissionState {
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
    if request.submission_count == 0
        || request.submission_count as usize > MAX_UPLOADS_PER_SUBMISSION
    {
        return Err(UploadError::new(
            UploadErrorKind::TooLarge,
            "submission must contain one to four images",
        ));
    }
    if request.submission_bytes == 0 || request.submission_bytes > MAX_SUBMISSION_BYTES {
        return Err(UploadError::new(
            UploadErrorKind::TooLarge,
            "submission exceeds the 48 MiB limit",
        ));
    }
    if request.length == 0 || request.length > MAX_UPLOAD_BYTES {
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

    let dimensions_file = File::open(&upload.staged.part_path)
        .map_err(|error| UploadError::storage("open staged attachment", error))?;
    let dimensions = ImageReader::with_format(BufReader::new(dimensions_file), ImageFormat::Jpeg)
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

    let decode_file = File::open(&upload.staged.part_path)
        .map_err(|error| UploadError::storage("reopen staged attachment", error))?;
    ImageReader::with_format(BufReader::new(decode_file), ImageFormat::Jpeg)
        .decode()
        .map_err(|_| {
            UploadError::new(
                UploadErrorKind::InvalidImage,
                "attachment is not a complete JPEG image",
            )
        })?;

    fs::rename(&upload.staged.part_path, &upload.staged.published_path)
        .map_err(|error| UploadError::storage("publish attachment", error))?;
    Ok(PublishedUpload {
        path: upload.staged.published_path.clone(),
    })
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

fn project_attachment_directory(canonical_cwd: &Path) -> Result<PathBuf, UploadError> {
    let aiterm = canonical_cwd.join(".aiterm");
    ensure_secure_directory(&aiterm)?;
    let attachments = aiterm.join("attachments");
    ensure_secure_directory(&attachments)?;
    revalidate_project_tree(canonical_cwd, &attachments)?;
    Ok(attachments)
}

fn revalidate_project_tree(cwd: &Path, attachments: &Path) -> Result<(), UploadError> {
    let canonical_cwd = fs::canonicalize(cwd)
        .map_err(|error| UploadError::storage("revalidate tab working directory", error))?;
    if canonical_cwd != cwd {
        return Err(UploadError::new(
            UploadErrorKind::UnsafePath,
            "tab working directory changed while staging",
        ));
    }
    let expected = cwd.join(".aiterm/attachments");
    revalidate_directory(&expected, attachments)
}

fn revalidate_directory(expected: &Path, directory: &Path) -> Result<(), UploadError> {
    let metadata = fs::symlink_metadata(directory)
        .map_err(|error| UploadError::storage("inspect attachment directory", error))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(UploadError::new(
            UploadErrorKind::UnsafePath,
            "attachment path is a symlink or non-directory",
        ));
    }
    let canonical = fs::canonicalize(directory)
        .map_err(|error| UploadError::storage("canonicalize attachment directory", error))?;
    if canonical != expected {
        return Err(UploadError::new(
            UploadErrorKind::UnsafePath,
            "attachment directory escaped its expected location",
        ));
    }
    Ok(())
}

fn ensure_secure_directory(path: &Path) -> Result<(), UploadError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(UploadError::new(
                    UploadErrorKind::UnsafePath,
                    format!("{} is a symlink or non-directory", path.display()),
                ));
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            create_owner_only_directory(path)?;
        }
        Err(error) => return Err(UploadError::storage("inspect attachment path", error)),
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(|error| UploadError::storage("set attachment directory permissions", error))?;
    }
    Ok(())
}

fn create_owner_only_directory(path: &Path) -> Result<(), UploadError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true).mode(0o700);
        builder
            .create(path)
            .map_err(|error| UploadError::storage("create attachment directory", error))?;
    }
    #[cfg(not(unix))]
    fs::create_dir_all(path)
        .map_err(|error| UploadError::storage("create attachment directory", error))?;
    Ok(())
}

fn create_owner_only_file(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

fn update_local_git_exclude(cwd: &Path) -> Result<(), UploadError> {
    let repository = match git2::Repository::discover(cwd) {
        Ok(repository) => repository,
        Err(error) if error.code() == git2::ErrorCode::NotFound => return Ok(()),
        Err(error) => return Err(UploadError::storage("discover Git repository", error)),
    };
    let info = repository.commondir().join("info");
    if let Ok(metadata) = fs::symlink_metadata(&info) {
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(UploadError::new(
                UploadErrorKind::UnsafePath,
                "Git info path is a symlink or non-directory",
            ));
        }
    } else {
        fs::create_dir_all(&info)
            .map_err(|error| UploadError::storage("create Git info directory", error))?;
    }

    let exclude = info.join("exclude");
    let mut bytes = match fs::symlink_metadata(&exclude) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(UploadError::new(
                    UploadErrorKind::UnsafePath,
                    "Git exclude path is a symlink or non-file",
                ));
            }
            let mut file = File::open(&exclude)
                .map_err(|error| UploadError::storage("open local Git exclude", error))?;
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes)
                .map_err(|error| UploadError::storage("read local Git exclude", error))?;
            bytes
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(UploadError::storage("inspect local Git exclude", error)),
    };

    const ENTRY: &[u8] = b".aiterm/attachments/";
    if bytes.split(|byte| *byte == b'\n').any(|line| {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        trim_ascii(line) == ENTRY
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
    bytes.extend_from_slice(ENTRY);
    bytes.extend_from_slice(newline);

    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&exclude)
        .map_err(|error| UploadError::storage("open local Git exclude for update", error))?;
    file.write_all(&bytes)
        .map_err(|error| UploadError::storage("update local Git exclude", error))?;
    file.flush()
        .map_err(|error| UploadError::storage("flush local Git exclude", error))?;
    file.sync_all()
        .map_err(|error| UploadError::storage("sync local Git exclude", error))?;
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

fn remove_file_if_present(path: &Path) {
    if let Err(error) = fs::remove_file(path) {
        if error.kind() != std::io::ErrorKind::NotFound {
            tracing::warn!(path = %path.display(), error = %error, "failed to remove staged attachment");
        }
    }
}

fn upload_not_found() -> UploadError {
    UploadError::new(UploadErrorKind::NotFound, "upload id was not found")
}
