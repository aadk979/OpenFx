//! Moving geometry between the filesystem and the engine.
//!
//! Scenes stay pure: an import stores geometry by content hash, and an `asset` node refers to that
//! hash, so a scene cooks identically however the source file later changes.
//!
//! Every path comes from an agent and is hostile until proven otherwise. File access is off unless
//! the host names a workspace, and all paths must stay inside it.

use pcc_eval::{AssetError, AssetResolver};
use pcc_geo::GeoDetail;
use pcc_io::{
    IoFormatError, from_obj, from_ofxg_bytes_with_limit, from_stl_bytes, to_obj, to_ofxg_bytes,
    to_stl_bytes,
};
use pcc_sop::{SopContext, triangulate};
use std::{
    collections::BTreeMap,
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, atomic::AtomicBool},
};

/// Inside the workspace; agents can neither read nor write it through paths.
const ASSET_DIRECTORY: &str = ".pcc-assets";
const MAX_IMPORT_TRIANGLES: usize = 50_000_000;
/// Windows opens these as devices whatever directory or extension they are given.
const RESERVED_NAMES: &[&str] = &[
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// Why a file operation failed. Messages name only workspace-relative paths.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FileError {
    /// The host did not configure a workspace.
    Disabled,
    PathRejected(&'static str),
    NotFound,
    AlreadyExists,
    UnsupportedFormat,
    TooLarge,
    Parse(IoFormatError),
    Io(io::ErrorKind),
}

impl FileError {
    #[must_use]
    pub fn message(&self) -> String {
        match self {
            Self::Disabled => {
                "file access is disabled: start the server with --workspace DIR".to_owned()
            }
            Self::PathRejected(reason) => format!("path rejected: {reason}"),
            Self::NotFound => "no such file in the workspace".to_owned(),
            Self::AlreadyExists => {
                "the file already exists: pass overwrite to replace it".to_owned()
            }
            Self::UnsupportedFormat => "unsupported format: use .stl, .obj, or .ofxg".to_owned(),
            Self::TooLarge => "the file exceeds the memory budget".to_owned(),
            Self::Parse(error) => format!("could not parse the file: {error:?}"),
            Self::Io(kind) => format!("file operation failed: {kind}"),
        }
    }
}

impl From<io::Error> for FileError {
    fn from(error: io::Error) -> Self {
        match error.kind() {
            io::ErrorKind::NotFound => Self::NotFound,
            io::ErrorKind::AlreadyExists => Self::AlreadyExists,
            kind => Self::Io(kind),
        }
    }
}

/// Mesh file formats, chosen by extension.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MeshFormat {
    Stl,
    Obj,
    Ofxg,
}

impl MeshFormat {
    fn from_path(path: &Path) -> Result<Self, FileError> {
        let extension = path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase);
        match extension.as_deref() {
            Some("stl") => Ok(Self::Stl),
            Some("obj") => Ok(Self::Obj),
            Some("ofxg") => Ok(Self::Ofxg),
            _ => Err(FileError::UnsupportedFormat),
        }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Stl => "stl",
            Self::Obj => "obj",
            Self::Ofxg => "ofxg",
        }
    }
}

/// Checks a relative path lexically, before anything touches the filesystem.
fn validate_relative(text: &str) -> Result<PathBuf, FileError> {
    let reject = |reason| Err(FileError::PathRejected(reason));
    if text.is_empty() {
        return reject("empty");
    }
    if text.contains('\0') || text.contains(':') {
        // A colon covers drive letters and NTFS alternate data streams.
        return reject("contains a forbidden character");
    }
    // Backslashes separate on Windows only, so treat them as separators everywhere to judge the
    // path the same way on every platform.
    let normalized = text.replace('\\', "/");
    // Judged on the raw text: `Path` would quietly normalize some of these away.
    for part in normalized.split('/') {
        if part.is_empty() || part == "." || part == ".." {
            // A leading separator makes an empty first segment, so this covers absolute paths.
            return reject("must be relative, without `.`, `..`, or empty segments");
        }
        if part.eq_ignore_ascii_case(ASSET_DIRECTORY) {
            return reject("the asset store is not accessible");
        }
        // Windows silently strips trailing dots and spaces, so `a.stl.` could alias `a.stl`.
        if part.ends_with('.') || part.ends_with(' ') {
            return reject("must not end a name with a dot or space");
        }
        let stem = part.split('.').next().unwrap_or(part);
        if RESERVED_NAMES.iter().any(|r| stem.eq_ignore_ascii_case(r)) {
            return reject("uses a reserved device name");
        }
    }
    Ok(PathBuf::from(normalized))
}

/// A directory that bounds all file access, and the content-addressed asset store inside it.
#[derive(Debug)]
pub struct Workspace {
    root: PathBuf,
    assets: PathBuf,
}

impl Workspace {
    /// Opens an existing directory as the workspace.
    pub fn open(root: &Path) -> Result<Self, FileError> {
        let root = root.canonicalize()?;
        if !root.is_dir() {
            return Err(FileError::NotFound);
        }
        let assets = root.join(ASSET_DIRECTORY);
        fs::create_dir_all(&assets)?;
        Ok(Self { root, assets })
    }

    /// Resolves a path whose symlinks, once followed, must remain inside the workspace.
    fn contained(&self, resolved: &Path) -> Result<(), FileError> {
        if resolved.starts_with(&self.root) && !resolved.starts_with(&self.assets) {
            Ok(())
        } else {
            Err(FileError::PathRejected("leaves the workspace"))
        }
    }

    fn resolve_existing(&self, text: &str) -> Result<PathBuf, FileError> {
        let relative = validate_relative(text)?;
        let resolved = self.root.join(relative).canonicalize()?;
        self.contained(&resolved)?;
        Ok(resolved)
    }

    /// Resolves a write target. Its parent must exist and be inside; the file itself may not.
    fn resolve_target(&self, text: &str) -> Result<PathBuf, FileError> {
        let relative = validate_relative(text)?;
        let joined = self.root.join(&relative);
        let name = joined
            .file_name()
            .ok_or(FileError::PathRejected("names no file"))?
            .to_owned();
        let parent = joined
            .parent()
            .ok_or(FileError::PathRejected("names no file"))?
            .canonicalize()?;
        self.contained(&parent)?;
        Ok(parent.join(name))
    }

    fn read_limited(&self, text: &str, limit: usize) -> Result<(Vec<u8>, MeshFormat), FileError> {
        let path = self.resolve_existing(text)?;
        let format = MeshFormat::from_path(&path)?;
        let metadata = fs::metadata(&path)?;
        if !metadata.is_file() {
            return Err(FileError::PathRejected("is not a regular file"));
        }
        if metadata.len() > limit as u64 {
            return Err(FileError::TooLarge);
        }
        // The length can change after the check, so the read itself is bounded as well.
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        fs::File::open(&path)?
            .take(limit as u64 + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() > limit {
            return Err(FileError::TooLarge);
        }
        Ok((bytes, format))
    }

    fn asset_path(&self, content_hash: &str) -> PathBuf {
        self.assets.join(format!("{content_hash}.ofxg"))
    }
}

/// Writes through a temporary file in the same directory, so that readers never see a partial
/// file and the rename replaces a symlink at the target rather than following it.
fn write_atomic(target: &Path, bytes: &[u8], overwrite: bool) -> Result<(), FileError> {
    if !overwrite && fs::symlink_metadata(target).is_ok() {
        return Err(FileError::AlreadyExists);
    }
    let name = target
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or(FileError::PathRejected("names no file"))?;
    let temporary = target.with_file_name(format!(".{name}.{}.tmp", std::process::id()));
    let result = (|| -> io::Result<()> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, target)
    })();
    if result.is_err() {
        // The original error is the useful one; a leftover temporary changes nothing about it.
        let _ = fs::remove_file(&temporary);
    }
    Ok(result?)
}

fn parse_mesh(bytes: &[u8], format: MeshFormat, limit: usize) -> Result<GeoDetail, FileError> {
    match format {
        MeshFormat::Stl => from_stl_bytes(bytes, MAX_IMPORT_TRIANGLES),
        MeshFormat::Ofxg => from_ofxg_bytes_with_limit(bytes, limit),
        MeshFormat::Obj => from_obj(&String::from_utf8_lossy(bytes)),
    }
    .map_err(FileError::Parse)
}

/// Serializes `geo` in `format`. STL has only triangles, so it stores a triangulated copy.
fn encode_mesh(
    geo: &GeoDetail,
    format: MeshFormat,
    ctx: &SopContext<'_>,
) -> Result<Vec<u8>, FileError> {
    match format {
        MeshFormat::Ofxg => Ok(to_ofxg_bytes(geo)),
        MeshFormat::Obj => Ok(to_obj(geo).into_bytes()),
        MeshFormat::Stl => {
            let triangles =
                triangulate(geo, ctx).map_err(|e| FileError::Parse(IoFormatError::Geo(e)))?;
            to_stl_bytes(&triangles).map_err(FileError::Parse)
        }
    }
}

/// Imported geometry, cached in memory and persisted by content hash.
#[derive(Debug)]
pub struct AssetStore {
    workspace: Option<Workspace>,
    loaded: Mutex<BTreeMap<String, Arc<GeoDetail>>>,
    memory_limit: usize,
}

impl AssetStore {
    #[must_use]
    pub fn new(workspace: Option<Workspace>, memory_limit: usize) -> Self {
        Self {
            workspace,
            loaded: Mutex::new(BTreeMap::new()),
            memory_limit,
        }
    }

    fn workspace(&self) -> Result<&Workspace, FileError> {
        self.workspace.as_ref().ok_or(FileError::Disabled)
    }

    /// Reads a mesh from the workspace into the store and returns it with its content hash.
    /// Importing the same content again is a no-op.
    pub fn import(&self, path: &str) -> Result<(String, Arc<GeoDetail>), FileError> {
        let workspace = self.workspace()?;
        let (bytes, format) = workspace.read_limited(path, self.memory_limit)?;
        let geo = parse_mesh(&bytes, format, self.memory_limit)?;
        let hash = geo.hash().to_hex().to_string();
        let target = workspace.asset_path(&hash);
        if fs::symlink_metadata(&target).is_err() {
            write_atomic(&target, &to_ofxg_bytes(&geo), false)?;
        }
        let geo = Arc::new(geo);
        if let Ok(mut loaded) = self.loaded.lock() {
            loaded.insert(hash.clone(), Arc::clone(&geo));
        }
        Ok((hash, geo))
    }

    /// Cooks are done by the caller; this encodes the result and writes it to the workspace.
    pub fn export(
        &self,
        geo: &GeoDetail,
        path: &str,
        overwrite: bool,
    ) -> Result<(MeshFormat, usize), FileError> {
        let workspace = self.workspace()?;
        let target = workspace.resolve_target(path)?;
        let format = MeshFormat::from_path(&target)?;
        let cancelled = AtomicBool::new(false);
        let ctx = SopContext::new(self.memory_limit, &cancelled);
        let bytes = encode_mesh(geo, format, &ctx)?;
        if let Ok(metadata) = fs::symlink_metadata(&target)
            && metadata.is_dir()
        {
            return Err(FileError::PathRejected("is a directory"));
        }
        write_atomic(&target, &bytes, overwrite)?;
        Ok((format, bytes.len()))
    }
}

impl AssetResolver for AssetStore {
    fn resolve(&self, content_hash: &str) -> Result<Arc<GeoDetail>, AssetError> {
        if let Ok(loaded) = self.loaded.lock()
            && let Some(geo) = loaded.get(content_hash)
        {
            return Ok(Arc::clone(geo));
        }
        let workspace = self.workspace.as_ref().ok_or(AssetError::NotFound)?;
        // The hash was validated as hexadecimal when the graph was, so it cannot escape the store.
        let path = workspace.asset_path(content_hash);
        let bytes = fs::read(path).map_err(|_| AssetError::NotFound)?;
        let geo = from_ofxg_bytes_with_limit(&bytes, self.memory_limit)
            .map_err(|_| AssetError::Corrupt)?;
        // A file can be edited on disk: what it holds must be what the scene asked for.
        if geo.hash().to_hex().as_str() != content_hash {
            return Err(AssetError::Corrupt);
        }
        let geo = Arc::new(geo);
        if let Ok(mut loaded) = self.loaded.lock() {
            loaded.insert(content_hash.to_owned(), Arc::clone(&geo));
        }
        Ok(geo)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh directory for one test, under the build's temporary area.
    fn scratch(name: &str) -> Result<PathBuf, io::Error> {
        let root = std::env::temp_dir().join(format!("pcc-exchange-{}-{name}", std::process::id()));
        if root.exists() {
            fs::remove_dir_all(&root)?;
        }
        fs::create_dir_all(&root)?;
        Ok(root)
    }

    fn triangle_stl() -> Vec<u8> {
        let mut bytes = vec![0u8; 80];
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&[0u8; 12]);
        for vertex in [[0f32, 0., 0.], [1., 0., 0.], [0., 1., 0.]] {
            for component in vertex {
                bytes.extend_from_slice(&component.to_le_bytes());
            }
        }
        bytes.extend_from_slice(&[0u8; 2]);
        bytes
    }

    #[test]
    fn hostile_paths_are_rejected_before_touching_the_filesystem() {
        let hostile = [
            "",
            "../secret.stl",
            "a/../../secret.stl",
            "..\\secret.stl",
            "a\\..\\..\\secret.stl",
            "/etc/passwd",
            "\\\\server\\share\\a.stl",
            "C:\\Windows\\win.ini",
            "C:/Windows/win.ini",
            "a.stl:stream",
            "a\0.stl",
            "./a.stl",
            "a//b.stl",
            "a/",
            "a/./b.stl",
            "NUL",
            "nul.stl",
            "Com1.obj",
            "sub/LPT9.stl",
            "a.stl.",
            "a.stl ",
            ".pcc-assets/abc.ofxg",
            ".PCC-ASSETS/abc.ofxg",
            "sub/.pcc-assets/abc.ofxg",
        ];
        for path in hostile {
            assert!(
                matches!(validate_relative(path), Err(FileError::PathRejected(_))),
                "{path:?} was accepted"
            );
        }
        for path in [
            "a.stl",
            "sub/dir/a.obj",
            "sub\\a.stl",
            "console.stl",
            ".hidden.stl",
        ] {
            assert!(validate_relative(path).is_ok(), "{path:?} was rejected");
        }
    }

    #[test]
    fn file_access_is_off_without_a_workspace() {
        let store = AssetStore::new(None, 1 << 20);
        assert_eq!(store.import("a.stl").err(), Some(FileError::Disabled));
        let empty = GeoDetail::points(Vec::new());
        assert_eq!(
            store.export(&empty, "a.obj", false).err(),
            Some(FileError::Disabled)
        );
        assert_eq!(
            store.resolve(&"0".repeat(64)).err(),
            Some(AssetError::NotFound)
        );
    }

    #[test]
    fn imports_are_content_addressed_and_survive_the_source_changing() -> Result<(), FileError> {
        let root = scratch("content").map_err(FileError::from)?;
        fs::write(root.join("mesh.stl"), triangle_stl())?;
        let store = AssetStore::new(Some(Workspace::open(&root)?), 1 << 20);
        let (hash, geo) = store.import("mesh.stl")?;
        assert_eq!(hash.len(), 64);
        assert_eq!(geo.prim_count(), 1);
        // Same bytes under another name are the same asset.
        fs::write(root.join("copy.stl"), triangle_stl())?;
        assert_eq!(store.import("copy.stl")?.0, hash);
        // Changing and deleting the source does not change what the hash resolves to, even after a
        // restart, which is what makes scenes reproducible.
        fs::write(root.join("mesh.stl"), b"overwritten")?;
        fs::remove_file(root.join("copy.stl"))?;
        let restarted = AssetStore::new(Some(Workspace::open(&root)?), 1 << 20);
        let found = restarted.resolve(&hash);
        assert_eq!(found.map(|g| g.hash()).ok(), Some(geo.hash()));
        assert_eq!(
            restarted.resolve(&"a".repeat(64)).err(),
            Some(AssetError::NotFound)
        );
        Ok(())
    }

    #[test]
    fn tampered_assets_are_detected() -> Result<(), FileError> {
        let root = scratch("tamper").map_err(FileError::from)?;
        fs::write(root.join("mesh.stl"), triangle_stl())?;
        fs::write(
            root.join("other.obj"),
            "v 0 0 0\nv 2 0 0\nv 0 2 0\nf 1 2 3\n",
        )?;
        let store = AssetStore::new(Some(Workspace::open(&root)?), 1 << 20);
        let (hash, _) = store.import("mesh.stl")?;
        let (other, _) = store.import("other.obj")?;
        let asset = root.join(ASSET_DIRECTORY);
        // A valid file holding different geometry, and a file that is not valid at all.
        fs::copy(
            asset.join(format!("{other}.ofxg")),
            asset.join(format!("{hash}.ofxg")),
        )?;
        let restarted = AssetStore::new(Some(Workspace::open(&root)?), 1 << 20);
        assert_eq!(restarted.resolve(&hash).err(), Some(AssetError::Corrupt));
        fs::write(asset.join(format!("{other}.ofxg")), b"garbage")?;
        assert_eq!(restarted.resolve(&other).err(), Some(AssetError::Corrupt));
        Ok(())
    }

    #[test]
    fn imports_report_missing_unsupported_oversized_and_unparsable_files() -> Result<(), FileError>
    {
        let root = scratch("errors").map_err(FileError::from)?;
        fs::write(root.join("big.stl"), vec![0u8; 4096])?;
        fs::write(root.join("bad.stl"), b"junk")?;
        fs::write(root.join("notes.txt"), b"hello")?;
        fs::create_dir(root.join("folder.stl"))?;
        let store = AssetStore::new(Some(Workspace::open(&root)?), 1024);
        assert_eq!(store.import("missing.stl").err(), Some(FileError::NotFound));
        assert_eq!(store.import("big.stl").err(), Some(FileError::TooLarge));
        assert_eq!(
            store.import("notes.txt").err(),
            Some(FileError::UnsupportedFormat)
        );
        assert!(matches!(store.import("bad.stl"), Err(FileError::Parse(_))));
        assert!(matches!(
            store.import("folder.stl"),
            Err(FileError::PathRejected(_))
        ));
        Ok(())
    }

    #[test]
    fn exports_are_atomic_refuse_to_overwrite_and_stay_inside() -> Result<(), FileError> {
        let root = scratch("export").map_err(FileError::from)?;
        fs::create_dir(root.join("out"))?;
        fs::write(root.join("mesh.stl"), triangle_stl())?;
        let store = AssetStore::new(Some(Workspace::open(&root)?), 1 << 20);
        let (_, geo) = store.import("mesh.stl")?;
        for (name, format) in [
            ("out/a.obj", MeshFormat::Obj),
            ("out/a.stl", MeshFormat::Stl),
            ("out/a.ofxg", MeshFormat::Ofxg),
        ] {
            let (written, bytes) = store.export(&geo, name, false)?;
            assert_eq!(written, format);
            assert_eq!(fs::metadata(root.join(name))?.len() as usize, bytes);
            // The file reads back as the same geometry through the same path rules.
            assert_eq!(store.import(name)?.1.prim_count(), 1);
        }
        assert_eq!(
            store.export(&geo, "out/a.obj", false).err(),
            Some(FileError::AlreadyExists)
        );
        assert!(store.export(&geo, "out/a.obj", true).is_ok());
        assert_eq!(
            store.export(&geo, "missing/a.obj", false).err(),
            Some(FileError::NotFound)
        );
        assert_eq!(
            store.export(&geo, "out/a.txt", false).err(),
            Some(FileError::UnsupportedFormat)
        );
        fs::create_dir(root.join("out/folder.obj"))?;
        assert_eq!(
            store.export(&geo, "out/folder.obj", true).err(),
            Some(FileError::PathRejected("is a directory"))
        );
        let leftovers = fs::read_dir(root.join("out"))?
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
            .count();
        assert_eq!(leftovers, 0);
        Ok(())
    }

    #[test]
    fn containment_allows_the_workspace_but_not_outside_or_the_asset_store() -> Result<(), FileError>
    {
        let root = scratch("contained").map_err(FileError::from)?;
        let workspace = Workspace::open(&root)?;
        let inside = workspace.root.join("sub/a.stl");
        assert_eq!(workspace.contained(&inside), Ok(()));
        let escaped = Err(FileError::PathRejected("leaves the workspace"));
        let sibling = workspace.root.with_file_name("outside.stl");
        assert_eq!(workspace.contained(&sibling), escaped);
        let asset = workspace.assets.join("a.ofxg");
        assert_eq!(workspace.contained(&asset), escaped);
        Ok(())
    }

    #[cfg(unix)]
    fn link_directory(target: &Path, link: &Path) -> bool {
        std::os::unix::fs::symlink(target, link).is_ok()
    }

    /// Junctions redirect like directory symlinks but need no privileges, so this always runs.
    #[cfg(windows)]
    fn link_directory(target: &Path, link: &Path) -> bool {
        std::process::Command::new("cmd")
            .arg("/C")
            .arg("mklink")
            .arg("/J")
            .arg(link)
            .arg(target)
            .output()
            .is_ok_and(|output| output.status.success())
    }

    #[cfg(unix)]
    fn link_file(target: &Path, link: &Path) -> bool {
        std::os::unix::fs::symlink(target, link).is_ok()
    }

    /// File symlinks can need privileges on Windows; Linux CI always covers this path.
    #[cfg(windows)]
    fn link_file(target: &Path, link: &Path) -> bool {
        std::os::windows::fs::symlink_file(target, link).is_ok()
    }

    #[test]
    fn directory_links_cannot_escape_the_workspace() -> Result<(), FileError> {
        let outside = scratch("outside-dir").map_err(FileError::from)?;
        let root = scratch("links-dir").map_err(FileError::from)?;
        fs::write(outside.join("secret.stl"), triangle_stl())?;
        fs::write(root.join("inside.stl"), triangle_stl())?;
        let store = AssetStore::new(Some(Workspace::open(&root)?), 1 << 20);
        assert!(
            link_directory(&outside, &root.join("door")),
            "link not created"
        );
        let escaped = Some(FileError::PathRejected("leaves the workspace"));
        assert_eq!(store.import("door/secret.stl").err(), escaped);
        let (_, geo) = store.import("inside.stl")?;
        assert_eq!(store.export(&geo, "door/planted.obj", false).err(), escaped);
        assert!(!outside.join("planted.obj").exists());
        // A link pointing at the store itself cannot expose it either.
        assert!(link_directory(
            &root.join(ASSET_DIRECTORY),
            &root.join("vault")
        ));
        assert_eq!(
            store.export(&geo, "vault/planted.ofxg", false).err(),
            escaped
        );
        Ok(())
    }

    #[test]
    fn file_links_cannot_escape_or_be_written_through() -> Result<(), FileError> {
        let outside = scratch("outside-file").map_err(FileError::from)?;
        let root = scratch("links-file").map_err(FileError::from)?;
        fs::write(outside.join("secret.stl"), triangle_stl())?;
        fs::write(root.join("inside.stl"), triangle_stl())?;
        let store = AssetStore::new(Some(Workspace::open(&root)?), 1 << 20);
        if !link_file(&outside.join("secret.stl"), &root.join("leak.stl")) {
            return Ok(());
        }
        let escaped = Some(FileError::PathRejected("leaves the workspace"));
        assert_eq!(store.import("leak.stl").err(), escaped);
        let (_, geo) = store.import("inside.stl")?;
        // Replacing a link swaps the link itself, leaving the outside file intact.
        let before = fs::read(outside.join("secret.stl"))?;
        store.export(&geo, "leak.stl", true)?;
        assert_eq!(fs::read(outside.join("secret.stl"))?, before);
        Ok(())
    }
}
