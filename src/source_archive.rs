//! Safe archive extraction with bounded size and count and per-format root policies.
use alloc::borrow::ToOwned;
use alloc::collections::VecDeque;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::io::Read as _;
use std::path::{Component, Path, PathBuf};

/// Total uncompressed bytes a safe extraction accepts.
const MAX_EXTRACT_BYTES: u64 = 256 * 1024 * 1024;

/// Total entries a safe extraction accepts.
const MAX_EXTRACT_FILES: u64 = 65_536;
/// Decoded TAR budget including block headers.
const MAX_TAR_BYTES: u64 = MAX_EXTRACT_BYTES + MAX_EXTRACT_FILES * 1024;
const LINK_DEADLINE: std::time::Duration = std::time::Duration::from_secs(30);

/// The archive format and its root and link policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ArchiveKind {
    /// A published crate archive. Root tracking uses UTF-8 names only, and every link is refused.
    Crate,
    /// A GitHub snapshot. Every entry must sit under one root directory, and links resolve inside it.
    GitHub,
}

/// One failure of a safe archive extraction.
#[derive(Debug, thiserror::Error)]
pub(crate) enum ArchiveError {
    /// The archive or an extracted entry could not be read or written.
    #[error("I/O: {0}")]
    Io(#[from] io::Error),
    /// An entry has an absolute path, escapes the root or has a special file type.
    #[error("unsafe entry {0} in archive")]
    UnsafeEntry(String),
    /// The archive exceeds the byte or entry limit.
    #[error("archive exceeds the extraction limit")]
    Oversized,
    /// The archive does not hold a usable root directory.
    #[error("archive has no single root directory")]
    SingleRoot,
    /// Link resolution outlived its deadline.
    #[error("archive link resolution exceeded its deadline")]
    LinkDeadline,
    #[cfg(not(any(unix, windows)))]
    /// Symbolic links are unsupported on this platform.
    #[error("symbolic links are unsupported on this platform")]
    SymlinksUnsupported,
}

/// Why a GitHub link was left out of the snapshot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LinkSkip {
    /// The target resolves outside the snapshot root, lexically or physically.
    Escapes,
    /// No pass of link creation made the target exist.
    Unresolved,
}

/// One GitHub link left out of the snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SkippedLink {
    /// The link path relative to the snapshot root.
    pub(crate) path: PathBuf,
    /// Why the link was left out.
    pub(crate) reason: LinkSkip,
}

impl SkippedLink {
    /// The skipped `link`, its archive root stripped.
    fn new(link: &PendingLink, reason: LinkSkip) -> Self {
        Self {
            path: link.path.components().skip(1).collect(),
            reason,
        }
    }
}

impl core::fmt::Display for SkippedLink {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let reason = match self.reason {
            LinkSkip::Escapes => "escapes the snapshot",
            LinkSkip::Unresolved => "cannot be resolved",
        };
        write!(
            formatter,
            "archive link \"{}\" {reason}",
            self.path.to_string_lossy().escape_debug()
        )
    }
}

/// One link held back until the regular files are extracted.
struct PendingLink {
    /// The link path inside the archive root.
    path: PathBuf,
    /// The link target as written in the archive.
    target: PathBuf,
    /// A symbolic link, rather than a hard link.
    symbolic: bool,
    location: PathBuf,
    candidate: PathBuf,
}

/// Extract a gzip tar archive into `destination`, an atomically published root directory,
/// returning the links left out of it.
///
/// Entries with absolute paths or parent escapes are refused. `ArchiveKind::Crate` tracks
/// UTF-8 names only for the root, refuses every link and hands other entries to `tar`.
/// `ArchiveKind::GitHub` requires every entry under one root directory, refuses special
/// entries and creates confined links after the regular files, skipping the others. An
/// existing `destination` directory is the completed extraction.
#[must_use = "Use the extraction as the archive outcome"]
pub(crate) fn extract_archive(
    archive: &Path,
    destination: impl AsRef<Path>,
    kind: ArchiveKind,
) -> Result<Vec<SkippedLink>, ArchiveError> {
    let destination = destination.as_ref();
    if destination.is_dir() {
        return Ok(Vec::new());
    }
    let file = fs::File::open(archive)?;
    let decoder = flate2::read::GzDecoder::new(file);
    let mut tar = tar::Archive::new(decoder.take(MAX_TAR_BYTES + 1));
    let stage = stage_beside(destination)?;
    let temp = stage.path();
    let mut total: u64 = 0;
    let mut count: u64 = 0;
    let mut top: Option<OsString> = None;
    let mut pending: Vec<PendingLink> = Vec::new();
    for entry in tar.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.to_path_buf();
        let role = entry_role(&path, entry.header().entry_type(), kind)?;
        total = total.saturating_add(entry.size());
        count = count.saturating_add(1);
        if total > MAX_EXTRACT_BYTES || count > MAX_EXTRACT_FILES {
            return Err(ArchiveError::Oversized);
        }
        if role == EntryRole::Metadata {
            continue;
        }
        if kind == ArchiveKind::GitHub {
            track_github_root(&path, &mut top)?;
        }
        let location = temp.join(&path);
        if let Some(parent) = location.parent() {
            fs::create_dir_all(parent)?;
        }
        if let EntryRole::Link { symbolic } = role {
            let target = entry
                .link_name()?
                .ok_or_else(|| ArchiveError::UnsafeEntry(path.display().to_string()))?
                .into_owned();
            let candidate = if symbolic {
                location
                    .parent()
                    .ok_or(ArchiveError::SingleRoot)?
                    .join(&target)
            } else {
                temp.join(&target)
            };
            pending.push(PendingLink {
                path,
                target,
                symbolic,
                location,
                candidate,
            });
            continue;
        }
        entry.unpack(location)?;
        if kind == ArchiveKind::Crate {
            track_top(&path, &mut top);
        }
    }
    if tar.into_inner().limit() == 0 {
        return Err(ArchiveError::Oversized);
    }
    let root = temp.join(top.ok_or(ArchiveError::SingleRoot)?);
    if !root.is_dir() {
        return Err(ArchiveError::SingleRoot);
    }
    let skipped = if kind == ArchiveKind::GitHub {
        resolve_confined_links(&root, pending)?
    } else {
        Vec::new()
    };
    match fs::rename(&root, destination) {
        Ok(()) => {}
        Err(_) if destination.is_dir() => {}
        Err(error) => return Err(ArchiveError::Io(error)),
    }
    Ok(skipped)
}

/// How one validated entry is extracted.
#[derive(Clone, Copy, PartialEq, Eq)]
enum EntryRole {
    /// A regular file or directory, unpacked immediately.
    Contents,
    /// A symbolic or hard link, created after the regular files.
    Link {
        /// A symbolic link, rather than a hard link.
        symbolic: bool,
    },
    /// A GitHub global header, which carries metadata and no file.
    Metadata,
}

/// The role of one entry, refusing escaping paths, links in crates and special GitHub entries.
fn entry_role(
    path: &Path,
    entry_type: tar::EntryType,
    kind: ArchiveKind,
) -> Result<EntryRole, ArchiveError> {
    let unsafe_entry = || ArchiveError::UnsafeEntry(path.display().to_string());
    if path.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return Err(unsafe_entry());
    }
    let symbolic = entry_type.is_symlink();
    if symbolic || entry_type.is_hard_link() {
        return match kind {
            ArchiveKind::Crate => Err(unsafe_entry()),
            ArchiveKind::GitHub => Ok(EntryRole::Link { symbolic }),
        };
    }
    match kind {
        ArchiveKind::Crate => Ok(EntryRole::Contents),
        ArchiveKind::GitHub if entry_type.is_file() || entry_type.is_dir() => {
            Ok(EntryRole::Contents)
        }
        ArchiveKind::GitHub if entry_type.is_pax_global_extensions() => Ok(EntryRole::Metadata),
        ArchiveKind::GitHub => Err(unsafe_entry()),
    }
}

/// Require every GitHub entry under the first entry's root directory.
fn track_github_root(path: &Path, top: &mut Option<OsString>) -> Result<(), ArchiveError> {
    let first = path
        .components()
        .find_map(|component| match component {
            Component::Normal(name) => Some(name),
            _ => None,
        })
        .ok_or(ArchiveError::SingleRoot)?;
    match top.as_deref() {
        Some(expected) if expected != first => Err(ArchiveError::SingleRoot),
        Some(_) => Ok(()),
        None => {
            *top = Some(first.to_owned());
            Ok(())
        }
    }
}

/// A staging directory beside `destination`, so publication is one rename.
fn stage_beside(destination: &Path) -> Result<tempfile::TempDir, ArchiveError> {
    let parent = destination
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "extraction destination has no parent directory",
            )
        })?;
    fs::create_dir_all(parent)?;
    Ok(tempfile::Builder::new()
        .prefix("dejadoc-")
        .tempdir_in(parent)?)
}

/// The crate root selection, the last tracked UTF-8 top name.
fn track_top(path: &Path, top: &mut Option<OsString>) {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return;
    };
    if name == ".crate" || name == "dejadoc-extract" {
        return;
    }
    if let Some(first) = path
        .components()
        .next()
        .and_then(|component| component.as_os_str().to_str())
        && top.as_deref() != Some(std::ffi::OsStr::new(first))
    {
        *top = Some(first.into());
    }
}

/// Create the deferred links that stay inside the root lexically and physically, returning
/// the skipped ones.
fn resolve_confined_links(
    root: &Path,
    links: Vec<PendingLink>,
) -> Result<Vec<SkippedLink>, ArchiveError> {
    let mut skipped = Vec::new();
    if links.is_empty() {
        return Ok(skipped);
    }
    let started = std::time::Instant::now();
    let root = fs::canonicalize(root)?;
    let mut queue = VecDeque::with_capacity(links.len());
    for link in links {
        let parent = link.location.parent().ok_or(ArchiveError::SingleRoot)?;
        if confined_target(&root, &link) && fs::canonicalize(parent)?.starts_with(&root) {
            queue.push_back(link);
        } else {
            skipped.push(SkippedLink::new(&link, LinkSkip::Escapes));
        }
    }
    let mut deferred = 0usize;
    while let Some(link) = queue.pop_front() {
        if started.elapsed() >= LINK_DEADLINE {
            return Err(ArchiveError::LinkDeadline);
        }
        // Any resolution failure means the target does not exist inside the stage yet.
        let Ok(target) = fs::canonicalize(&link.candidate) else {
            deferred += 1;
            if deferred > queue.len() {
                skipped.push(SkippedLink::new(&link, LinkSkip::Unresolved));
                skipped.extend(
                    queue
                        .drain(..)
                        .map(|link| SkippedLink::new(&link, LinkSkip::Unresolved)),
                );
                break;
            }
            queue.push_back(link);
            continue;
        };
        if !target.starts_with(&root) {
            skipped.push(SkippedLink::new(&link, LinkSkip::Escapes));
            continue;
        }
        if link.symbolic {
            create_symlink(&link.target, &link.location, &target)?;
        } else {
            fs::hard_link(target, &link.location)?;
        }
        deferred = 0;
    }
    Ok(skipped)
}

/// Create a symbolic link, a directory link on Windows when the target is a directory.
fn create_symlink(target: &Path, link: &Path, resolved: &Path) -> Result<(), ArchiveError> {
    #[cfg(unix)]
    {
        let _ = resolved;
        std::os::unix::fs::symlink(target, link)?;
        Ok(())
    }
    #[cfg(windows)]
    {
        if fs::metadata(resolved)?.is_dir() {
            std::os::windows::fs::symlink_dir(target, link)?;
        } else {
            std::os::windows::fs::symlink_file(target, link)?;
        }
        Ok(())
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (target, link, resolved);
        Err(ArchiveError::SymlinksUnsupported)
    }
}

/// Whether the link target stays inside the root lexically, symbolic targets relative to the
/// link's parent and hard-link targets relative to the archive.
fn confined_target(root: &Path, link: &PendingLink) -> bool {
    let mut components = link.target.components();
    let mut depth = if link.symbolic {
        link.path
            .components()
            .filter(|part| matches!(part, Component::Normal(_)))
            .count()
            .saturating_sub(2)
    } else {
        match components.find(|part| !matches!(part, Component::CurDir)) {
            Some(Component::Normal(name)) if Some(name) == root.file_name() => 0,
            _ => return false,
        }
    };
    for component in components {
        match component {
            Component::CurDir => {}
            Component::ParentDir if depth > 0 => depth -= 1,
            Component::Normal(_) => depth = depth.saturating_add(1),
            _ => return false,
        }
    }
    true
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use alloc::format;
    use alloc::vec;
    #[cfg(unix)]
    use std::os::unix::ffi::OsStringExt as _;

    /// A gzip tar archive with explicit entry types, `0` for regular files, and paths written
    /// unvalidated into the header.
    pub(crate) fn packed(entries: Vec<(u8, std::ffi::OsString, String, Vec<u8>)>) -> Vec<u8> {
        let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        let mut tar = tar::Builder::new(encoder);
        for (entry_type, path, link, data) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_size(u64::try_from(data.len()).unwrap());
            header.set_mode(if entry_type == b'5' { 0o755 } else { 0o644 });
            if entry_type != 0 {
                header.set_entry_type(tar::EntryType::new(entry_type));
                if !link.is_empty() {
                    header.set_link_name(&link).unwrap();
                }
            }
            let bytes = path.as_encoded_bytes();
            let name = &mut header.as_old_mut().name;
            name[..bytes.len()].copy_from_slice(bytes);
            header.set_cksum();
            tar.append(&header, data.as_slice()).unwrap();
        }
        let encoder = tar.into_inner().unwrap();
        encoder.finish().unwrap()
    }

    /// A safe archive with one root and one regular file.
    fn safe_archive(root: &str) -> Vec<u8> {
        packed(vec![
            (b'5', format!("{root}/").into(), String::new(), Vec::new()),
            (
                0,
                format!("{root}/src/lib.rs").into(),
                String::new(),
                b"pub fn f() {}\n".to_vec(),
            ),
        ])
    }

    fn archive_file(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn unusable_destinations_are_refused_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let archive = archive_file(dir.path(), "dep.crate", &safe_archive("dep-1.0.0"));
        let error = extract_archive(&archive, Path::new("nv"), ArchiveKind::Crate).unwrap_err();
        assert!(
            matches!(error, ArchiveError::Io(source) if source.kind() == io::ErrorKind::InvalidInput)
        );
        let taken = dir.path().join("out/taken");
        std::fs::create_dir_all(taken.parent().unwrap()).unwrap();
        std::fs::write(&taken, "a file").unwrap();
        assert!(extract_archive(&archive, &taken, ArchiveKind::Crate).is_err());
        assert_eq!(std::fs::read_to_string(&taken).unwrap(), "a file");
    }

    #[test]
    fn a_single_root_archive_is_extracted() {
        let dir = tempfile::tempdir().unwrap();
        let archive = archive_file(dir.path(), "dep.crate", &safe_archive("dep-1.0.0"));
        let dest = dir.path().join("out").join("dep-1.0.0");
        extract_archive(&archive, &dest, ArchiveKind::Crate).expect("extract the archive");
        assert_eq!(
            std::fs::read_to_string(dest.join("src/lib.rs")).unwrap(),
            "pub fn f() {}\n"
        );
        assert_eq!(std::fs::read_dir(&dest).unwrap().count(), 1);
    }

    #[test]
    fn a_github_global_header_is_metadata_and_not_a_root() {
        let dir = tempfile::tempdir().unwrap();
        let comment = format!("52 comment={}\n", "a".repeat(40));
        let archive = archive_file(
            dir.path(),
            "snapshot.tar.gz",
            &packed(vec![
                (
                    b'g',
                    "pax_global_header".into(),
                    String::new(),
                    comment.into_bytes(),
                ),
                (b'5', "o-app-abc1234/".into(), String::new(), Vec::new()),
                (
                    0,
                    "o-app-abc1234/src/lib.rs".into(),
                    String::new(),
                    b"pub fn f() {}\n".to_vec(),
                ),
            ]),
        );
        let dest = dir.path().join("snapshot");
        extract_archive(&archive, &dest, ArchiveKind::GitHub).expect("extract the snapshot");
        assert_eq!(
            std::fs::read_to_string(dest.join("src/lib.rs")).unwrap(),
            "pub fn f() {}\n"
        );
    }

    #[test]
    fn unsafe_entries_are_rejected_and_nothing_is_published() {
        let dir = tempfile::tempdir().unwrap();
        for (note, entries) in [
            (
                "absolute",
                vec![(0, "/etc/evil".into(), String::new(), b"x".to_vec())],
            ),
            (
                "parent",
                vec![(0, "top/../escape".into(), String::new(), b"x".to_vec())],
            ),
            (
                "fifo",
                vec![(b'6', "top/pipe".into(), String::new(), Vec::new())],
            ),
            (
                "file-top",
                vec![(0, "top".into(), String::new(), b"x".to_vec())],
            ),
            ("empty", Vec::new()),
        ] {
            let archive = archive_file(
                dir.path(),
                format!("{note}.crate").as_str(),
                &packed(entries),
            );
            let dest = dir.path().join("out").join(note);
            let kind = if note == "fifo" {
                ArchiveKind::GitHub
            } else {
                ArchiveKind::Crate
            };
            let error = extract_archive(&archive, &dest, kind).unwrap_err();
            match note {
                "file-top" | "empty" => {
                    assert!(matches!(error, ArchiveError::SingleRoot), "{note} {error}");
                }
                _ => assert!(
                    matches!(error, ArchiveError::UnsafeEntry(_)),
                    "{note} {error}"
                ),
            }
            assert!(!dest.exists(), "{note}");
        }
    }

    #[test]
    fn the_crate_kind_unpacks_unknown_entry_types_as_files() {
        let dir = tempfile::tempdir().unwrap();
        let archive = archive_file(
            dir.path(),
            "fifo.crate",
            &packed(vec![
                (b'5', "top/".into(), String::new(), Vec::new()),
                (b'6', "top/pipe".into(), String::new(), b"x".to_vec()),
            ]),
        );
        let dest = dir.path().join("out");
        extract_archive(&archive, &dest, ArchiveKind::Crate).unwrap();
        assert_eq!(std::fs::read(dest.join("pipe")).unwrap(), b"x");
    }

    /// The skipped links of an extraction as sorted path and reason pairs.
    fn skips(skipped: &[SkippedLink]) -> Vec<(&Path, LinkSkip)> {
        let mut pairs: Vec<_> = skipped
            .iter()
            .map(|link| (link.path.as_path(), link.reason))
            .collect();
        pairs.sort_unstable_by_key(|(path, _)| *path);
        pairs
    }

    #[test]
    fn a_github_snapshot_requires_one_root_for_every_entry() {
        let dir = tempfile::tempdir().unwrap();
        let two_roots = vec![
            (0, "a/f".into(), String::new(), b"x".to_vec()),
            (0, "b/g".into(), String::new(), b"y".to_vec()),
        ];
        let archive = archive_file(dir.path(), "two.crate", &packed(two_roots));
        let error = extract_archive(
            &archive,
            dir.path().join("out").join("two"),
            ArchiveKind::GitHub,
        )
        .unwrap_err();
        assert!(matches!(error, ArchiveError::SingleRoot), "{error}");
    }

    /// One directory name byte that is not valid UTF-8.
    #[cfg(unix)]
    fn non_utf8() -> std::ffi::OsString {
        std::ffi::OsString::from_vec(vec![0xFFu8])
    }

    #[cfg(unix)]
    #[test]
    fn a_non_utf8_sibling_keeps_the_package_root() {
        let dir = tempfile::tempdir().unwrap();
        let mixed = vec![
            (
                0,
                "top/src/lib.rs".into(),
                String::new(),
                b"pub fn f() {}\n".to_vec(),
            ),
            (0, "top/inner".into(), String::new(), b"x".to_vec()),
            (
                0,
                PathBuf::from("top").join(non_utf8()).into_os_string(),
                String::new(),
                b"w".to_vec(),
            ),
            (
                0,
                PathBuf::from(non_utf8()).join("y").into_os_string(),
                String::new(),
                b"z".to_vec(),
            ),
        ];
        let archive = archive_file(dir.path(), "mixed.crate", &packed(mixed));
        let error = extract_archive(
            &archive,
            dir.path().join("out").join("mixed"),
            ArchiveKind::GitHub,
        )
        .unwrap_err();
        assert!(matches!(error, ArchiveError::SingleRoot), "{error}");
        extract_archive(
            &archive,
            dir.path().join("out").join("mixed-crate"),
            ArchiveKind::Crate,
        )
        .expect("crate selection keeps the UTF-8 root");
        let dest = dir.path().join("out").join("mixed-crate");
        assert!(dest.join("src/lib.rs").exists());
        assert_eq!(std::fs::read(dest.join(non_utf8())).unwrap(), b"w");
        assert!(!dest.join("y").exists());
    }

    #[test]
    fn the_crate_kind_keeps_the_last_root_and_skips_the_markers() {
        let dir = tempfile::tempdir().unwrap();
        let last_wins = vec![
            (0, "a/f".into(), String::new(), b"x".to_vec()),
            (0, "b/g".into(), String::new(), b"y".to_vec()),
        ];
        let archive = archive_file(dir.path(), "last.crate", &packed(last_wins));
        let dest = dir.path().join("out").join("last");
        extract_archive(&archive, &dest, ArchiveKind::Crate)
            .expect("crate selection keeps the last root");
        assert_eq!(std::fs::read_to_string(dest.join("g")).unwrap(), "y");
        assert!(!dest.join("f").exists());
        let markers = vec![
            (
                0,
                "top/src/lib.rs".into(),
                String::new(),
                b"pub fn f() {}\n".to_vec(),
            ),
            (0, ".crate".into(), String::new(), b"x".to_vec()),
            (0, "dejadoc-extract".into(), String::new(), b"x".to_vec()),
        ];
        let archive = archive_file(dir.path(), "markers.crate", &packed(markers));
        let dest = dir.path().join("out").join("markers");
        extract_archive(&archive, &dest, ArchiveKind::Crate)
            .expect("the marker basename does not displace the root");
        assert_eq!(
            std::fs::read_to_string(dest.join("src/lib.rs")).unwrap(),
            "pub fn f() {}\n"
        );
    }

    #[test]
    fn the_extraction_limits_refuse_oversized_archives() {
        let dir = tempfile::tempdir().unwrap();
        let archive = archive_file(
            dir.path(),
            "huge.crate",
            &packed(vec![(
                0,
                "top/blob".into(),
                String::new(),
                vec![0u8; 300_000_000],
            )]),
        );
        let error = extract_archive(
            &archive,
            dir.path().join("out").join("huge"),
            ArchiveKind::Crate,
        )
        .unwrap_err();
        assert!(matches!(error, ArchiveError::Oversized), "{error}");
        let entries: Vec<(u8, std::ffi::OsString, String, Vec<u8>)> = (0..=MAX_EXTRACT_FILES)
            .map(|i| (0, format!("top/f{i}").into(), String::new(), vec![b'x']))
            .collect();
        let archive = archive_file(dir.path(), "many.crate", &packed(entries));
        let error = extract_archive(
            &archive,
            dir.path().join("out").join("many"),
            ArchiveKind::Crate,
        )
        .unwrap_err();
        assert!(matches!(error, ArchiveError::Oversized), "{error}");
    }

    /// A gzip tar of one PAX record of `metadata` bytes, one zero-filled `top/blob` of
    /// `payload` bytes and one end block, both sizes multiples of 512.
    fn padded_archive(metadata: u64, payload: u64) -> Vec<u8> {
        use std::io::{Read as _, Write as _};
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        let mut pax = tar::Header::new_ustar();
        pax.set_entry_type(tar::EntryType::XHeader);
        pax.set_size(metadata);
        pax.set_cksum();
        encoder.write_all(pax.as_bytes()).unwrap();
        let length = metadata.to_string();
        let value = metadata - u64::try_from(length.len() + " comment=\n".len()).unwrap();
        write!(encoder, "{length} comment=").unwrap();
        std::io::copy(&mut std::io::repeat(b'a').take(value), &mut encoder).unwrap();
        encoder.write_all(b"\n").unwrap();
        let mut file = tar::Header::new_gnu();
        file.set_path("top/blob").unwrap();
        file.set_size(payload);
        file.set_mode(0o644);
        file.set_cksum();
        encoder.write_all(file.as_bytes()).unwrap();
        std::io::copy(&mut std::io::repeat(0).take(payload), &mut encoder).unwrap();
        encoder.write_all(&[0u8; 512]).unwrap();
        encoder.finish().unwrap()
    }

    /// The documented 256 MiB payload limit, spelled out so the boundary tests pin it.
    const PAYLOAD_LIMIT: u64 = 268_435_456;

    /// The documented 320 MiB decoded-stream limit.
    const DECODED_LIMIT: u64 = 335_544_320;

    #[test]
    fn an_archive_at_the_byte_limits_is_extracted() {
        let dir = tempfile::tempdir().unwrap();
        let metadata = DECODED_LIMIT - PAYLOAD_LIMIT - 3 * 512;
        let archive = archive_file(
            dir.path(),
            "full.crate",
            &padded_archive(metadata, PAYLOAD_LIMIT),
        );
        let dest = dir.path().join("out").join("full");
        extract_archive(&archive, &dest, ArchiveKind::Crate).expect("the limits are inclusive");
        assert_eq!(
            std::fs::metadata(dest.join("blob")).unwrap().len(),
            PAYLOAD_LIMIT
        );
    }

    #[test]
    fn metadata_past_the_decoded_limit_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let metadata = DECODED_LIMIT - PAYLOAD_LIMIT - 2 * 512;
        let archive = archive_file(
            dir.path(),
            "padded.crate",
            &padded_archive(metadata, PAYLOAD_LIMIT),
        );
        let dest = dir.path().join("out").join("padded");
        assert!(extract_archive(&archive, &dest, ArchiveKind::Crate).is_err());
        assert!(!dest.exists());
    }

    #[test]
    fn an_archive_at_the_entry_limit_is_extracted() {
        let dir = tempfile::tempdir().unwrap();
        let entries: Vec<(u8, std::ffi::OsString, String, Vec<u8>)> = (0..MAX_EXTRACT_FILES)
            .map(|i| (0, format!("top/f{i}").into(), String::new(), vec![b'x']))
            .collect();
        let archive = archive_file(dir.path(), "full-count.crate", &packed(entries));
        let dest = dir.path().join("out").join("full-count");
        extract_archive(&archive, &dest, ArchiveKind::Crate).expect("the entry limit is inclusive");
        assert!(dest.join(format!("f{}", MAX_EXTRACT_FILES - 1)).is_file());
    }

    #[cfg(unix)]
    #[test]
    fn a_hard_link_into_another_archive_root_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let archive = archive_file(
            dir.path(),
            "hard.tar.gz",
            &packed(vec![
                (b'5', "top/".into(), String::new(), Vec::new()),
                (0, "top/file".into(), String::new(), b"content".to_vec()),
                (b'1', "top/hard".into(), "other/file".into(), Vec::new()),
            ]),
        );
        let destination = dir.path().join("snapshot");
        let skipped = extract_archive(&archive, &destination, ArchiveKind::GitHub).unwrap();
        assert_eq!(skips(&skipped), [(Path::new("hard"), LinkSkip::Escapes)]);
        assert!(std::fs::symlink_metadata(destination.join("hard")).is_err());
    }

    #[test]
    fn the_crate_kind_refuses_links() {
        let dir = tempfile::tempdir().unwrap();
        for (note, entry) in [
            (
                "symlink",
                (b'2', "top/link".to_owned(), "src/lib.rs".to_owned()),
            ),
            (
                "hardlink",
                (b'1', "top/link".to_owned(), "src/lib.rs".to_owned()),
            ),
        ] {
            let archive = archive_file(
                dir.path(),
                &format!("{note}.crate"),
                &packed(vec![
                    (
                        0,
                        "top/src/lib.rs".into(),
                        String::new(),
                        b"pub fn f() {}\n".to_vec(),
                    ),
                    (entry.0, entry.1.into(), entry.2.clone(), Vec::new()),
                ]),
            );
            let error = extract_archive(
                &archive,
                dir.path().join("out").join(note),
                ArchiveKind::Crate,
            )
            .unwrap_err();
            assert!(
                matches!(error, ArchiveError::UnsafeEntry(_)),
                "{note} {error}"
            );
            assert!(!dir.path().join("out").join(note).exists(), "{note}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn confined_links_are_created_after_the_regular_files() {
        let dir = tempfile::tempdir().unwrap();
        let archive = archive_file(
            dir.path(),
            "org.crate",
            &packed(vec![
                (b'5', "o-app-abc1234/".into(), String::new(), Vec::new()),
                (
                    0,
                    "o-app-abc1234/src/lib.rs".into(),
                    String::new(),
                    b"pub fn f() {}\n".to_vec(),
                ),
                (
                    b'2',
                    "o-app-abc1234/forward.rs".into(),
                    "alias.rs".into(),
                    Vec::new(),
                ),
                (
                    b'2',
                    "o-app-abc1234/source".into(),
                    "src".into(),
                    Vec::new(),
                ),
                (
                    b'2',
                    "o-app-abc1234/alias.rs".into(),
                    "src/lib.rs".to_owned(),
                    Vec::new(),
                ),
                (
                    b'2',
                    "o-app-abc1234/deep/sibling.rs".into(),
                    "../src/lib.rs".to_owned(),
                    Vec::new(),
                ),
                (
                    b'1',
                    "o-app-abc1234/hard.rs".into(),
                    "o-app-abc1234/src/lib.rs".to_owned(),
                    Vec::new(),
                ),
            ]),
        );
        let dest = dir.path().join("out").join("o-app-abc1234");
        extract_archive(&archive, &dest, ArchiveKind::GitHub).expect("extract the archive");
        assert_eq!(
            std::fs::read_to_string(dest.join("forward.rs")).unwrap(),
            "pub fn f() {}\n"
        );
        assert_eq!(
            std::fs::read_to_string(dest.join("source/lib.rs")).unwrap(),
            "pub fn f() {}\n"
        );
        let alias = dest.join("alias.rs");
        assert!(
            std::fs::symlink_metadata(&alias)
                .unwrap()
                .file_type()
                .is_symlink(),
            "the alias is a symbolic link"
        );
        assert_eq!(
            std::fs::read_to_string(&alias).unwrap(),
            "pub fn f() {}\n",
            "the alias resolves through the regular file"
        );
        assert_eq!(
            std::fs::read_to_string(dest.join("deep/sibling.rs")).unwrap(),
            "pub fn f() {}\n",
            "the sibling link climbs one level inside the root"
        );
        let hard = std::fs::metadata(dest.join("hard.rs")).unwrap();
        assert!(
            !hard.file_type().is_symlink(),
            "the hard link is a regular file"
        );
        assert_eq!(
            std::fs::read_to_string(dest.join("hard.rs")).unwrap(),
            "pub fn f() {}\n"
        );
    }

    #[test]
    fn confined_links_that_escape_the_root_are_skipped() {
        let dir = tempfile::tempdir().unwrap();
        for (note, target) in [
            ("climb", "../../evil"),
            ("absolute", "/etc/passwd"),
            ("deep-climb", "src/../../../../evil"),
        ] {
            let archive = archive_file(
                dir.path(),
                &format!("{note}.crate"),
                &packed(vec![
                    (
                        0,
                        "top/src/lib.rs".into(),
                        String::new(),
                        b"pub fn f() {}\n".to_vec(),
                    ),
                    (b'2', "top/link".into(), target.to_owned(), Vec::new()),
                ]),
            );
            let dest = dir.path().join("out").join(note);
            let skipped = extract_archive(&archive, &dest, ArchiveKind::GitHub).unwrap();
            assert_eq!(
                skips(&skipped),
                [(Path::new("link"), LinkSkip::Escapes)],
                "{note}"
            );
            assert!(dest.join("src/lib.rs").is_file(), "{note}");
            assert!(
                std::fs::symlink_metadata(dest.join("link")).is_err(),
                "{note}"
            );
        }
    }

    #[test]
    fn an_existing_destination_is_reused_without_reextraction() {
        let dir = tempfile::tempdir().unwrap();
        let safe = archive_file(dir.path(), "dep.crate", &safe_archive("dep-1.0.0"));
        let dest = dir.path().join("out").join("dep-1.0.0");
        extract_archive(&safe, &dest, ArchiveKind::Crate).expect("extract the archive");
        let poison = archive_file(
            dir.path(),
            "poison.crate",
            &packed(vec![(0, "/etc/evil".into(), String::new(), b"x".to_vec())]),
        );
        extract_archive(&poison, &dest, ArchiveKind::Crate)
            .expect("the published directory is the completed extraction");
        assert_eq!(
            std::fs::read_to_string(dest.join("src/lib.rs")).unwrap(),
            "pub fn f() {}\n"
        );
    }

    #[test]
    fn a_failed_extraction_leaves_no_stage_behind() {
        let dir = tempfile::tempdir().unwrap();
        let archive = archive_file(
            dir.path(),
            "bad.crate",
            &packed(vec![(0, "/etc/evil".into(), String::new(), b"x".to_vec())]),
        );
        let parent = dir.path().join("out");
        std::fs::create_dir_all(&parent).unwrap();
        extract_archive(&archive, parent.join("bad"), ArchiveKind::Crate).unwrap_err();
        assert_eq!(std::fs::read_dir(&parent).unwrap().count(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn a_link_chain_cannot_escape_the_published_root() {
        let dir = tempfile::tempdir().unwrap();
        let archive = archive_file(
            dir.path(),
            "chain.tar.gz",
            &packed(vec![
                (b'5', "top/".into(), String::new(), Vec::new()),
                (0, "top/file".into(), String::new(), b"content".to_vec()),
                (b'2', "top/alias".into(), ".".into(), Vec::new()),
                (b'2', "top/escape".into(), "alias/..".into(), Vec::new()),
            ]),
        );
        let destination = dir.path().join("snapshot");
        let skipped = extract_archive(&archive, &destination, ArchiveKind::GitHub).unwrap();
        assert_eq!(skips(&skipped), [(Path::new("escape"), LinkSkip::Escapes)]);
        assert_eq!(
            std::fs::read(destination.join("alias/file")).unwrap(),
            b"content"
        );
        assert!(std::fs::symlink_metadata(destination.join("escape")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn unresolvable_and_dangling_links_are_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let archive = archive_file(
            dir.path(),
            "cycle.tar.gz",
            &packed(vec![
                (b'5', "top/".into(), String::new(), Vec::new()),
                (0, "top/file".into(), String::new(), b"content".to_vec()),
                (b'2', "top/one".into(), "two".into(), Vec::new()),
                (b'2', "top/two".into(), "one".into(), Vec::new()),
                (b'2', "top/dangling".into(), "missing".into(), Vec::new()),
                (b'2', "top/through".into(), "file/inner".into(), Vec::new()),
                (b'2', "top/alias".into(), "file".into(), Vec::new()),
            ]),
        );
        let destination = dir.path().join("snapshot");
        let skipped = extract_archive(&archive, &destination, ArchiveKind::GitHub).unwrap();
        assert_eq!(
            skips(&skipped),
            [
                (Path::new("dangling"), LinkSkip::Unresolved),
                (Path::new("one"), LinkSkip::Unresolved),
                (Path::new("through"), LinkSkip::Unresolved),
                (Path::new("two"), LinkSkip::Unresolved),
            ]
        );
        assert_eq!(
            std::fs::read(destination.join("alias")).unwrap(),
            b"content"
        );
        for name in ["one", "two", "dangling", "through"] {
            assert!(
                std::fs::symlink_metadata(destination.join(name)).is_err(),
                "{name}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn link_targets_preserve_non_utf8_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let name = non_utf8();
        let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        let mut builder = tar::Builder::new(encoder);
        let mut header = tar::Header::new_gnu();
        header.set_size(7);
        header.set_mode(0o644);
        builder
            .append_data(
                &mut header,
                PathBuf::from("top").join(&name),
                &b"content"[..],
            )
            .unwrap();
        let mut header = tar::Header::new_gnu();
        header.set_size(0);
        header.set_mode(0o777);
        header.set_entry_type(tar::EntryType::Symlink);
        header.set_link_name(&name).unwrap();
        builder
            .append_data(&mut header, "top/alias", &[][..])
            .unwrap();
        let bytes = builder.into_inner().unwrap().finish().unwrap();
        let archive = archive_file(dir.path(), "raw-link.tar.gz", &bytes);
        let destination = dir.path().join("snapshot");
        extract_archive(&archive, &destination, ArchiveKind::GitHub).unwrap();
        assert_eq!(
            std::fs::read(destination.join("alias")).unwrap(),
            b"content"
        );
        assert_eq!(
            std::fs::read_link(destination.join("alias")).unwrap(),
            PathBuf::from(name)
        );
    }

    #[test]
    fn pax_sizes_count_toward_the_payload_limit() {
        use std::io::Read as _;
        let dir = tempfile::tempdir().unwrap();
        let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        let mut builder = tar::Builder::new(encoder);
        let size = MAX_EXTRACT_BYTES + 1;
        let value = size.to_string();
        builder
            .append_pax_extensions([("size", value.as_bytes())])
            .unwrap();
        let mut header = tar::Header::new_gnu();
        header.set_mode(0o644);
        header.set_size(0);
        builder
            .append_data(&mut header, "top/oversized", std::io::repeat(0).take(size))
            .unwrap();
        let bytes = builder.into_inner().unwrap().finish().unwrap();
        let archive = archive_file(dir.path(), "pax-size.crate", &bytes);
        let destination = dir.path().join("snapshot");
        assert!(matches!(
            extract_archive(&archive, &destination, ArchiveKind::Crate),
            Err(ArchiveError::Oversized)
        ));
        assert!(!destination.exists());
    }
}
