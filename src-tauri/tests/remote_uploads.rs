use aiterm_lib::remote::uploads::{
    AttachmentStore, UploadBegin, UploadErrorKind, UploadSet, MAX_CLOSED_SUBMISSIONS,
    MAX_SUBMISSION_BYTES, MAX_UPLOAD_BYTES, MAX_UPLOAD_CHUNK_BYTES,
};
use aiterm_lib::tabs::{AttachmentId, TabId};
use image::{DynamicImage, ImageBuffer, ImageFormat, Rgb};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};

struct UploadFixture {
    root: PathBuf,
    cwd: PathBuf,
    cache: PathBuf,
    uploads: UploadSet,
    tab_id: TabId,
    attachment_id: AttachmentId,
}

impl UploadFixture {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "aiterm-remote-upload-{name}-{}",
            uuid::Uuid::new_v4()
        ));
        let cwd = root.join("project");
        let cache = root.join("cache");
        fs::create_dir_all(&cwd).unwrap();
        let store = AttachmentStore::new(cache.clone()).unwrap();
        let uploads = store.upload_set();
        Self {
            root,
            cwd,
            cache,
            uploads,
            tab_id: TabId::new(),
            attachment_id: AttachmentId::new(),
        }
    }

    fn jpeg(&self, width: u32, height: u32) -> Vec<u8> {
        let image = ImageBuffer::from_pixel(width, height, Rgb([19_u8, 71_u8, 113_u8]));
        let mut bytes = Cursor::new(Vec::new());
        DynamicImage::ImageRgb8(image)
            .write_to(&mut bytes, ImageFormat::Jpeg)
            .unwrap();
        bytes.into_inner()
    }

    fn multi_chunk_jpeg(&self) -> Vec<u8> {
        let image = ImageBuffer::from_fn(1024, 768, |x, y| {
            let mixed = x
                .wrapping_mul(747_796_405)
                .wrapping_add(y.wrapping_mul(2_891_336_453));
            Rgb([
                mixed as u8,
                mixed.rotate_left(11) as u8,
                mixed.rotate_left(21) as u8,
            ])
        });
        let mut bytes = Cursor::new(Vec::new());
        DynamicImage::ImageRgb8(image)
            .write_to(&mut bytes, ImageFormat::Jpeg)
            .unwrap();
        let bytes = bytes.into_inner();
        assert!(bytes.len() > MAX_UPLOAD_CHUNK_BYTES);
        bytes
    }

    fn begin(&self, length: usize, digest: [u8; 32]) -> UploadBegin {
        self.begin_for_submission(
            &uuid::Uuid::new_v4().to_string(),
            1,
            length as u64,
            length,
            digest,
        )
    }

    fn begin_for_submission(
        &self,
        submission_id: &str,
        submission_count: u8,
        submission_bytes: u64,
        length: usize,
        digest: [u8; 32],
    ) -> UploadBegin {
        UploadBegin {
            tab_id: self.tab_id.clone(),
            attachment_id: self.attachment_id.clone(),
            submission_id: submission_id.to_string(),
            submission_count,
            submission_bytes,
            length: length as u64,
            sha256: digest,
        }
    }

    fn part_files(&self) -> Vec<PathBuf> {
        files_with_extension(&self.cwd.join(".aiterm/attachments"), "part")
    }
}

impl Drop for UploadFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn files_with_extension(directory: &Path, extension: &str) -> Vec<PathBuf> {
    fs::read_dir(directory)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some(extension))
        .collect()
}

fn digest(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

fn publish(fixture: &mut UploadFixture, jpeg: &[u8]) -> PathBuf {
    let request = fixture.begin(jpeg.len(), digest(jpeg));
    let began = fixture.uploads.begin(Some(&fixture.cwd), request).unwrap();
    for (index, chunk) in jpeg.chunks(MAX_UPLOAD_CHUNK_BYTES).enumerate() {
        fixture
            .uploads
            .chunk(&began.upload_id, index as u32, chunk)
            .unwrap();
    }
    fixture.uploads.finish(&began.upload_id).unwrap().path
}

#[test]
fn ordered_jpeg_chunks_publish_atomically_under_the_tab_cwd() {
    let mut fixture = UploadFixture::new("publish");
    let jpeg = fixture.multi_chunk_jpeg();
    let request = fixture.begin(jpeg.len(), digest(&jpeg));
    let began = fixture.uploads.begin(Some(&fixture.cwd), request).unwrap();
    let part = fixture.part_files().pop().unwrap();
    let final_path = part.parent().unwrap().join(part.file_stem().unwrap());
    assert!(part.exists());
    assert!(!final_path.exists());
    for (index, chunk) in jpeg.chunks(MAX_UPLOAD_CHUNK_BYTES).enumerate() {
        fixture
            .uploads
            .chunk(&began.upload_id, index as u32, chunk)
            .unwrap();
        assert!(!final_path.exists());
    }
    let published = fixture.uploads.finish(&began.upload_id).unwrap().path;

    assert!(published.starts_with(fixture.cwd.join(".aiterm/attachments")));
    assert_eq!(published, final_path);
    assert_eq!(fs::read(&published).unwrap(), jpeg);
    assert_eq!(
        published.extension().and_then(|value| value.to_str()),
        Some("jpg")
    );
    assert!(fixture.part_files().is_empty());
}

#[test]
fn an_out_of_order_or_duplicate_chunk_aborts_and_removes_the_partial_file() {
    for first_index in [1, 0] {
        let mut fixture = UploadFixture::new("chunk-order");
        let jpeg = fixture.jpeg(640, 480);
        let request = fixture.begin(jpeg.len(), digest(&jpeg));
        let began = fixture.uploads.begin(Some(&fixture.cwd), request).unwrap();
        let first = &jpeg[..jpeg.len().min(64)];
        if first_index == 0 {
            fixture.uploads.chunk(&began.upload_id, 0, first).unwrap();
        }

        let error = fixture
            .uploads
            .chunk(&began.upload_id, first_index, first)
            .unwrap_err();

        assert_eq!(error.kind(), UploadErrorKind::OutOfOrder);
        assert!(fixture.part_files().is_empty());
        assert_eq!(
            fixture.uploads.finish(&began.upload_id).unwrap_err().kind(),
            UploadErrorKind::NotFound
        );
    }
}

#[test]
fn declared_and_actual_length_mismatch_aborts_publication() {
    let mut fixture = UploadFixture::new("length-mismatch");
    let jpeg = fixture.jpeg(64, 48);
    let request = fixture.begin(jpeg.len() + 1, digest(&jpeg));
    let began = fixture.uploads.begin(Some(&fixture.cwd), request).unwrap();
    fixture.uploads.chunk(&began.upload_id, 0, &jpeg).unwrap();

    let error = fixture.uploads.finish(&began.upload_id).unwrap_err();

    assert_eq!(error.kind(), UploadErrorKind::LengthMismatch);
    assert!(fixture.part_files().is_empty());
    assert!(files_with_extension(&fixture.cwd.join(".aiterm/attachments"), "jpg").is_empty());
}

#[test]
fn bytes_past_the_declared_length_abort_immediately() {
    let mut fixture = UploadFixture::new("extra-bytes");
    let jpeg = fixture.jpeg(64, 48);
    let request = fixture.begin(jpeg.len() - 1, digest(&jpeg));
    let began = fixture.uploads.begin(Some(&fixture.cwd), request).unwrap();

    let error = fixture
        .uploads
        .chunk(&began.upload_id, 0, &jpeg)
        .unwrap_err();

    assert_eq!(error.kind(), UploadErrorKind::LengthMismatch);
    assert!(fixture.part_files().is_empty());
}

#[test]
fn digest_mismatch_aborts_publication() {
    let mut fixture = UploadFixture::new("digest-mismatch");
    let jpeg = fixture.jpeg(64, 48);
    let request = fixture.begin(jpeg.len(), [0_u8; 32]);
    let began = fixture.uploads.begin(Some(&fixture.cwd), request).unwrap();
    fixture.uploads.chunk(&began.upload_id, 0, &jpeg).unwrap();

    let error = fixture.uploads.finish(&began.upload_id).unwrap_err();

    assert_eq!(error.kind(), UploadErrorKind::DigestMismatch);
    assert!(fixture.part_files().is_empty());
}

#[test]
fn non_jpeg_stream_is_rejected_and_removed() {
    let mut fixture = UploadFixture::new("not-jpeg");
    let bytes = b"this is not a jpeg";
    let request = fixture.begin(bytes.len(), digest(bytes));
    let began = fixture.uploads.begin(Some(&fixture.cwd), request).unwrap();
    fixture.uploads.chunk(&began.upload_id, 0, bytes).unwrap();

    let error = fixture.uploads.finish(&began.upload_id).unwrap_err();

    assert_eq!(error.kind(), UploadErrorKind::InvalidImage);
    assert!(fixture.part_files().is_empty());
}

#[test]
fn jpeg_with_parseable_dimensions_but_truncated_scan_data_is_rejected() {
    let mut fixture = UploadFixture::new("truncated-jpeg");
    let complete = fixture.multi_chunk_jpeg();
    let retained = complete.len() / 2;
    let truncated = &complete[..retained];
    assert!(
        image::ImageReader::with_format(Cursor::new(truncated), ImageFormat::Jpeg)
            .into_dimensions()
            .is_ok()
    );
    let request = fixture.begin(truncated.len(), digest(truncated));
    let began = fixture.uploads.begin(Some(&fixture.cwd), request).unwrap();
    for (index, chunk) in truncated.chunks(MAX_UPLOAD_CHUNK_BYTES).enumerate() {
        fixture
            .uploads
            .chunk(&began.upload_id, index as u32, chunk)
            .unwrap();
    }

    let error = fixture.uploads.finish(&began.upload_id).unwrap_err();

    assert_eq!(error.kind(), UploadErrorKind::InvalidImage);
    assert!(fixture.part_files().is_empty());
}

#[test]
fn image_edge_above_4096_pixels_is_rejected() {
    let mut fixture = UploadFixture::new("oversize-dimensions");
    let jpeg = fixture.jpeg(4097, 1);
    let request = fixture.begin(jpeg.len(), digest(&jpeg));
    let began = fixture.uploads.begin(Some(&fixture.cwd), request).unwrap();
    fixture.uploads.chunk(&began.upload_id, 0, &jpeg).unwrap();

    let error = fixture.uploads.finish(&began.upload_id).unwrap_err();

    assert_eq!(error.kind(), UploadErrorKind::InvalidImage);
    assert!(fixture.part_files().is_empty());
}

#[test]
fn declaration_above_twelve_mib_is_rejected_before_staging() {
    let mut fixture = UploadFixture::new("oversize-file");
    let request = fixture.begin((MAX_UPLOAD_BYTES + 1) as usize, [0_u8; 32]);

    let error = fixture
        .uploads
        .begin(Some(&fixture.cwd), request)
        .unwrap_err();

    assert_eq!(error.kind(), UploadErrorKind::TooLarge);
    assert!(!fixture.cwd.join(".aiterm").exists());
}

#[test]
fn zero_count_length_and_submission_total_are_invalid_declarations() {
    let mut fixture = UploadFixture::new("zero-declarations");
    for request in [
        fixture.begin_for_submission("zero-count", 0, 1, 1, [0_u8; 32]),
        fixture.begin_for_submission("zero-total", 1, 0, 1, [0_u8; 32]),
        fixture.begin_for_submission("zero-length", 1, 1, 0, [0_u8; 32]),
    ] {
        assert_eq!(
            fixture
                .uploads
                .begin(Some(&fixture.cwd), request)
                .unwrap_err()
                .kind(),
            UploadErrorKind::InvalidSubmission
        );
    }
    assert!(!fixture.cwd.join(".aiterm").exists());
}

#[test]
fn exact_submission_file_count_and_byte_boundaries_are_accepted() {
    let mut fixture = UploadFixture::new("exact-submission-boundaries");
    let submission = uuid::Uuid::new_v4().to_string();
    let mut first_upload = None;
    for _ in 0..4 {
        let request = fixture.begin_for_submission(
            &submission,
            4,
            MAX_SUBMISSION_BYTES,
            MAX_UPLOAD_BYTES as usize,
            [0_u8; 32],
        );
        let began = fixture.uploads.begin(Some(&fixture.cwd), request).unwrap();
        first_upload.get_or_insert(began.upload_id);
    }

    assert_eq!(fixture.part_files().len(), 4);
    fixture
        .uploads
        .cancel(first_upload.as_deref().unwrap())
        .unwrap();
    assert!(fixture.part_files().is_empty());
}

#[test]
fn exact_4096_pixel_edge_is_accepted() {
    let mut fixture = UploadFixture::new("exact-image-edge");
    let jpeg = fixture.jpeg(4096, 1);

    let published = publish(&mut fixture, &jpeg);

    assert_eq!(fs::read(published).unwrap(), jpeg);
}

#[test]
fn exact_chunk_limit_is_accepted_but_oversize_and_empty_chunks_abort() {
    let mut fixture = UploadFixture::new("chunk-boundaries");
    let exact = vec![7_u8; MAX_UPLOAD_CHUNK_BYTES];
    let exact_request = fixture.begin(exact.len(), digest(&exact));
    let exact_began = fixture
        .uploads
        .begin(Some(&fixture.cwd), exact_request)
        .unwrap();
    assert_eq!(
        fixture
            .uploads
            .chunk(&exact_began.upload_id, 0, &exact)
            .unwrap(),
        1
    );
    fixture.uploads.cancel(&exact_began.upload_id).unwrap();

    let oversized = vec![9_u8; MAX_UPLOAD_CHUNK_BYTES + 1];
    let oversized_request = fixture.begin(oversized.len(), digest(&oversized));
    let oversized_began = fixture
        .uploads
        .begin(Some(&fixture.cwd), oversized_request)
        .unwrap();
    assert_eq!(
        fixture
            .uploads
            .chunk(&oversized_began.upload_id, 0, &oversized)
            .unwrap_err()
            .kind(),
        UploadErrorKind::TooLarge
    );

    let jpeg = fixture.jpeg(64, 48);
    let empty_request = fixture.begin(jpeg.len(), digest(&jpeg));
    let empty_began = fixture
        .uploads
        .begin(Some(&fixture.cwd), empty_request)
        .unwrap();
    assert_eq!(
        fixture
            .uploads
            .chunk(&empty_began.upload_id, 0, b"")
            .unwrap_err()
            .kind(),
        UploadErrorKind::LengthMismatch
    );
    assert!(fixture.part_files().is_empty());
}

#[cfg(unix)]
#[test]
fn symlinked_aiterm_directory_is_rejected_without_touching_its_target() {
    use std::os::unix::fs::symlink;

    let mut fixture = UploadFixture::new("symlink");
    let outside = fixture.root.join("outside");
    fs::create_dir(&outside).unwrap();
    symlink(&outside, fixture.cwd.join(".aiterm")).unwrap();
    let jpeg = fixture.jpeg(64, 48);
    let request = fixture.begin(jpeg.len(), digest(&jpeg));

    let error = fixture
        .uploads
        .begin(Some(&fixture.cwd), request)
        .unwrap_err();

    assert_eq!(error.kind(), UploadErrorKind::UnsafePath);
    assert!(fs::read_dir(outside).unwrap().next().is_none());
}

#[cfg(unix)]
#[test]
fn symlinked_attachments_directory_is_rejected_without_touching_its_target() {
    use std::os::unix::fs::symlink;

    let mut fixture = UploadFixture::new("attachments-symlink");
    let outside = fixture.root.join("outside");
    fs::create_dir(&outside).unwrap();
    fs::create_dir(fixture.cwd.join(".aiterm")).unwrap();
    symlink(&outside, fixture.cwd.join(".aiterm/attachments")).unwrap();
    let jpeg = fixture.jpeg(64, 48);
    let request = fixture.begin(jpeg.len(), digest(&jpeg));

    let error = fixture
        .uploads
        .begin(Some(&fixture.cwd), request)
        .unwrap_err();

    assert_eq!(error.kind(), UploadErrorKind::UnsafePath);
    assert!(fs::read_dir(outside).unwrap().next().is_none());
}

#[cfg(unix)]
#[test]
fn tab_cwd_is_canonicalized_before_server_derived_staging() {
    use std::os::unix::fs::symlink;

    let mut fixture = UploadFixture::new("canonical-cwd");
    let cwd_link = fixture.root.join("project-link");
    symlink(&fixture.cwd, &cwd_link).unwrap();
    let jpeg = fixture.jpeg(64, 48);
    let request = fixture.begin(jpeg.len(), digest(&jpeg));

    let began = fixture.uploads.begin(Some(&cwd_link), request).unwrap();
    fixture.uploads.chunk(&began.upload_id, 0, &jpeg).unwrap();
    let published = fixture.uploads.finish(&began.upload_id).unwrap();

    assert!(published
        .path
        .starts_with(fixture.cwd.join(".aiterm/attachments")));
    assert!(!cwd_link
        .join(".aiterm")
        .symlink_metadata()
        .unwrap()
        .file_type()
        .is_symlink());
}

#[test]
fn unknown_upload_id_is_rejected() {
    let mut fixture = UploadFixture::new("unknown");

    assert_eq!(
        fixture
            .uploads
            .chunk("missing", 0, b"bytes")
            .unwrap_err()
            .kind(),
        UploadErrorKind::NotFound
    );
    assert_eq!(
        fixture.uploads.finish("missing").unwrap_err().kind(),
        UploadErrorKind::NotFound
    );
    assert_eq!(
        fixture.uploads.cancel("missing").unwrap_err().kind(),
        UploadErrorKind::NotFound
    );
}

#[test]
fn cancellation_removes_the_partial_file_and_closes_the_submission() {
    let mut fixture = UploadFixture::new("cancel");
    let jpeg = fixture.jpeg(64, 48);
    let submission = uuid::Uuid::new_v4().to_string();
    let request = fixture.begin_for_submission(
        &submission,
        2,
        (jpeg.len() * 2) as u64,
        jpeg.len(),
        digest(&jpeg),
    );
    let began = fixture.uploads.begin(Some(&fixture.cwd), request).unwrap();
    fixture
        .uploads
        .chunk(&began.upload_id, 0, &jpeg[..16])
        .unwrap();

    fixture.uploads.cancel(&began.upload_id).unwrap();

    assert!(fixture.part_files().is_empty());
    let retry = fixture.begin_for_submission(
        &submission,
        2,
        (jpeg.len() * 2) as u64,
        jpeg.len(),
        digest(&jpeg),
    );
    assert_eq!(
        fixture
            .uploads
            .begin(Some(&fixture.cwd), retry)
            .unwrap_err()
            .kind(),
        UploadErrorKind::ClosedSubmission
    );
}

#[test]
fn dropping_an_upload_set_removes_every_partial() {
    let mut fixture = UploadFixture::new("drop-cleanup");
    let jpeg = fixture.jpeg(64, 48);
    let submission = uuid::Uuid::new_v4().to_string();
    for _ in 0..2 {
        let request = fixture.begin_for_submission(
            &submission,
            2,
            (jpeg.len() * 2) as u64,
            jpeg.len(),
            digest(&jpeg),
        );
        fixture.uploads.begin(Some(&fixture.cwd), request).unwrap();
    }
    assert_eq!(fixture.part_files().len(), 2);
    let replacement = AttachmentStore::new(fixture.cache.clone())
        .unwrap()
        .upload_set();
    let uploads = std::mem::replace(&mut fixture.uploads, replacement);

    drop(uploads);

    assert!(fixture.part_files().is_empty());
}

#[test]
fn submission_metadata_and_aggregate_limits_are_enforced_server_side() {
    let mut fixture = UploadFixture::new("submission-limits");
    let jpeg = fixture.jpeg(64, 48);
    let submission = uuid::Uuid::new_v4().to_string();
    let first = fixture.begin_for_submission(
        &submission,
        2,
        (jpeg.len() * 2) as u64,
        jpeg.len(),
        digest(&jpeg),
    );
    fixture.uploads.begin(Some(&fixture.cwd), first).unwrap();
    let inconsistent = fixture.begin_for_submission(
        &submission,
        3,
        (jpeg.len() * 3) as u64,
        jpeg.len(),
        digest(&jpeg),
    );

    assert_eq!(
        fixture
            .uploads
            .begin(Some(&fixture.cwd), inconsistent)
            .unwrap_err()
            .kind(),
        UploadErrorKind::InvalidSubmission
    );
    assert!(fixture.part_files().is_empty());

    let over_count = fixture.begin_for_submission(
        &uuid::Uuid::new_v4().to_string(),
        5,
        jpeg.len() as u64,
        jpeg.len(),
        digest(&jpeg),
    );
    assert_eq!(
        fixture
            .uploads
            .begin(Some(&fixture.cwd), over_count)
            .unwrap_err()
            .kind(),
        UploadErrorKind::TooLarge
    );

    let over_bytes = fixture.begin_for_submission(
        &uuid::Uuid::new_v4().to_string(),
        1,
        MAX_SUBMISSION_BYTES + 1,
        jpeg.len(),
        digest(&jpeg),
    );
    assert_eq!(
        fixture
            .uploads
            .begin(Some(&fixture.cwd), over_bytes)
            .unwrap_err()
            .kind(),
        UploadErrorKind::TooLarge
    );
}

#[test]
fn exact_declared_submission_total_is_required_by_the_final_begin() {
    let mut fixture = UploadFixture::new("submission-total");
    let jpeg = fixture.jpeg(64, 48);
    let submission = uuid::Uuid::new_v4().to_string();
    let first = fixture.begin_for_submission(
        &submission,
        2,
        (jpeg.len() * 2 + 1) as u64,
        jpeg.len(),
        digest(&jpeg),
    );
    fixture.uploads.begin(Some(&fixture.cwd), first).unwrap();
    let second = fixture.begin_for_submission(
        &submission,
        2,
        (jpeg.len() * 2 + 1) as u64,
        jpeg.len(),
        digest(&jpeg),
    );

    assert_eq!(
        fixture
            .uploads
            .begin(Some(&fixture.cwd), second)
            .unwrap_err()
            .kind(),
        UploadErrorKind::InvalidSubmission
    );
    assert!(fixture.part_files().is_empty());
}

#[test]
fn malformed_later_begin_aborts_the_existing_submission_partial() {
    let mut fixture = UploadFixture::new("malformed-later-begin");
    let jpeg = fixture.jpeg(64, 48);
    let submission = uuid::Uuid::new_v4().to_string();
    let first = fixture.begin_for_submission(
        &submission,
        2,
        MAX_SUBMISSION_BYTES,
        jpeg.len(),
        digest(&jpeg),
    );
    fixture.uploads.begin(Some(&fixture.cwd), first).unwrap();
    assert_eq!(fixture.part_files().len(), 1);
    let malformed = fixture.begin_for_submission(
        &submission,
        2,
        MAX_SUBMISSION_BYTES,
        (MAX_UPLOAD_BYTES + 1) as usize,
        [0_u8; 32],
    );

    assert_eq!(
        fixture
            .uploads
            .begin(Some(&fixture.cwd), malformed)
            .unwrap_err()
            .kind(),
        UploadErrorKind::TooLarge
    );
    assert!(fixture.part_files().is_empty());
}

#[test]
fn completed_submission_id_cannot_be_reused() {
    let mut fixture = UploadFixture::new("closed-complete");
    let jpeg = fixture.jpeg(64, 48);
    let submission = uuid::Uuid::new_v4().to_string();
    let request =
        fixture.begin_for_submission(&submission, 1, jpeg.len() as u64, jpeg.len(), digest(&jpeg));
    let began = fixture.uploads.begin(Some(&fixture.cwd), request).unwrap();
    fixture.uploads.chunk(&began.upload_id, 0, &jpeg).unwrap();
    fixture.uploads.finish(&began.upload_id).unwrap();
    let reused =
        fixture.begin_for_submission(&submission, 1, jpeg.len() as u64, jpeg.len(), digest(&jpeg));

    assert_eq!(
        fixture
            .uploads
            .begin(Some(&fixture.cwd), reused)
            .unwrap_err()
            .kind(),
        UploadErrorKind::ClosedSubmission
    );
}

#[test]
fn a_connection_accepts_only_one_active_submission() {
    let mut fixture = UploadFixture::new("one-active-submission");
    let jpeg = fixture.jpeg(64, 48);
    let first_submission = uuid::Uuid::new_v4().to_string();
    let first = fixture.begin_for_submission(
        &first_submission,
        2,
        (jpeg.len() * 2) as u64,
        jpeg.len(),
        digest(&jpeg),
    );
    fixture.uploads.begin(Some(&fixture.cwd), first).unwrap();
    let competing = fixture.begin_for_submission(
        &uuid::Uuid::new_v4().to_string(),
        1,
        jpeg.len() as u64,
        jpeg.len(),
        digest(&jpeg),
    );

    assert_eq!(
        fixture
            .uploads
            .begin(Some(&fixture.cwd), competing)
            .unwrap_err()
            .kind(),
        UploadErrorKind::Busy
    );
    assert_eq!(fixture.part_files().len(), 1);
}

#[test]
fn closed_submission_history_is_bounded_and_then_fails_the_connection_closed() {
    let mut fixture = UploadFixture::new("bounded-closures");
    let jpeg = fixture.jpeg(64, 48);
    for _ in 0..MAX_CLOSED_SUBMISSIONS {
        let request = fixture.begin(jpeg.len(), digest(&jpeg));
        let began = fixture.uploads.begin(Some(&fixture.cwd), request).unwrap();
        fixture.uploads.cancel(&began.upload_id).unwrap();
    }
    let blocked = fixture.begin(jpeg.len(), digest(&jpeg));

    assert_eq!(
        fixture
            .uploads
            .begin(Some(&fixture.cwd), blocked)
            .unwrap_err()
            .kind(),
        UploadErrorKind::Capacity
    );
    assert!(fixture.part_files().is_empty());
}

#[test]
fn project_publication_adds_one_local_git_exclusion_without_changing_line_endings() {
    let mut fixture = UploadFixture::new("git-exclude");
    let repository = git2::Repository::init(&fixture.cwd).unwrap();
    let exclude = repository.commondir().join("info/exclude");
    fs::write(&exclude, b"existing-entry\r\n").unwrap();
    let jpeg = fixture.jpeg(64, 48);

    let first = publish(&mut fixture, &jpeg);
    let second = publish(&mut fixture, &jpeg);

    assert!(first.exists());
    assert!(second.exists());
    assert_eq!(
        fs::read(&exclude).unwrap(),
        b"existing-entry\r\n/.aiterm/attachments/\r\n"
    );
    let statuses = repository.statuses(None).unwrap();
    let dirty: Vec<_> = statuses
        .iter()
        .map(|entry| (entry.path().map(str::to_owned), entry.status()))
        .collect();
    assert!(
        dirty
            .iter()
            .all(|(_, status)| *status == git2::Status::IGNORED),
        "attachment storage dirtied git status: {dirty:?}"
    );
}

#[test]
fn nested_project_publication_uses_an_anchored_repo_relative_git_exclusion() {
    let mut fixture = UploadFixture::new("nested-git-exclude");
    let repository = git2::Repository::init(&fixture.cwd).unwrap();
    let nested = fixture.cwd.join("packages/app");
    fs::create_dir_all(&nested).unwrap();
    let exclude = repository.commondir().join("info/exclude");
    fs::write(&exclude, b"existing-entry\n").unwrap();
    let jpeg = fixture.jpeg(64, 48);
    let request = fixture.begin(jpeg.len(), digest(&jpeg));
    let began = fixture.uploads.begin(Some(&nested), request).unwrap();
    fixture.uploads.chunk(&began.upload_id, 0, &jpeg).unwrap();
    let published = fixture.uploads.finish(&began.upload_id).unwrap();

    assert!(published
        .path
        .starts_with(nested.join(".aiterm/attachments")));
    assert_eq!(
        fs::read(&exclude).unwrap(),
        b"existing-entry\n/packages/app/.aiterm/attachments/\n"
    );
    let statuses = repository.statuses(None).unwrap();
    assert!(statuses
        .iter()
        .all(|entry| entry.status() == git2::Status::IGNORED));
}

#[test]
fn nested_git_exclusion_escapes_pattern_metacharacters_in_path_components() {
    let mut fixture = UploadFixture::new("escaped-git-exclude");
    let repository = git2::Repository::init(&fixture.cwd).unwrap();
    let nested = fixture.cwd.join("pkg [one]#!");
    fs::create_dir_all(&nested).unwrap();
    let exclude = repository.commondir().join("info/exclude");
    fs::write(&exclude, b"").unwrap();
    let jpeg = fixture.jpeg(64, 48);
    let request = fixture.begin(jpeg.len(), digest(&jpeg));
    let began = fixture.uploads.begin(Some(&nested), request).unwrap();
    fixture.uploads.chunk(&began.upload_id, 0, &jpeg).unwrap();
    fixture.uploads.finish(&began.upload_id).unwrap();

    assert_eq!(
        fs::read(&exclude).unwrap(),
        b"/pkg\\ \\[one\\]\\#\\!/.aiterm/attachments/\n"
    );
    let statuses = repository.statuses(None).unwrap();
    assert!(statuses
        .iter()
        .all(|entry| entry.status() == git2::Status::IGNORED));
}

#[cfg(unix)]
#[test]
fn failed_atomic_git_exclude_update_preserves_existing_bytes() {
    use std::os::unix::fs::PermissionsExt;

    let mut fixture = UploadFixture::new("git-exclude-failure");
    let repository = git2::Repository::init(&fixture.cwd).unwrap();
    let info = repository.commondir().join("info");
    let exclude = info.join("exclude");
    let original = b"must-survive\n";
    fs::write(&exclude, original).unwrap();
    fs::set_permissions(&info, fs::Permissions::from_mode(0o500)).unwrap();
    let jpeg = fixture.jpeg(64, 48);
    let request = fixture.begin(jpeg.len(), digest(&jpeg));

    let result = fixture.uploads.begin(Some(&fixture.cwd), request);
    fs::set_permissions(&info, fs::Permissions::from_mode(0o700)).unwrap();

    assert_eq!(result.unwrap_err().kind(), UploadErrorKind::Storage);
    assert_eq!(fs::read(exclude).unwrap(), original);
}

#[cfg(unix)]
#[test]
fn replacing_the_staged_entry_with_a_symlink_cannot_publish_the_target() {
    use std::os::unix::fs::symlink;

    let mut fixture = UploadFixture::new("part-replacement");
    let jpeg = fixture.jpeg(64, 48);
    let request = fixture.begin(jpeg.len(), digest(&jpeg));
    let began = fixture.uploads.begin(Some(&fixture.cwd), request).unwrap();
    fixture.uploads.chunk(&began.upload_id, 0, &jpeg).unwrap();
    let part = fixture.part_files().pop().unwrap();
    let outside = fixture.root.join("outside.jpg");
    fs::write(&outside, &jpeg).unwrap();
    fs::remove_file(&part).unwrap();
    symlink(&outside, &part).unwrap();

    let error = fixture.uploads.finish(&began.upload_id).unwrap_err();

    assert_eq!(error.kind(), UploadErrorKind::UnsafePath);
    assert_eq!(fs::read(&outside).unwrap(), jpeg);
    assert!(files_with_extension(&fixture.cwd.join(".aiterm/attachments"), "jpg").is_empty());
}

#[cfg(target_os = "linux")]
#[test]
fn preexisting_final_entry_is_never_replaced_during_publication() {
    use std::os::unix::fs::symlink;

    let mut fixture = UploadFixture::new("final-collision");
    let jpeg = fixture.jpeg(64, 48);
    let request = fixture.begin(jpeg.len(), digest(&jpeg));
    let began = fixture.uploads.begin(Some(&fixture.cwd), request).unwrap();
    fixture.uploads.chunk(&began.upload_id, 0, &jpeg).unwrap();
    let part = fixture.part_files().pop().unwrap();
    let final_path = part.parent().unwrap().join(part.file_stem().unwrap());
    let outside = fixture.root.join("outside-final.jpg");
    fs::write(&outside, b"must survive").unwrap();
    symlink(&outside, &final_path).unwrap();

    let error = fixture.uploads.finish(&began.upload_id).unwrap_err();

    assert_eq!(error.kind(), UploadErrorKind::Storage);
    assert_eq!(fs::read(outside).unwrap(), b"must survive");
    assert!(final_path
        .symlink_metadata()
        .unwrap()
        .file_type()
        .is_symlink());
    assert!(!part.exists());
}

#[cfg(unix)]
#[test]
fn replacing_the_attachment_parent_cannot_redirect_validation_or_publication() {
    use std::os::unix::fs::symlink;

    let mut fixture = UploadFixture::new("parent-replacement");
    let jpeg = fixture.jpeg(64, 48);
    let request = fixture.begin(jpeg.len(), digest(&jpeg));
    let began = fixture.uploads.begin(Some(&fixture.cwd), request).unwrap();
    fixture.uploads.chunk(&began.upload_id, 0, &jpeg).unwrap();
    let attachments = fixture.cwd.join(".aiterm/attachments");
    let part_name = fixture
        .part_files()
        .pop()
        .unwrap()
        .file_name()
        .unwrap()
        .to_owned();
    let original = fixture.cwd.join(".aiterm/original-attachments");
    fs::rename(&attachments, &original).unwrap();
    let outside = fixture.root.join("outside-directory");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join(&part_name), &jpeg).unwrap();
    symlink(&outside, &attachments).unwrap();

    let error = fixture.uploads.finish(&began.upload_id).unwrap_err();

    assert_eq!(error.kind(), UploadErrorKind::UnsafePath);
    assert_eq!(fs::read(outside.join(&part_name)).unwrap(), jpeg);
    assert!(files_with_extension(&original, "part").is_empty());
}

#[cfg(unix)]
#[test]
fn later_staging_path_failure_aborts_and_closes_the_live_submission() {
    use std::os::unix::fs::symlink;

    let mut fixture = UploadFixture::new("later-staging-failure");
    let jpeg = fixture.jpeg(64, 48);
    let submission = uuid::Uuid::new_v4().to_string();
    let first = fixture.begin_for_submission(
        &submission,
        2,
        (jpeg.len() * 2) as u64,
        jpeg.len(),
        digest(&jpeg),
    );
    let first_began = fixture.uploads.begin(Some(&fixture.cwd), first).unwrap();
    let attachments = fixture.cwd.join(".aiterm/attachments");
    let original = fixture.cwd.join(".aiterm/original-attachments");
    fs::rename(&attachments, &original).unwrap();
    let outside = fixture.root.join("outside-directory");
    fs::create_dir(&outside).unwrap();
    symlink(&outside, &attachments).unwrap();
    let second = fixture.begin_for_submission(
        &submission,
        2,
        (jpeg.len() * 2) as u64,
        jpeg.len(),
        digest(&jpeg),
    );

    let error = fixture
        .uploads
        .begin(Some(&fixture.cwd), second)
        .unwrap_err();

    assert_eq!(error.kind(), UploadErrorKind::UnsafePath);
    assert!(files_with_extension(&original, "part").is_empty());
    assert_eq!(
        fixture
            .uploads
            .finish(&first_began.upload_id)
            .unwrap_err()
            .kind(),
        UploadErrorKind::NotFound
    );
    fs::remove_file(&attachments).unwrap();
    fs::rename(&original, &attachments).unwrap();
    let retry = fixture.begin_for_submission(
        &submission,
        2,
        (jpeg.len() * 2) as u64,
        jpeg.len(),
        digest(&jpeg),
    );
    assert_eq!(
        fixture
            .uploads
            .begin(Some(&fixture.cwd), retry)
            .unwrap_err()
            .kind(),
        UploadErrorKind::ClosedSubmission
    );
}

#[cfg(unix)]
#[test]
fn later_git_exclude_failure_aborts_the_live_submission_without_following_symlink() {
    use std::os::unix::fs::symlink;

    let mut fixture = UploadFixture::new("later-git-failure");
    let repository = git2::Repository::init(&fixture.cwd).unwrap();
    let jpeg = fixture.jpeg(64, 48);
    let submission = uuid::Uuid::new_v4().to_string();
    let first = fixture.begin_for_submission(
        &submission,
        2,
        (jpeg.len() * 2) as u64,
        jpeg.len(),
        digest(&jpeg),
    );
    let first_began = fixture.uploads.begin(Some(&fixture.cwd), first).unwrap();
    assert_eq!(fixture.part_files().len(), 1);
    let exclude = repository.commondir().join("info/exclude");
    fs::remove_file(&exclude).unwrap();
    let outside = fixture.root.join("outside-exclude");
    fs::write(&outside, b"must survive").unwrap();
    symlink(&outside, &exclude).unwrap();
    let second = fixture.begin_for_submission(
        &submission,
        2,
        (jpeg.len() * 2) as u64,
        jpeg.len(),
        digest(&jpeg),
    );

    let error = fixture
        .uploads
        .begin(Some(&fixture.cwd), second)
        .unwrap_err();

    assert_eq!(error.kind(), UploadErrorKind::UnsafePath);
    assert_eq!(fs::read(outside).unwrap(), b"must survive");
    assert!(fixture.part_files().is_empty());
    assert_eq!(
        fixture
            .uploads
            .finish(&first_began.upload_id)
            .unwrap_err()
            .kind(),
        UploadErrorKind::NotFound
    );
}

#[test]
fn absent_tab_cwd_uses_the_owner_only_fallback_cache() {
    let mut fixture = UploadFixture::new("fallback");
    let jpeg = fixture.jpeg(64, 48);
    let request = fixture.begin(jpeg.len(), digest(&jpeg));
    let began = fixture.uploads.begin(None, request).unwrap();
    fixture.uploads.chunk(&began.upload_id, 0, &jpeg).unwrap();

    let published = fixture.uploads.finish(&began.upload_id).unwrap();

    assert!(published.path.starts_with(&fixture.cache));
    assert_eq!(fs::read(published.path).unwrap(), jpeg);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&fixture.cache).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }
}

#[cfg(unix)]
#[test]
fn staged_and_published_files_are_owner_only() {
    use std::os::unix::fs::PermissionsExt;

    let mut fixture = UploadFixture::new("permissions");
    let jpeg = fixture.jpeg(64, 48);
    let request = fixture.begin(jpeg.len(), digest(&jpeg));
    let began = fixture.uploads.begin(Some(&fixture.cwd), request).unwrap();
    let part = fixture.part_files().pop().unwrap();
    assert_eq!(
        fs::metadata(&part).unwrap().permissions().mode() & 0o777,
        0o600
    );
    fixture.uploads.chunk(&began.upload_id, 0, &jpeg).unwrap();

    let published = fixture.uploads.finish(&began.upload_id).unwrap();

    assert_eq!(
        fs::metadata(published.path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        fs::metadata(fixture.cwd.join(".aiterm"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(fixture.cwd.join(".aiterm/attachments"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
}
