#![doc = include_str!("../README.md")]

use std::{
    env,
    fs::File,
    io::Write,
    path::{Path, PathBuf},
};

use path_clean::PathClean;
use rand::{RngExt, distr::Alphanumeric, rng};

/// Represents a temporary directory.\
/// By default this temporary directory is deleted when this struct is dropped.
#[derive(Debug)]
pub struct TempDirectory {
    path: PathBuf,
    delete_on_drop: bool,
}

impl TempDirectory {
    #[must_use]
    #[allow(clippy::missing_const_for_fn)]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Error happening when creating the directory tree.
#[derive(Debug, thiserror::Error)]
pub enum BuildError {
    #[error("Failed to create the root directory '{0}': {1}")]
    FailedToCreateRootDirectory(PathBuf, std::io::Error),
    #[error("Failed to create directory '{0}': {1}")]
    FailedToCreateDirectory(PathBuf, std::io::Error),
    #[error("Failed to delete directory '{0}': {1}")]
    FailedToDeleteDirectory(PathBuf, std::io::Error),
    #[error("Failed to create file '{0}': {1}")]
    FailedToCreateFile(PathBuf, std::io::Error),
    #[error("Failed to read source file '{0}': {1}")]
    FailedToCopyFile(PathBuf, std::io::Error),
    #[error("Failed to write file '{0}': {1}")]
    FailedToWriteFile(PathBuf, std::io::Error),
    #[error("The entry '{0}' is outside the temporary directory")]
    EntryOutsideDirectory(PathBuf),
    #[error("The entry {0} has an empty name")]
    EmptyEntryName(usize),
    #[error("The entry '{0}' is already existing")]
    DuplicateEntry(PathBuf),
    #[error("Failed to set permissions on '{0}': {1}")]
    FailedToSetPermissions(PathBuf, std::io::Error),
    #[error("Failed to create a symlink to '{0}': {1}")]
    FailedToCreateSymLink(PathBuf, std::io::Error),
}

/// A temporary directory builder that contains a list of entries to be created.
///
/// # Examples
///
/// ```rust
// <snip id="example-builder">
/// use temp_dir_builder::TempDirectoryBuilder;
/// let temp_dir = TempDirectoryBuilder::default()
///     .add_text_file("test/foo.txt", "bar")
///     .add_binary_file("test/foo2.txt", &[98u8, 97u8, 114u8])
///     .add_empty_file("test/folder-a/folder-b/bar.txt")
///     .add_file("test/file.rs", file!())
///     .add_directory("test/dir")
///     .build()
///     .expect("create temp dir");
/// println!("created successfully in {}", temp_dir.path().display());
// </snip>
/// ```
#[derive(Debug)]
pub struct TempDirectoryBuilder {
    /// Root folder where the tree will be created.
    root: Root,
    /// List of file metadata entries in the tree.
    entries: Vec<Entry>,
    /// Flag indicating whether the temporary directory created must be deleted when the instance is dropped.
    delete_on_drop: bool,
}

impl Default for TempDirectoryBuilder {
    /// Creates a default `TempDirectoryBuilder` instance with an empty file list,
    fn default() -> Self {
        Self {
            entries: vec![],
            root: Root::Random,
            delete_on_drop: true,
        }
    }
}

/// Root folder where the tree will be created.
#[derive(Debug)]
enum Root {
    /// A random temporary directory will be generated and atomically created during `build()`.
    Random,
    /// A fixed, caller-provided directory.
    Fixed(PathBuf),
}

impl Drop for TempDirectory {
    fn drop(&mut self) {
        if self.delete_on_drop {
            make_deletable(&self.path);
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

impl TempDirectoryBuilder {
    /// Sets the root folder where the tree will be created.\
    /// By default this is the temporary directory path returned by `std::env::temp_dir()`.
    #[must_use]
    pub fn root_folder(mut self, dir: impl AsRef<Path>) -> Self {
        self.root = Root::Fixed(dir.as_ref().to_path_buf());
        self
    }

    /// Specifies whether to automatically delete the temporary folder when the `TempDirectory` instance is dropped.\
    /// By default this is value is set to `true`.
    #[must_use]
    pub const fn delete_on_drop(mut self, delete_on_drop: bool) -> Self {
        self.delete_on_drop = delete_on_drop;
        self
    }

    #[must_use]
    fn add(mut self, path: impl AsRef<Path>, kind: Kind) -> EntryBuilder {
        self.entries.push(Entry {
            path: path.as_ref().to_path_buf(),
            kind,
            readonly: None,
            #[cfg(unix)]
            mode: None,
        });
        let entry_index = self.entries.len() - 1;
        EntryBuilder {
            builder: self,
            entry_index,
        }
    }

    /// Adds an empty file.
    /// * `path` - Path of the file to create. This path must be relative to the created directory. If the path is outside
    ///   the created directory (e.g: "../foo") the error `BuildError::EntryOutsideDirectory` will be returned.
    #[must_use]
    pub fn add_empty_file<P: AsRef<Path>>(self, path: P) -> EntryBuilder {
        self.add(path, Kind::EmptyFile)
    }

    /// Adds a directory.
    /// * `path` - Path of the directory to create. This path must be relative to the created directory.
    ///   If the path is outside the created directory (e.g: "../foo") the error `BuildError::EntryOutsideDirectory` will be returned.
    #[must_use]
    pub fn add_directory(self, path: impl AsRef<Path>) -> EntryBuilder {
        self.add(path, Kind::Directory)
    }

    /// Adds a text file specifying the content.
    /// * `path` - Path of the text file to create. This path must be relative to the created directory.
    ///   If the path is outside the created directory (e.g: "../foo") the error `BuildError::EntryOutsideDirectory` will be returned.
    /// * `text` - Text to be written in the new file created.
    #[must_use]
    pub fn add_text_file(self, path: impl AsRef<Path>, text: impl Into<String>) -> EntryBuilder {
        self.add(path, Kind::TextFile(text.into()))
    }

    /// Adds a binary file specifying the content.
    /// * `path` - Path of the binary file to create. This path must be relative to the created directory.
    ///   If the path is outside the created directory (e.g: "../foo") the error `BuildError::EntryOutsideDirectory` will be returned.
    /// * `content` - The bytes to be written in the new file created.
    #[must_use]
    pub fn add_binary_file(self, path: impl AsRef<Path>, content: &[u8]) -> EntryBuilder {
        self.add(path, Kind::BinaryFile(content.to_vec()))
    }

    /// Adds a file specifying a source file to be copied.
    /// * `path` - Path of the file to create. This path must be relative to the created directory.
    ///   If the path is outside the created directory (e.g: "../foo") the error `BuildError::EntryOutsideDirectory` will be returned.
    /// * `file` - Path of the file to be copied. If relative, it is resolved against the current working directory.
    #[must_use]
    pub fn add_file(self, path: impl AsRef<Path>, file: impl AsRef<Path>) -> EntryBuilder {
        self.add(path, Kind::FileToCopy(file.as_ref().to_path_buf()))
    }

    /// Builds the file tree by generating files and directories based on the
    /// list of `Entry`s.
    ///
    /// # Errors
    /// A `BuildError` is returned in case of error.
    pub fn build(&self) -> Result<TempDirectory, BuildError> {
        let root = match &self.root {
            Root::Fixed(root) => {
                create_or_validate_fixed_root(root)?;
                root.clone()
            }
            Root::Random => create_random_temp_directory()?,
        };

        let mut created_paths = Vec::with_capacity(self.entries.len());

        for (entry_index, entry) in self.entries.iter().enumerate() {
            if entry.path.as_os_str().is_empty() {
                return Err(BuildError::EmptyEntryName(entry_index));
            }

            let entry_path = root.join(&entry.path).clean();

            if !entry_path.starts_with(&root) {
                return Err(BuildError::EntryOutsideDirectory(entry.path.clone()));
            }

            if entry_path.exists() {
                return Err(BuildError::DuplicateEntry(entry_path));
            }

            if let Some(parent_dir) = Path::new(&entry_path).parent() {
                std::fs::create_dir_all(parent_dir).map_err(|err| {
                    BuildError::FailedToCreateDirectory(parent_dir.to_path_buf(), err)
                })?;
            }

            create_entry(&entry_path, &entry.kind)?;
            created_paths.push(entry_path);
        }

        for (entry, entry_path) in self.entries.iter().zip(created_paths) {
            apply_permissions(&entry_path, entry)?;
        }

        Ok(TempDirectory {
            path: root,
            delete_on_drop: self.delete_on_drop,
        })
    }
}

fn create_entry(entry_path: &Path, kind: &Kind) -> Result<(), BuildError> {
    match kind {
        Kind::Directory => {
            std::fs::create_dir(entry_path).map_err(|err| {
                BuildError::FailedToCreateDirectory(entry_path.to_path_buf(), err)
            })?;
        }
        Kind::EmptyFile => {
            File::create(entry_path)
                .map_err(|err| BuildError::FailedToCreateFile(entry_path.to_path_buf(), err))?;
        }
        Kind::TextFile(text) => {
            let mut new_file = File::create(entry_path)
                .map_err(|err| BuildError::FailedToCreateFile(entry_path.to_path_buf(), err))?;

            new_file
                .write_all(text.as_bytes())
                .map_err(|err| BuildError::FailedToWriteFile(entry_path.to_path_buf(), err))?;
        }
        Kind::BinaryFile(bytes) => {
            let mut new_file = File::create(entry_path)
                .map_err(|err| BuildError::FailedToCreateFile(entry_path.to_path_buf(), err))?;

            new_file
                .write_all(bytes)
                .map_err(|err| BuildError::FailedToWriteFile(entry_path.to_path_buf(), err))?;
        }
        Kind::FileToCopy(source_path) => {
            std::fs::copy(source_path, entry_path)
                .map_err(|err| BuildError::FailedToCopyFile(source_path.clone(), err))?;
        }
        #[cfg(unix)]
        Kind::SymLink {
            kind: SymLinkKind::Unix,
            original,
        } => {
            std::os::unix::fs::symlink(original, &entry_path)
                .map_err(|err| BuildError::FailedToCreateSymLink(original.clone(), err))?;
        }
    }

    Ok(())
}

fn apply_permissions(entry_path: &Path, entry: &Entry) -> Result<(), BuildError> {
    #[cfg(unix)]
    let mode = entry.mode;
    #[cfg(not(unix))]
    let mode: Option<u32> = None;

    if mode.is_none() && entry.readonly.is_none() {
        return Ok(());
    }

    let mut permissions = std::fs::metadata(entry_path)
        .map_err(|err| BuildError::FailedToSetPermissions(entry_path.to_path_buf(), err))?
        .permissions();

    #[cfg(unix)]
    if let Some(mode) = mode {
        use std::os::unix::fs::PermissionsExt;
        permissions.set_mode(mode);
    }

    if let Some(readonly) = entry.readonly {
        set_readonly(&mut permissions, readonly);
    }

    std::fs::set_permissions(entry_path, permissions)
        .map_err(|err| BuildError::FailedToSetPermissions(entry_path.to_path_buf(), err))
}

fn set_readonly(permissions: &mut std::fs::Permissions, readonly: bool) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = permissions.mode();
        permissions.set_mode(if readonly {
            mode & !0o222
        } else {
            mode | 0o200
        });
    }
    #[cfg(not(unix))]
    permissions.set_readonly(readonly);
}

fn make_deletable(path: &Path) {
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return;
    };
    let file_type = metadata.file_type();

    if file_type.is_symlink() {
        return;
    }

    let mut permissions = metadata.permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if !file_type.is_dir() {
            return;
        }
        permissions.set_mode(permissions.mode() | 0o700);
    }
    #[cfg(not(unix))]
    set_readonly(&mut permissions, false);

    let _ = std::fs::set_permissions(path, permissions);

    if file_type.is_dir() {
        if let Ok(entries) = std::fs::read_dir(path) {
            for entry in entries.flatten() {
                make_deletable(&entry.path());
            }
        }
    }
}

/// A builder returned after an entry is added, allowing permissions
/// to be configured before continuing to build the tree.
///
/// # Examples
///
/// ```rust
// <snip id="example-set-readonly">
/// use temp_dir_builder::TempDirectoryBuilder;
/// let temp_dir = TempDirectoryBuilder::default()
///     .add_text_file("test/foo.txt", "bar").set_readonly(true)
///     .add_directory("test/dir")
///     .build()
///     .expect("create temp dir");
// </snip>
/// ```
///
/// On Unix platforms, `set_mode` can be used to set the raw permission bits:
///
/// ```rust
// <snip id="example-set-mode">
/// # #[cfg(unix)]
/// # {
/// use temp_dir_builder::TempDirectoryBuilder;
/// let temp_dir = TempDirectoryBuilder::default()
///     .add_text_file("test/foo.txt", "bar").set_mode(0o744)
///     .add_directory("test/dir")
///     .build()
///     .expect("create temp dir");
/// # }
// </snip>
/// ```
#[derive(Debug)]
pub struct EntryBuilder {
    builder: TempDirectoryBuilder,
    entry_index: usize,
}

impl EntryBuilder {
    /// Sets whether the entry just added is read-only.
    #[must_use]
    pub fn set_readonly(mut self, readonly: bool) -> Self {
        self.last_entry_mut().readonly = Some(readonly);
        self
    }

    /// Sets the Unix permission bits of the entry just added, e.g. `0o744`.
    #[cfg(unix)]
    #[must_use]
    pub fn set_mode(mut self, mode: u32) -> Self {
        self.last_entry_mut().mode = Some(mode);
        self
    }

    fn last_entry_mut(&mut self) -> &mut Entry {
        &mut self.builder.entries[self.entry_index]
    }

    /// Sets the root folder where the tree will be created.
    /// By default this is the temporary directory path returned by `std::env::temp_dir()`.
    #[must_use]
    pub fn root_folder(self, dir: impl AsRef<Path>) -> TempDirectoryBuilder {
        self.builder.root_folder(dir)
    }

    /// Specifies whether to automatically delete the temporary folder when the `TempDirectory` instance is dropped.
    /// By default this is value is set to `true`.
    #[must_use]
    pub fn delete_on_drop(self, delete_on_drop: bool) -> TempDirectoryBuilder {
        self.builder.delete_on_drop(delete_on_drop)
    }

    /// Adds an empty file.
    #[must_use]
    pub fn add_empty_file<P: AsRef<Path>>(self, path: P) -> Self {
        self.builder.add_empty_file(path)
    }

    /// Adds a directory.
    #[must_use]
    pub fn add_directory(self, path: impl AsRef<Path>) -> Self {
        self.builder.add_directory(path)
    }

    /// Adds a text file specifying the content.
    #[must_use]
    pub fn add_text_file(self, path: impl AsRef<Path>, text: impl Into<String>) -> Self {
        self.builder.add_text_file(path, text)
    }

    /// Adds a binary file specifying the content.
    #[must_use]
    pub fn add_binary_file(self, path: impl AsRef<Path>, content: &[u8]) -> Self {
        self.builder.add_binary_file(path, content)
    }

    /// Adds a file specifying a source file to be copied.
    #[must_use]
    pub fn add_file(self, path: impl AsRef<Path>, file: impl AsRef<Path>) -> Self {
        self.builder.add_file(path, file)
    }

    /// Builds the file tree by generating files and directories based on the
    /// list of `Entry`s.
    ///
    /// # Errors
    /// A `BuildError` is returned in case of error.
    pub fn build(&self) -> Result<TempDirectory, BuildError> {
        self.builder.build()
    }
}

fn create_or_validate_fixed_root(root: &Path) -> Result<(), BuildError> {
    match std::fs::create_dir(root) {
        Ok(()) => return Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            if let Some(parent) = root.parent() {
                std::fs::create_dir_all(parent).map_err(|err| {
                    BuildError::FailedToCreateRootDirectory(root.to_path_buf(), err)
                })?;
            }

            match std::fs::create_dir(root) {
                Ok(()) => return Ok(()),
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(err) => {
                    return Err(BuildError::FailedToCreateRootDirectory(
                        root.to_path_buf(),
                        err,
                    ));
                }
            }
        }
        Err(err) => {
            return Err(BuildError::FailedToCreateRootDirectory(
                root.to_path_buf(),
                err,
            ));
        }
    }

    // Residual TOCTOU: root could be swapped for a symlink between this check and later use, see issue #16.
    match std::fs::symlink_metadata(root) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            Err(BuildError::FailedToCreateRootDirectory(
                root.to_path_buf(),
                std::io::Error::new(std::io::ErrorKind::AlreadyExists, "root path is a symlink"),
            ))
        }
        Ok(metadata) if !metadata.is_dir() => Err(BuildError::FailedToCreateRootDirectory(
            root.to_path_buf(),
            std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "root path exists and is not a directory",
            ),
        )),
        Ok(_) => Ok(()),
        Err(err) => Err(BuildError::FailedToCreateRootDirectory(
            root.to_path_buf(),
            err,
        )),
    }
}

const MAX_RANDOM_DIRECTORY_ATTEMPTS: u32 = 100;

fn create_random_temp_directory() -> Result<PathBuf, BuildError> {
    for _ in 0..MAX_RANDOM_DIRECTORY_ATTEMPTS {
        let random_string: String = rng()
            .sample_iter(&Alphanumeric)
            .take(5)
            .map(char::from)
            .collect();

        let path = env::temp_dir().join(random_string);

        match std::fs::create_dir(&path) {
            Ok(()) => return Ok(path),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(err) => return Err(BuildError::FailedToCreateRootDirectory(path, err)),
        }
    }

    Err(BuildError::FailedToCreateRootDirectory(
        env::temp_dir(),
        std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "exhausted attempts to generate an unused random directory name",
        ),
    ))
}

#[derive(Debug)]
enum Kind {
    Directory,
    EmptyFile,
    TextFile(String),
    BinaryFile(Vec<u8>),
    FileToCopy(PathBuf),
    SymLink {
        kind: SymLinkKind,
        original: std::path::PathBuf,
    },
}

#[derive(Debug)]
enum SymLinkKind {
    #[cfg(unix)]
    Unix,
}

/// Represents an entry, file or directory, to be created.
#[derive(Debug)]
struct Entry {
    /// Path of the entry relative to the root folder.
    path: PathBuf,
    /// The kind of the entry.
    kind: Kind,
    /// Whether the entry must be made read-only.
    readonly: Option<bool>,
    /// The Unix permission bits to apply to the entry.
    #[cfg(unix)]
    mode: Option<u32>,
}

#[cfg(unix)]
pub mod unix {
    use std::path::Path;

    use crate::{EntryBuilder, Kind, TempDirectoryBuilder};

    pub trait TempDirectoryBuilderExt {
        #[must_use]
        fn add_symlink(self, original: impl AsRef<Path>, link: impl AsRef<Path>) -> EntryBuilder;
    }

    impl TempDirectoryBuilderExt for TempDirectoryBuilder {
        /// Adds a symlink
        fn add_symlink(self, original: impl AsRef<Path>, link: impl AsRef<Path>) -> EntryBuilder {
            self.add(
                link,
                Kind::SymLink {
                    kind: crate::SymLinkKind::Unix,
                    original: original.as_ref().to_path_buf(),
                },
            )
        }
    }

    impl TempDirectoryBuilderExt for EntryBuilder {
        fn add_symlink(self, original: impl AsRef<Path>, link: impl AsRef<Path>) -> EntryBuilder {
            self.builder.add_symlink(original, link)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_temp_dir() {
        let temp_dir = TempDirectoryBuilder::default().build().unwrap();

        assert!(temp_dir.path().exists());
        assert!(temp_dir.path().is_dir());
    }

    #[test]
    fn test_add_text_file() {
        let expected_content = "bar";
        let entry_name = "foo.txt";
        let temp_dir = TempDirectoryBuilder::default()
            .add_text_file(entry_name, expected_content)
            .build()
            .unwrap();
        let entry_path = temp_dir.path().join(entry_name);

        assert!(entry_path.exists());

        let content = std::fs::read_to_string(entry_path).expect("read text in foo.txt");

        assert_eq!(content, expected_content);
    }

    #[test]
    fn test_add_binary_file() {
        let expected_content = [98u8, 97u8, 114u8];
        let entry_name = "foo.txt";
        let temp_dir = TempDirectoryBuilder::default()
            .add_binary_file(entry_name, &expected_content)
            .build()
            .unwrap();
        let entry_path = temp_dir.path().join(entry_name);

        assert!(entry_path.exists());

        let content = std::fs::read(entry_path).expect("read foo.txt");

        assert_eq!(content, expected_content);
    }

    #[test]
    fn test_add_empty_file() {
        let entry_name = "empty_file.txt";
        let temp_dir = TempDirectoryBuilder::default()
            .add_empty_file(entry_name)
            .build()
            .unwrap();
        let entry_path = temp_dir.path().join(entry_name);

        assert!(entry_path.exists());

        let created_entry_metadata = std::fs::metadata(entry_path).expect("get entry metadata");

        assert_eq!(created_entry_metadata.len(), 0);
    }

    #[test]
    fn test_add_directory() {
        let entry_name = "empty_directory";
        let temp_dir = TempDirectoryBuilder::default()
            .add_directory(entry_name)
            .build()
            .unwrap();
        let entry_path = temp_dir.path().join(entry_name);

        assert!(entry_path.exists());
        assert!(entry_path.is_dir());
    }

    #[test]
    fn test_add_file() {
        let entry_name = "test.rs";
        let source_file_path = file!();
        let temp_dir = TempDirectoryBuilder::default()
            .add_file(entry_name, source_file_path)
            .build()
            .unwrap();
        let entry_path = temp_dir.path().join(entry_name);

        assert!(entry_path.exists());
        assert!(entry_path.is_file());

        let entry_content = std::fs::read_to_string(entry_path).unwrap();
        let source_content = std::fs::read_to_string(source_file_path).unwrap();

        assert_eq!(entry_content, source_content);
    }

    #[test]
    #[cfg(unix)]
    fn test_add_unix_symlink() {
        use crate::unix::TempDirectoryBuilderExt;

        let entry_name = "test.rs";
        let source_file_path = Path::new(file!()).canonicalize().unwrap();
        let temp_dir = TempDirectoryBuilder::default()
            .add_symlink(&source_file_path, entry_name)
            .build()
            .unwrap();
        let entry_path = temp_dir.path().join(entry_name);

        assert!(entry_path.exists());
        assert!(entry_path.is_symlink());
        assert!(entry_path.is_file());

        let entry_content = std::fs::read_to_string(entry_path).unwrap();
        let source_content = std::fs::read_to_string(source_file_path).unwrap();

        assert_eq!(entry_content, source_content);
    }

    #[test]
    fn test_temp_dir_is_dropped() {
        let temp_dir = TempDirectoryBuilder::default().build().unwrap();

        let temp_dir_path = temp_dir.path().to_path_buf();

        assert!(temp_dir_path.exists());
        assert!(temp_dir_path.is_dir());

        drop(temp_dir);

        assert!(!temp_dir_path.exists());
    }

    #[test]
    fn test_entry_outside_temp_dir() {
        let path_outside_temp_dir = std::env::temp_dir().join("outside");
        let builder = TempDirectoryBuilder::default().add_empty_file(path_outside_temp_dir);
        let error = builder.build().unwrap_err();

        assert!(matches!(error, BuildError::EntryOutsideDirectory(_)));
    }

    #[test]
    fn test_source_file_does_not_exists() {
        let source_file_path = std::env::temp_dir().join("not existing file");
        let builder = TempDirectoryBuilder::default().add_file("foo", source_file_path);
        let error = builder.build().unwrap_err();

        assert!(matches!(error, BuildError::FailedToCopyFile(..)));
    }

    #[test]
    fn test_duplicated_entries() {
        let builder = TempDirectoryBuilder::default()
            .add_empty_file("foo")
            .add_empty_file("foo");
        let error = builder.build().unwrap_err();

        assert!(matches!(error, BuildError::DuplicateEntry(..)));
    }

    #[test]
    fn test_entry_outside_directory() {
        let builder = TempDirectoryBuilder::default().add_empty_file("../foo");
        let error = builder.build().unwrap_err();

        assert!(matches!(error, BuildError::EntryOutsideDirectory(..)));
    }

    #[test]
    fn test_empty_entry_name() {
        let builder = TempDirectoryBuilder::default().add_empty_file("");
        let error = builder.build().unwrap_err();

        assert!(matches!(error, BuildError::EmptyEntryName(0)));
    }

    #[test]
    fn test_set_readonly_file() {
        let temp_dir = TempDirectoryBuilder::default()
            .add_text_file("readonly.txt", "foo")
            .set_readonly(true)
            .add_text_file("writable.txt", "bar")
            .set_readonly(false)
            .add_text_file("default.txt", "baz")
            .build()
            .unwrap();

        let readonly_path = temp_dir.path().join("readonly.txt");
        let writable_path = temp_dir.path().join("writable.txt");
        let default_path = temp_dir.path().join("default.txt");

        assert!(
            std::fs::metadata(&readonly_path)
                .unwrap()
                .permissions()
                .readonly()
        );
        assert!(
            !std::fs::metadata(&writable_path)
                .unwrap()
                .permissions()
                .readonly()
        );
        assert!(
            !std::fs::metadata(&default_path)
                .unwrap()
                .permissions()
                .readonly()
        );
        assert!(std::fs::write(&readonly_path, "changed").is_err());
        assert!(std::fs::write(&writable_path, "changed").is_ok());
    }

    #[test]
    fn test_set_readonly_directory() {
        let temp_dir = TempDirectoryBuilder::default()
            .add_directory("dir")
            .set_readonly(true)
            .add_text_file("dir/foo.txt", "foo")
            .build()
            .unwrap();

        let dir_path = temp_dir.path().join("dir");

        assert!(
            std::fs::metadata(&dir_path)
                .unwrap()
                .permissions()
                .readonly()
        );
        assert_eq!(
            std::fs::read_to_string(dir_path.join("foo.txt")).unwrap(),
            "foo"
        );
    }

    #[test]
    fn test_readonly_entries_are_deleted_on_drop() {
        let temp_dir = TempDirectoryBuilder::default()
            .add_directory("dir")
            .set_readonly(true)
            .add_text_file("dir/foo.txt", "foo")
            .set_readonly(true)
            .add_directory("dir/nested")
            .set_readonly(true)
            .add_empty_file("dir/nested/bar.txt")
            .set_readonly(true)
            .build()
            .unwrap();
        let root = temp_dir.path().to_path_buf();

        drop(temp_dir);

        assert!(!root.exists());
    }

    #[test]
    #[cfg(unix)]
    fn test_set_mode() {
        use std::os::unix::fs::PermissionsExt;

        let temp_dir = TempDirectoryBuilder::default()
            .add_text_file("script.sh", "#!/bin/sh")
            .set_mode(0o744)
            .add_directory("dir")
            .set_mode(0o500)
            .add_empty_file("dir/foo.txt")
            .build()
            .unwrap();

        let script_mode = std::fs::metadata(temp_dir.path().join("script.sh"))
            .unwrap()
            .permissions()
            .mode();
        let dir_mode = std::fs::metadata(temp_dir.path().join("dir"))
            .unwrap()
            .permissions()
            .mode();

        assert_eq!(script_mode & 0o777, 0o744);
        assert_eq!(dir_mode & 0o777, 0o500);
        assert!(temp_dir.path().join("dir/foo.txt").exists());

        let root = temp_dir.path().to_path_buf();

        drop(temp_dir);

        assert!(!root.exists());
    }

    #[test]
    #[cfg(unix)]
    fn test_set_mode_then_readonly() {
        use std::os::unix::fs::PermissionsExt;

        let temp_dir = TempDirectoryBuilder::default()
            .add_empty_file("foo")
            .set_mode(0o766)
            .set_readonly(true)
            .build()
            .unwrap();

        let mode = std::fs::metadata(temp_dir.path().join("foo"))
            .unwrap()
            .permissions()
            .mode();

        assert_eq!(mode & 0o777, 0o544);
    }
}
