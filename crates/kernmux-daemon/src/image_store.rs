//! Immutable content-addressed storage for managed boot artifacts.

use std::{
    fmt, fs,
    fs::File,
    io::{self, Read, Write},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;

use kernmux_api::v1::Generation;

const RECORD_SCHEMA_VERSION: u32 = 1;
const COPY_BUFFER_BYTES: usize = 64 * 1024;
const MAX_RECORD_BYTES: u64 = 4096;

/// Semantic use of one immutable artifact.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    Kernel,
    Initrd,
}

impl ArtifactKind {
    const fn directory(self) -> &'static str {
        match self {
            Self::Kernel => "kernel",
            Self::Initrd => "initrd",
        }
    }
}

/// Canonical SHA-256 identity of immutable artifact bytes.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ArtifactId(String);

impl ArtifactId {
    /// Parses `sha256:` followed by exactly 64 lowercase hexadecimal digits.
    ///
    /// # Errors
    ///
    /// Returns [`ImageStoreError::InvalidArtifactId`] for noncanonical input.
    pub fn parse(value: impl Into<String>) -> Result<Self, ImageStoreError> {
        let value = value.into();
        let digest = value
            .strip_prefix("sha256:")
            .filter(|digest| {
                digest.len() == 64
                    && digest
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            })
            .ok_or_else(|| ImageStoreError::InvalidArtifactId(value.clone()))?;
        debug_assert_eq!(digest.len(), 64);
        Ok(Self(value))
    }

    fn from_digest(digest: &[u8]) -> Self {
        let mut value = String::with_capacity(71);
        value.push_str("sha256:");
        for byte in digest {
            use std::fmt::Write as _;
            write!(value, "{byte:02x}").expect("writing to a String cannot fail");
        }
        Self(value)
    }

    fn digest(&self) -> &str {
        self.0
            .strip_prefix("sha256:")
            .expect("ArtifactId construction validates the prefix")
    }
}

impl fmt::Display for ArtifactId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Serialize for ArtifactId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for ArtifactId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(de::Error::custom)
    }
}

/// Deterministic metadata for one artifact use.
#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct ArtifactRecord {
    pub schema_version: u32,
    pub kind: ArtifactKind,
    pub id: ArtifactId,
    pub bytes: u64,
}

/// Local immutable artifact store.
#[derive(Clone, Debug)]
pub struct ImageStore {
    root: PathBuf,
    max_artifact_bytes: u64,
}

/// Verified point-in-time view of the image catalog.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImageCatalogSnapshot {
    pub generation: Generation,
    pub artifacts: Vec<ArtifactRecord>,
}

/// Generation-owning reconciliation layer around an [`ImageStore`].
#[derive(Clone, Debug)]
pub struct ImageCatalog {
    store: ImageStore,
    generation: Generation,
    observed: Option<Vec<ArtifactRecord>>,
}

impl ImageCatalog {
    /// Creates an unrefreshed catalog over an immutable store.
    #[must_use]
    pub const fn new(store: ImageStore) -> Self {
        Self {
            store,
            generation: Generation(0),
            observed: None,
        }
    }

    /// Reconciles verified disk state and advances generation only on change.
    ///
    /// # Errors
    ///
    /// Fails closed on catalog corruption, I/O errors, or generation overflow.
    pub fn refresh(&mut self) -> Result<ImageCatalogSnapshot, ImageCatalogError> {
        let artifacts = self.store.list()?;
        if self.observed.as_ref() != Some(&artifacts) {
            self.generation = Generation(
                self.generation
                    .0
                    .checked_add(1)
                    .ok_or(ImageCatalogError::GenerationExhausted)?,
            );
            self.observed = Some(artifacts.clone());
        }
        Ok(ImageCatalogSnapshot {
            generation: self.generation,
            artifacts,
        })
    }

    /// Imports an artifact if the caller observed the current catalog generation.
    ///
    /// Idempotent imports leave the generation unchanged.
    ///
    /// # Errors
    ///
    /// Rejects stale preconditions and propagates store or reconciliation failures.
    pub fn import_path(
        &mut self,
        expected_generation: Generation,
        kind: ArtifactKind,
        source: impl AsRef<Path>,
        expected_id: Option<&ArtifactId>,
    ) -> Result<(ArtifactRecord, ImageCatalogSnapshot), ImageCatalogError> {
        let before = self.refresh()?;
        if expected_generation != before.generation {
            return Err(ImageCatalogError::StaleGeneration {
                expected: expected_generation,
                actual: before.generation,
            });
        }
        let artifact = self.store.import_path(kind, source, expected_id)?;
        let after = self.refresh()?;
        Ok((artifact, after))
    }

    /// Resolves one verified artifact to its immutable blob path.
    ///
    /// # Errors
    ///
    /// Fails closed if catalog state or blob contents cannot be verified.
    pub fn resolve(
        &mut self,
        kind: ArtifactKind,
        id: &ArtifactId,
    ) -> Result<PathBuf, ImageCatalogError> {
        self.refresh()?;
        self.store.resolve(kind, id).map_err(Into::into)
    }
}

/// Failure to reconcile or mutate the image catalog.
#[derive(Debug)]
pub enum ImageCatalogError {
    StaleGeneration {
        expected: Generation,
        actual: Generation,
    },
    GenerationExhausted,
    Store(ImageStoreError),
}

impl From<ImageStoreError> for ImageCatalogError {
    fn from(error: ImageStoreError) -> Self {
        Self::Store(error)
    }
}

impl fmt::Display for ImageCatalogError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::StaleGeneration { expected, actual } => write!(
                formatter,
                "image catalog generation changed from {} to {}",
                expected.0, actual.0
            ),
            Self::GenerationExhausted => {
                formatter.write_str("image catalog generation is exhausted")
            }
            Self::Store(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for ImageCatalogError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(error) => Some(error),
            _ => None,
        }
    }
}

impl ImageStore {
    /// Opens or creates a store rooted at `root`.
    ///
    /// # Errors
    ///
    /// Rejects a zero size limit or an unusable store directory.
    pub fn new(root: impl Into<PathBuf>, max_artifact_bytes: u64) -> Result<Self, ImageStoreError> {
        if max_artifact_bytes == 0 {
            return Err(ImageStoreError::InvalidSizeLimit);
        }
        let store = Self {
            root: root.into(),
            max_artifact_bytes,
        };
        store.prepare_layout()?;
        Ok(store)
    }

    /// Imports a regular file and atomically registers its semantic kind.
    ///
    /// `expected_id`, when present, is verified before any artifact is
    /// published. Re-importing identical bytes and kind is idempotent.
    ///
    /// # Errors
    ///
    /// Rejects non-regular, empty, oversized, digest-mismatched, or corrupt
    /// artifacts and propagates filesystem failures.
    pub fn import_path(
        &self,
        kind: ArtifactKind,
        source: impl AsRef<Path>,
        expected_id: Option<&ArtifactId>,
    ) -> Result<ArtifactRecord, ImageStoreError> {
        let source = source.as_ref();
        let mut input = open_no_follow(source)
            .map_err(|source| ImageStoreError::io("open source artifact", source))?;
        if !input
            .metadata()
            .map_err(|source| ImageStoreError::io("inspect source artifact", source))?
            .is_file()
        {
            return Err(ImageStoreError::SourceNotRegular);
        }

        let mut temporary = NamedTempFile::new_in(self.temporary_directory())
            .map_err(|source| ImageStoreError::io("create temporary blob", source))?;
        let (id, bytes) = self.copy_and_hash(&mut input, temporary.as_file_mut())?;
        if let Some(expected) = expected_id
            && expected != &id
        {
            return Err(ImageStoreError::DigestMismatch {
                expected: expected.clone(),
                actual: id,
            });
        }
        sync_read_only(temporary.as_file_mut(), "sync temporary blob")?;

        let record = ArtifactRecord {
            schema_version: RECORD_SCHEMA_VERSION,
            kind,
            id,
            bytes,
        };
        self.publish_blob(&temporary, &record)?;
        self.publish_record(&record)?;
        Ok(record)
    }

    /// Gets and fully verifies one registered artifact.
    ///
    /// # Errors
    ///
    /// Fails closed when metadata or blob bytes are corrupt.
    pub fn get(
        &self,
        kind: ArtifactKind,
        id: &ArtifactId,
    ) -> Result<Option<ArtifactRecord>, ImageStoreError> {
        let path = self.record_path(kind, id);
        match fs::symlink_metadata(&path) {
            Ok(metadata) => {
                require_read_only_file(&metadata, "artifact metadata")?;
                let record = read_record(&path, kind, id)?;
                self.verify_blob(&record)?;
                Ok(Some(record))
            }
            Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(source) => Err(ImageStoreError::io("inspect artifact metadata", source)),
        }
    }

    /// Lists every registered artifact in deterministic kind/ID order.
    ///
    /// # Errors
    ///
    /// Fails closed if any catalog entry or blob is malformed or corrupt.
    pub fn list(&self) -> Result<Vec<ArtifactRecord>, ImageStoreError> {
        let mut records = Vec::new();
        for kind in [ArtifactKind::Kernel, ArtifactKind::Initrd] {
            let directory = self.record_directory(kind);
            for entry in fs::read_dir(&directory)
                .map_err(|source| ImageStoreError::io("read artifact catalog", source))?
            {
                let entry =
                    entry.map_err(|source| ImageStoreError::io("read catalog entry", source))?;
                let metadata = fs::symlink_metadata(entry.path())
                    .map_err(|source| ImageStoreError::io("inspect catalog entry", source))?;
                require_read_only_file(&metadata, "artifact metadata")?;
                let name = entry.file_name();
                let name = name
                    .to_str()
                    .and_then(|name| name.strip_suffix(".json"))
                    .ok_or_else(|| corrupt("artifact metadata filename is invalid"))?;
                let id = ArtifactId::parse(format!("sha256:{name}"))
                    .map_err(|_| corrupt("artifact metadata filename is invalid"))?;
                let record = read_record(&entry.path(), kind, &id)?;
                self.verify_blob(&record)?;
                records.push(record);
            }
        }
        records.sort();
        Ok(records)
    }

    /// Resolves a verified artifact to its immutable blob path.
    ///
    /// # Errors
    ///
    /// Rejects missing or corrupt artifacts.
    pub fn resolve(&self, kind: ArtifactKind, id: &ArtifactId) -> Result<PathBuf, ImageStoreError> {
        self.get(kind, id)?
            .ok_or(ImageStoreError::NotFound)
            .map(|record| self.blob_path(&record.id))
    }

    fn prepare_layout(&self) -> Result<(), ImageStoreError> {
        for directory in [
            self.root.clone(),
            self.blob_directory(),
            self.record_directory(ArtifactKind::Kernel),
            self.record_directory(ArtifactKind::Initrd),
            self.temporary_directory(),
        ] {
            fs::create_dir_all(&directory)
                .map_err(|source| ImageStoreError::io("create image store directory", source))?;
            if !fs::symlink_metadata(&directory)
                .map_err(|source| ImageStoreError::io("inspect image store directory", source))?
                .file_type()
                .is_dir()
            {
                return Err(corrupt("image store path is not a directory"));
            }
        }
        Ok(())
    }

    fn copy_and_hash(
        &self,
        input: &mut File,
        output: &mut File,
    ) -> Result<(ArtifactId, u64), ImageStoreError> {
        let mut hasher = Sha256::new();
        let mut bytes = 0_u64;
        let mut buffer = vec![0_u8; COPY_BUFFER_BYTES].into_boxed_slice();
        loop {
            let read = input
                .read(&mut buffer)
                .map_err(|source| ImageStoreError::io("read source artifact", source))?;
            if read == 0 {
                break;
            }
            bytes = bytes
                .checked_add(read as u64)
                .ok_or(ImageStoreError::TooLarge {
                    limit: self.max_artifact_bytes,
                })?;
            if bytes > self.max_artifact_bytes {
                return Err(ImageStoreError::TooLarge {
                    limit: self.max_artifact_bytes,
                });
            }
            hasher.update(&buffer[..read]);
            output
                .write_all(&buffer[..read])
                .map_err(|source| ImageStoreError::io("write temporary blob", source))?;
        }
        if bytes == 0 {
            return Err(ImageStoreError::EmptyArtifact);
        }
        Ok((ArtifactId::from_digest(&hasher.finalize()), bytes))
    }

    fn publish_blob(
        &self,
        temporary: &NamedTempFile,
        record: &ArtifactRecord,
    ) -> Result<(), ImageStoreError> {
        let destination = self.blob_path(&record.id);
        match fs::hard_link(temporary.path(), &destination) {
            Ok(()) => sync_directory(&self.blob_directory(), "sync blob directory"),
            Err(source) if source.kind() == io::ErrorKind::AlreadyExists => {
                self.verify_blob(record)
            }
            Err(source) => Err(ImageStoreError::io("publish immutable blob", source)),
        }
    }

    fn publish_record(&self, record: &ArtifactRecord) -> Result<(), ImageStoreError> {
        let bytes = serde_json::to_vec(record)
            .map_err(|source| ImageStoreError::MetadataEncoding(source.to_string()))?;
        let mut temporary = NamedTempFile::new_in(self.temporary_directory())
            .map_err(|source| ImageStoreError::io("create temporary metadata", source))?;
        temporary
            .write_all(&bytes)
            .map_err(|source| ImageStoreError::io("write temporary metadata", source))?;
        sync_read_only(temporary.as_file_mut(), "sync temporary metadata")?;
        let destination = self.record_path(record.kind, &record.id);
        match fs::hard_link(temporary.path(), &destination) {
            Ok(()) => sync_directory(&self.record_directory(record.kind), "sync record directory"),
            Err(source) if source.kind() == io::ErrorKind::AlreadyExists => {
                let existing = read_record(&destination, record.kind, &record.id)?;
                if existing == *record {
                    Ok(())
                } else {
                    Err(corrupt("artifact metadata disagrees with imported bytes"))
                }
            }
            Err(source) => Err(ImageStoreError::io("publish artifact metadata", source)),
        }
    }

    fn verify_blob(&self, record: &ArtifactRecord) -> Result<(), ImageStoreError> {
        let path = self.blob_path(&record.id);
        let mut file = open_no_follow(&path)
            .map_err(|source| ImageStoreError::io("open immutable blob", source))?;
        let metadata = file
            .metadata()
            .map_err(|source| ImageStoreError::io("inspect immutable blob", source))?;
        require_read_only_file(&metadata, "immutable blob")?;
        if metadata.len() != record.bytes {
            return Err(corrupt("immutable blob size disagrees with metadata"));
        }
        let mut hasher = Sha256::new();
        let mut buffer = vec![0_u8; COPY_BUFFER_BYTES].into_boxed_slice();
        loop {
            let read = file
                .read(&mut buffer)
                .map_err(|source| ImageStoreError::io("verify immutable blob", source))?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
        }
        if ArtifactId::from_digest(&hasher.finalize()) != record.id {
            return Err(corrupt("immutable blob digest does not match its ID"));
        }
        Ok(())
    }

    fn blob_directory(&self) -> PathBuf {
        self.root.join("blobs/sha256")
    }

    fn blob_path(&self, id: &ArtifactId) -> PathBuf {
        self.blob_directory().join(id.digest())
    }

    fn record_directory(&self, kind: ArtifactKind) -> PathBuf {
        self.root
            .join("records")
            .join(kind.directory())
            .join("sha256")
    }

    fn record_path(&self, kind: ArtifactKind, id: &ArtifactId) -> PathBuf {
        self.record_directory(kind)
            .join(format!("{}.json", id.digest()))
    }

    fn temporary_directory(&self) -> PathBuf {
        self.root.join(".tmp")
    }
}

fn read_record(
    path: &Path,
    expected_kind: ArtifactKind,
    expected_id: &ArtifactId,
) -> Result<ArtifactRecord, ImageStoreError> {
    let mut file = open_no_follow(path)
        .map_err(|source| ImageStoreError::io("open artifact metadata", source))?;
    let metadata = file
        .metadata()
        .map_err(|source| ImageStoreError::io("inspect artifact metadata", source))?;
    require_read_only_file(&metadata, "artifact metadata")?;
    if metadata.len() > MAX_RECORD_BYTES {
        return Err(corrupt("artifact metadata is oversized"));
    }
    let capacity = usize::try_from(metadata.len())
        .map_err(|_| corrupt("artifact metadata size exceeds address space"))?;
    let mut bytes = Vec::with_capacity(capacity);
    file.read_to_end(&mut bytes)
        .map_err(|source| ImageStoreError::io("read artifact metadata", source))?;
    let record: ArtifactRecord =
        serde_json::from_slice(&bytes).map_err(|_| corrupt("artifact metadata is malformed"))?;
    if record.schema_version != RECORD_SCHEMA_VERSION
        || record.kind != expected_kind
        || record.id != *expected_id
        || record.bytes == 0
    {
        return Err(corrupt("artifact metadata fields are inconsistent"));
    }
    Ok(record)
}

fn require_read_only_file(metadata: &fs::Metadata, label: &str) -> Result<(), ImageStoreError> {
    if !metadata.file_type().is_file() {
        return Err(corrupt(format!("{label} is not a regular file")));
    }
    if metadata.permissions().mode() & 0o222 != 0 {
        return Err(corrupt(format!("{label} is writable")));
    }
    Ok(())
}

fn open_no_follow(path: &Path) -> io::Result<File> {
    rustix::fs::open(
        path,
        rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::CLOEXEC | rustix::fs::OFlags::NOFOLLOW,
        rustix::fs::Mode::empty(),
    )
    .map(File::from)
    .map_err(io::Error::from)
}

fn sync_read_only(file: &mut File, operation: &'static str) -> Result<(), ImageStoreError> {
    file.flush()
        .map_err(|source| ImageStoreError::io(operation, source))?;
    file.set_permissions(fs::Permissions::from_mode(0o444))
        .map_err(|source| ImageStoreError::io(operation, source))?;
    file.sync_all()
        .map_err(|source| ImageStoreError::io(operation, source))
}

fn sync_directory(path: &Path, operation: &'static str) -> Result<(), ImageStoreError> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|source| ImageStoreError::io(operation, source))
}

fn corrupt(detail: impl Into<String>) -> ImageStoreError {
    ImageStoreError::Corrupt(detail.into())
}

/// Failure to import or verify an immutable artifact.
#[derive(Debug)]
pub enum ImageStoreError {
    InvalidSizeLimit,
    InvalidArtifactId(String),
    SourceNotRegular,
    EmptyArtifact,
    TooLarge {
        limit: u64,
    },
    DigestMismatch {
        expected: ArtifactId,
        actual: ArtifactId,
    },
    NotFound,
    Corrupt(String),
    MetadataEncoding(String),
    Io {
        operation: &'static str,
        source: io::Error,
    },
}

impl ImageStoreError {
    fn io(operation: &'static str, source: io::Error) -> Self {
        Self::Io { operation, source }
    }
}

impl fmt::Display for ImageStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSizeLimit => formatter.write_str("artifact size limit must be positive"),
            Self::InvalidArtifactId(value) => {
                write!(formatter, "artifact ID '{value}' is not canonical SHA-256")
            }
            Self::SourceNotRegular => formatter.write_str("artifact source is not a regular file"),
            Self::EmptyArtifact => formatter.write_str("artifact source is empty"),
            Self::TooLarge { limit } => {
                write!(formatter, "artifact exceeds the {limit}-byte size limit")
            }
            Self::DigestMismatch { expected, actual } => {
                write!(
                    formatter,
                    "artifact digest {actual} does not match expected {expected}"
                )
            }
            Self::NotFound => formatter.write_str("artifact was not found"),
            Self::Corrupt(detail) => write!(formatter, "image store is corrupt: {detail}"),
            Self::MetadataEncoding(detail) => {
                write!(
                    formatter,
                    "artifact metadata could not be encoded: {detail}"
                )
            }
            Self::Io { operation, source } => write!(formatter, "{operation}: {source}"),
        }
    }
}

impl std::error::Error for ImageStoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        os::unix::fs::{MetadataExt, symlink},
    };

    use tempfile::TempDir;

    use super::*;

    struct Fixture {
        root: TempDir,
        store: ImageStore,
    }

    impl Fixture {
        fn new(limit: u64) -> Self {
            let root = TempDir::new().unwrap();
            let store = ImageStore::new(root.path().join("store"), limit).unwrap();
            Self { root, store }
        }

        fn source(&self, name: &str, bytes: &[u8]) -> PathBuf {
            let path = self.root.path().join(name);
            fs::write(&path, bytes).unwrap();
            path
        }

        fn assert_no_temporary_files(&self) {
            assert_eq!(
                fs::read_dir(self.store.temporary_directory())
                    .unwrap()
                    .count(),
                0
            );
        }
    }

    #[test]
    fn imports_to_read_only_content_addressed_layout() {
        let fixture = Fixture::new(1024);
        let source = fixture.source("vmlinux", b"kernel image");

        let record = fixture
            .store
            .import_path(ArtifactKind::Kernel, source, None)
            .unwrap();
        let blob = fixture
            .store
            .resolve(ArtifactKind::Kernel, &record.id)
            .unwrap();
        let metadata = fixture.store.record_path(ArtifactKind::Kernel, &record.id);

        assert_eq!(record.schema_version, 1);
        assert_eq!(record.bytes, 12);
        assert_eq!(fs::read(blob).unwrap(), b"kernel image");
        assert_eq!(
            fs::metadata(metadata).unwrap().permissions().mode() & 0o777,
            0o444
        );
        assert_eq!(
            fs::metadata(fixture.store.blob_path(&record.id))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o444
        );
        fixture.assert_no_temporary_files();
    }

    #[test]
    fn reimport_is_idempotent_and_kinds_share_one_blob() {
        let fixture = Fixture::new(1024);
        let source = fixture.source("image", b"same bytes");

        let kernel = fixture
            .store
            .import_path(ArtifactKind::Kernel, &source, None)
            .unwrap();
        assert_eq!(
            fixture
                .store
                .import_path(ArtifactKind::Kernel, &source, None)
                .unwrap(),
            kernel
        );
        let initrd = fixture
            .store
            .import_path(ArtifactKind::Initrd, &source, Some(&kernel.id))
            .unwrap();

        assert_eq!(kernel.id, initrd.id);
        assert_eq!(fixture.store.list().unwrap().len(), 2);
        assert_eq!(
            fs::read_dir(fixture.store.blob_directory())
                .unwrap()
                .count(),
            1
        );
        let kernel_metadata =
            fs::metadata(fixture.store.record_path(ArtifactKind::Kernel, &kernel.id)).unwrap();
        let initrd_metadata =
            fs::metadata(fixture.store.record_path(ArtifactKind::Initrd, &initrd.id)).unwrap();
        assert_ne!(kernel_metadata.ino(), initrd_metadata.ino());
        fixture.assert_no_temporary_files();
    }

    #[test]
    fn expected_digest_mismatch_publishes_nothing() {
        let fixture = Fixture::new(1024);
        let source = fixture.source("image", b"unexpected");
        let expected = ArtifactId::parse(format!("sha256:{}", "0".repeat(64))).unwrap();

        assert!(matches!(
            fixture
                .store
                .import_path(ArtifactKind::Kernel, source, Some(&expected)),
            Err(ImageStoreError::DigestMismatch { .. })
        ));
        assert!(fixture.store.list().unwrap().is_empty());
        assert_eq!(
            fs::read_dir(fixture.store.blob_directory())
                .unwrap()
                .count(),
            0
        );
        fixture.assert_no_temporary_files();
    }

    #[test]
    fn rejects_size_empty_and_non_regular_sources_without_temp_leaks() {
        let fixture = Fixture::new(4);
        let oversized = fixture.source("oversized", b"12345");
        let empty = fixture.source("empty", b"");
        let directory = fixture.root.path().join("directory");
        fs::create_dir(&directory).unwrap();
        let symlink_source = fixture.root.path().join("symlink");
        symlink(&oversized, &symlink_source).unwrap();

        assert!(matches!(
            fixture
                .store
                .import_path(ArtifactKind::Kernel, oversized, None),
            Err(ImageStoreError::TooLarge { limit: 4 })
        ));
        assert!(matches!(
            fixture.store.import_path(ArtifactKind::Kernel, empty, None),
            Err(ImageStoreError::EmptyArtifact)
        ));
        assert!(matches!(
            fixture
                .store
                .import_path(ArtifactKind::Kernel, directory, None),
            Err(ImageStoreError::SourceNotRegular)
        ));
        assert!(matches!(
            fixture
                .store
                .import_path(ArtifactKind::Kernel, symlink_source, None),
            Err(ImageStoreError::Io { .. })
        ));
        fixture.assert_no_temporary_files();
    }

    #[test]
    fn rejects_noncanonical_artifact_ids() {
        for value in [
            "",
            "sha256:abc",
            &format!("sha256:{}", "A".repeat(64)),
            &format!("sha512:{}", "0".repeat(64)),
            &format!("sha256:{}", "g".repeat(64)),
        ] {
            assert!(matches!(
                ArtifactId::parse(value),
                Err(ImageStoreError::InvalidArtifactId(_))
            ));
        }
    }

    #[test]
    fn fails_closed_on_corrupt_existing_blob() {
        let fixture = Fixture::new(1024);
        let source = fixture.source("image", b"original");
        let record = fixture
            .store
            .import_path(ArtifactKind::Kernel, &source, None)
            .unwrap();
        let blob = fixture.store.blob_path(&record.id);
        fs::set_permissions(&blob, fs::Permissions::from_mode(0o644)).unwrap();
        fs::write(&blob, b"changed!").unwrap();
        fs::set_permissions(&blob, fs::Permissions::from_mode(0o444)).unwrap();

        assert!(matches!(
            fixture.store.get(ArtifactKind::Kernel, &record.id),
            Err(ImageStoreError::Corrupt(_))
        ));
        assert!(matches!(
            fixture
                .store
                .import_path(ArtifactKind::Kernel, source, None),
            Err(ImageStoreError::Corrupt(_))
        ));
        fixture.assert_no_temporary_files();
    }

    #[test]
    fn fails_closed_on_corrupt_existing_metadata() {
        let fixture = Fixture::new(1024);
        let source = fixture.source("image", b"original");
        let record = fixture
            .store
            .import_path(ArtifactKind::Kernel, &source, None)
            .unwrap();
        let metadata = fixture.store.record_path(ArtifactKind::Kernel, &record.id);
        fs::set_permissions(&metadata, fs::Permissions::from_mode(0o644)).unwrap();
        fs::write(&metadata, b"not json").unwrap();
        fs::set_permissions(&metadata, fs::Permissions::from_mode(0o444)).unwrap();

        assert!(matches!(
            fixture.store.get(ArtifactKind::Kernel, &record.id),
            Err(ImageStoreError::Corrupt(_))
        ));
        assert!(matches!(
            fixture
                .store
                .import_path(ArtifactKind::Kernel, source, None),
            Err(ImageStoreError::Corrupt(_))
        ));
        fixture.assert_no_temporary_files();
    }

    #[test]
    fn lists_records_in_deterministic_order() {
        let fixture = Fixture::new(1024);
        for (kind, name, bytes) in [
            (ArtifactKind::Initrd, "z", b"z".as_slice()),
            (ArtifactKind::Kernel, "b", b"b".as_slice()),
            (ArtifactKind::Kernel, "a", b"a".as_slice()),
        ] {
            let source = fixture.source(name, bytes);
            fixture.store.import_path(kind, source, None).unwrap();
        }

        let records = fixture.store.list().unwrap();
        assert!(records.windows(2).all(|pair| pair[0] < pair[1]));
        assert_eq!(records[0].kind, ArtifactKind::Kernel);
        assert_eq!(records[1].kind, ArtifactKind::Kernel);
        assert_eq!(records[2].kind, ArtifactKind::Initrd);
        fixture.assert_no_temporary_files();
    }

    #[test]
    fn rejects_writable_catalog_entries_and_missing_resolve() {
        let fixture = Fixture::new(1024);
        let source = fixture.source("image", b"original");
        let record = fixture
            .store
            .import_path(ArtifactKind::Kernel, source, None)
            .unwrap();
        let metadata = fixture.store.record_path(ArtifactKind::Kernel, &record.id);
        fs::set_permissions(&metadata, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(matches!(
            fixture.store.list(),
            Err(ImageStoreError::Corrupt(_))
        ));

        let missing = ArtifactId::parse(format!("sha256:{}", "f".repeat(64))).unwrap();
        assert!(matches!(
            fixture.store.resolve(ArtifactKind::Kernel, &missing),
            Err(ImageStoreError::NotFound)
        ));
    }

    #[test]
    fn rejects_a_symlinked_store_root() {
        let root = TempDir::new().unwrap();
        let actual = root.path().join("actual");
        fs::create_dir(&actual).unwrap();
        let linked = root.path().join("linked");
        symlink(&actual, &linked).unwrap();

        assert!(matches!(
            ImageStore::new(linked, 1024),
            Err(ImageStoreError::Corrupt(_))
        ));
    }

    #[test]
    fn catalog_generation_changes_only_with_verified_state() {
        let fixture = Fixture::new(1024);
        let external = fixture.store.clone();
        let mut catalog = ImageCatalog::new(fixture.store.clone());

        assert_eq!(catalog.refresh().unwrap().generation, Generation(1));
        assert_eq!(catalog.refresh().unwrap().generation, Generation(1));

        let source = fixture.source("kernel", b"kernel");
        external
            .import_path(ArtifactKind::Kernel, source, None)
            .unwrap();
        let changed = catalog.refresh().unwrap();
        assert_eq!(changed.generation, Generation(2));
        assert_eq!(changed.artifacts.len(), 1);
        assert_eq!(catalog.refresh().unwrap().generation, Generation(2));
    }

    #[test]
    fn catalog_import_enforces_generation_and_is_idempotent() {
        let fixture = Fixture::new(1024);
        let source = fixture.source("kernel", b"kernel");
        let mut catalog = ImageCatalog::new(fixture.store.clone());
        assert_eq!(catalog.refresh().unwrap().generation, Generation(1));

        let (artifact, imported) = catalog
            .import_path(Generation(1), ArtifactKind::Kernel, &source, None)
            .unwrap();
        assert_eq!(imported.generation, Generation(2));
        let (_, idempotent) = catalog
            .import_path(
                Generation(2),
                ArtifactKind::Kernel,
                &source,
                Some(&artifact.id),
            )
            .unwrap();
        assert_eq!(idempotent.generation, Generation(2));

        let other = fixture.source("initrd", b"initrd");
        assert!(matches!(
            catalog.import_path(Generation(1), ArtifactKind::Initrd, other, None),
            Err(ImageCatalogError::StaleGeneration {
                expected: Generation(1),
                actual: Generation(2)
            })
        ));
        assert_eq!(catalog.refresh().unwrap().artifacts.len(), 1);
    }

    #[test]
    fn catalog_resolves_only_verified_kind_and_bytes() {
        let fixture = Fixture::new(1024);
        let source = fixture.source("kernel", b"managed kernel");
        let mut catalog = ImageCatalog::new(fixture.store.clone());
        let (record, _) = catalog
            .import_path(Generation(1), ArtifactKind::Kernel, source, None)
            .unwrap();

        let path = catalog.resolve(ArtifactKind::Kernel, &record.id).unwrap();
        assert_eq!(fs::read(path).unwrap(), b"managed kernel");
        assert!(matches!(
            catalog.resolve(ArtifactKind::Initrd, &record.id),
            Err(ImageCatalogError::Store(ImageStoreError::NotFound))
        ));
    }

    #[test]
    fn catalog_fails_closed_without_advancing_on_corruption() {
        let fixture = Fixture::new(1024);
        let source = fixture.source("kernel", b"kernel");
        let record = fixture
            .store
            .import_path(ArtifactKind::Kernel, source, None)
            .unwrap();
        let mut catalog = ImageCatalog::new(fixture.store.clone());
        assert_eq!(catalog.refresh().unwrap().generation, Generation(1));
        let blob = fixture.store.blob_path(&record.id);
        fs::set_permissions(&blob, fs::Permissions::from_mode(0o644)).unwrap();
        fs::write(&blob, b"broken").unwrap();
        fs::set_permissions(&blob, fs::Permissions::from_mode(0o444)).unwrap();

        assert!(matches!(
            catalog.refresh(),
            Err(ImageCatalogError::Store(ImageStoreError::Corrupt(_)))
        ));
        assert_eq!(catalog.generation, Generation(1));
    }
}
