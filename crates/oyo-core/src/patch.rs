use git2::DiffLineType;
use std::collections::HashSet;
use std::io::BufRead;
use std::path::Path;
use std::sync::Arc;

use crate::git::FileStatus;
use crate::multi::FileEntry;
use crate::{Change, ChangeSpan, DiffResult, Hunk};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BlobId(git2::Oid);

impl std::fmt::Display for BlobId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

impl BlobId {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        Ok(Self(git2::Oid::hash_object(git2::ObjectType::Blob, bytes)?))
    }

    pub fn parse(value: &str) -> Result<Self> {
        if value.len() != 40
            || !value.bytes().all(|byte| byte.is_ascii_hexdigit())
            || value.bytes().all(|byte| byte == b'0')
        {
            return Err(
                "Full SHA-1 blob identity required; other object formats are unsupported".into(),
            );
        }
        Ok(Self(git2::Oid::from_str(value)?))
    }

    pub fn verify(self, bytes: &[u8]) -> Result<()> {
        if git2::Oid::hash_object(git2::ObjectType::Blob, bytes)? != self.0 {
            return Err("Immutable blob hash mismatch".into());
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlobSide {
    pub id: BlobId,
    pub mode: u32,
}

#[derive(Clone, Debug, Default)]
struct RecordMetadata {
    ids: Option<(String, String)>,
    modes: (Option<u32>, Option<u32>),
}

impl RecordMetadata {
    fn parse(bytes: &[u8]) -> Result<Self> {
        let mut metadata = Self::default();
        let parse_mode = |value: &str| -> Result<u32> {
            let mode = u32::from_str_radix(value.trim(), 8)?;
            if !matches!(mode, 0o100644 | 0o100755 | 0o120000) {
                return Err("Unsupported patch object mode".into());
            }
            Ok(mode)
        };
        let set_mode = |slot: &mut Option<u32>, mode: u32| -> Result<()> {
            if slot.replace(mode).is_some() {
                return Err("Repeated patch object mode".into());
            }
            Ok(())
        };
        for line in bytes.split_inclusive(|byte| *byte == b'\n') {
            if line.starts_with(b"@@ -")
                || line == b"GIT binary patch\n"
                || line.starts_with(b"Binary files ")
            {
                break;
            }
            let line = std::str::from_utf8(line)?.trim_end_matches('\n');
            if let Some(value) = line.strip_prefix("index ") {
                if metadata.ids.is_some() {
                    return Err("Repeated patch object identities".into());
                }
                let mut fields = value.split_whitespace();
                let pair = fields.next().ok_or("Missing patch object identities")?;
                let (old, new) = pair
                    .split_once("..")
                    .ok_or("Invalid patch object identities")?;
                if old.len() != new.len()
                    || !(4..=40).contains(&old.len())
                    || !old
                        .bytes()
                        .chain(new.bytes())
                        .all(|byte| byte.is_ascii_hexdigit())
                {
                    return Err("Unsupported or malformed patch object identities".into());
                }
                metadata.ids = Some((old.to_ascii_lowercase(), new.to_ascii_lowercase()));
                if let Some(value) = fields.next() {
                    let mode = parse_mode(value)?;
                    set_mode(&mut metadata.modes.0, mode)?;
                    set_mode(&mut metadata.modes.1, mode)?;
                }
                if fields.next().is_some() {
                    return Err("Unexpected index header data".into());
                }
            } else if let Some(value) = line.strip_prefix("old mode ") {
                set_mode(&mut metadata.modes.0, parse_mode(value)?)?;
            } else if let Some(value) = line.strip_prefix("new mode ") {
                set_mode(&mut metadata.modes.1, parse_mode(value)?)?;
            } else if let Some(value) = line.strip_prefix("new file mode ") {
                set_mode(&mut metadata.modes.0, 0)?;
                set_mode(&mut metadata.modes.1, parse_mode(value)?)?;
            } else if let Some(value) = line.strip_prefix("deleted file mode ") {
                set_mode(&mut metadata.modes.0, parse_mode(value)?)?;
                set_mode(&mut metadata.modes.1, 0)?;
            }
        }
        Ok(metadata)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PatchContentKind {
    Text,
    Metadata,
    BinaryObjects,
}

#[derive(Debug, Clone)]
pub struct PatchRecord {
    pub entry: FileEntry,
    pub old_mode: u32,
    pub new_mode: u32,
    bytes: Arc<[u8]>,
    metadata: RecordMetadata,
    content_kind: PatchContentKind,
}

fn safe_path(path: &Path) -> Result<()> {
    let text = path
        .to_str()
        .ok_or("Non-UTF-8 remote paths are unsupported")?;
    if text.is_empty()
        || text.starts_with('/')
        || text.contains('\\')
        || text.chars().any(char::is_control)
        || text
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err("Unsafe repository patch path".into());
    }
    Ok(())
}

enum BinarySection {
    Summary,
    NextBlock(usize),
    Data { blocks: usize, has_data: bool },
}

fn validate_binary_line(line: &[u8], section: &mut BinarySection) -> Result<()> {
    match section {
        BinarySection::Summary => {
            if line != b"\n" {
                return Err("Unexpected trailing binary patch data".into());
            }
        }
        BinarySection::NextBlock(blocks) => {
            let header = std::str::from_utf8(line)?.trim_end_matches('\n');
            let length = header
                .strip_prefix("literal ")
                .or_else(|| header.strip_prefix("delta "))
                .ok_or("Invalid binary patch block")?
                .parse::<usize>()?;
            if *blocks >= 2 || length > 32 * 1024 * 1024 {
                return Err("Binary patch block exceeds its loading limit".into());
            }
            *section = BinarySection::Data {
                blocks: *blocks + 1,
                has_data: false,
            };
        }
        BinarySection::Data { blocks, has_data } => {
            if line == b"\n" {
                if !*has_data {
                    return Err("Empty binary patch block".into());
                }
                *section = BinarySection::NextBlock(*blocks);
            } else {
                let length = match line.first() {
                    Some(b'A'..=b'Z') => (line[0] - b'A' + 1) as usize,
                    Some(b'a'..=b'z') => (line[0] - b'a' + 27) as usize,
                    _ => return Err("Invalid binary patch encoding".into()),
                };
                let alphabet=b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz!#$%&()*+-;<=>?@^_`{|}~";
                if line.len() != 2 + length.div_ceil(4) * 5
                    || !line[1..line.len() - 1].iter().all(|b| alphabet.contains(b))
                {
                    return Err("Invalid binary patch payload".into());
                }
                *has_data = true;
            }
        }
    }
    Ok(())
}

fn validate_hunk_framing(bytes: &[u8]) -> Result<()> {
    let mut body = false;
    let mut remaining = None;
    let mut marker_allowed = false;
    let mut last_origin = 0u8;
    let mut previous_end = (0u32, 0u32);
    let mut old_eof = false;
    let mut new_eof = false;
    let mut binary = None;
    for line in bytes.split_inclusive(|b| *b == b'\n') {
        if !line.ends_with(b"\n") {
            return Err("Truncated patch line".into());
        }
        if let Some(section) = binary.as_mut() {
            validate_binary_line(line, section)?;
            continue;
        }
        if !body && line.starts_with(b"Binary files ") && line.ends_with(b" differ\n") {
            binary = Some(BinarySection::Summary);
            continue;
        }
        if !body && line == b"GIT binary patch\n" {
            binary = Some(BinarySection::NextBlock(0));
            continue;
        }
        if line.starts_with(b"@@ -") {
            if remaining.is_some() {
                return Err("Truncated patch hunk".into());
            }
            let header = std::str::from_utf8(line)?;
            let mut parts = header.split_whitespace();
            if parts.next() != Some("@@") {
                return Err("Invalid hunk header".into());
            }
            let parse = |part: &str, prefix: char| -> Result<(u32, usize)> {
                let range = part.strip_prefix(prefix).ok_or("Invalid hunk side")?;
                let (start, count) = range.split_once(',').unwrap_or((range, "1"));
                let start: u32 = start.parse()?;
                let count: u32 = count.parse()?;
                start
                    .checked_add(count)
                    .filter(|n| *n <= i32::MAX as u32)
                    .ok_or("Hunk range overflow")?;
                if count > 0 && start == 0 {
                    return Err("A nonempty hunk range starts at zero".into());
                }
                Ok((start, count as usize))
            };
            let (old_start, old) = parse(parts.next().ok_or("Missing old range")?, '-')?;
            let (new_start, new) = parse(parts.next().ok_or("Missing new range")?, '+')?;
            if old_eof && old > 0 || new_eof && new > 0 {
                return Err("Patch contains lines after a side reached EOF".into());
            }
            if old_start < previous_end.0 || new_start < previous_end.1 {
                return Err("Overlapping or reversed patch hunks".into());
            }
            previous_end = (old_start + old as u32, new_start + new as u32);
            if parts.next() != Some("@@") || old == 0 && new == 0 {
                return Err("Invalid or empty hunk".into());
            }
            remaining = Some((old, new));
            body = true;
            marker_allowed = false;
        } else if line.starts_with(b"\\ ") {
            let (old_left, new_left) = remaining.unwrap_or((0, 0));
            let at_end = match last_origin {
                b'-' => old_left == 0,
                b'+' => new_left == 0,
                b' ' => old_left == 0 && new_left == 0,
                _ => false,
            };
            if line != b"\\ No newline at end of file\n" || !marker_allowed || !at_end {
                return Err("Invalid newline marker".into());
            }
            old_eof |= matches!(last_origin, b'-' | b' ');
            new_eof |= matches!(last_origin, b'+' | b' ');
            marker_allowed = false;
        } else if let Some((old, new)) = remaining.as_mut() {
            match line.first() {
                Some(b' ') => {
                    *old = old.checked_sub(1).ok_or("Old count underflow")?;
                    *new = new.checked_sub(1).ok_or("New count underflow")?;
                }
                Some(b'-') => {
                    *old = old.checked_sub(1).ok_or("Old count underflow")?;
                }
                Some(b'+') => {
                    *new = new.checked_sub(1).ok_or("New count underflow")?;
                }
                _ => return Err("Invalid hunk body".into()),
            }
            marker_allowed = true;
            last_origin = line[0];
            if *old == 0 && *new == 0 {
                remaining = None;
            }
        } else if body {
            return Err("Unexpected trailing patch data".into());
        } else if ![
            b"diff --git ".as_slice(),
            b"index ",
            b"old mode ",
            b"new mode ",
            b"deleted file mode ",
            b"new file mode ",
            b"similarity index ",
            b"dissimilarity index ",
            b"rename from ",
            b"rename to ",
            b"copy from ",
            b"copy to ",
            b"--- ",
            b"+++ ",
        ]
        .iter()
        .any(|prefix| line.starts_with(prefix))
        {
            return Err("Invalid patch record header".into());
        }
    }
    if remaining.is_some() {
        return Err("Truncated patch hunk".into());
    }
    if matches!(
        binary,
        Some(BinarySection::Data { .. }) | Some(BinarySection::NextBlock(0))
    ) {
        return Err("Truncated binary patch block".into());
    }
    Ok(())
}

fn hunk_line_offset(start: u32, lines: u32) -> Result<usize> {
    if lines == 0 {
        Ok(start as usize)
    } else {
        start
            .checked_sub(1)
            .map(|n| n as usize)
            .ok_or("Invalid zero hunk start".into())
    }
}
fn append_reconstructed_line(out: &mut Vec<u8>, line: &[u8], ended: &mut bool) -> Result<()> {
    if *ended && !line.is_empty() {
        return Err("New bytes follow an unterminated line".into());
    }
    if out
        .len()
        .checked_add(line.len())
        .is_none_or(|n| n > 32 * 1024 * 1024)
    {
        return Err("Reconstructed blob exceeds32MiB".into());
    }
    if line.is_empty() {
        return Err("Empty unterminated patch line".into());
    }
    out.extend_from_slice(line);
    *ended = !line.ends_with(b"\n");
    Ok(())
}

impl PatchRecord {
    pub fn reconstruct_text(&self, old: &[u8]) -> Result<Vec<u8>> {
        if old.len() > 32 * 1024 * 1024 {
            return Err("Old blob exceeds32MiB".into());
        }

        if self.content_kind == PatchContentKind::BinaryObjects {
            return Err("Binary payload uses separate proof".into());
        }
        let diff = Self::parse_diff(&self.bytes).map_err(|e| e.to_string())?;
        let Some(patch) = git2::Patch::from_diff(&diff, 0).map_err(|e| e.to_string())? else {
            return Ok(old.to_vec());
        };
        let mut input = old.split_inclusive(|b| *b == b'\n');
        let (mut old_pos, mut new_pos) = (0usize, 0usize);
        let mut out = Vec::new();
        let mut ended = false;
        for h in 0..patch.num_hunks() {
            let (hunk, count) = patch.hunk(h).map_err(|e| e.to_string())?;
            let target = hunk_line_offset(hunk.old_start(), hunk.old_lines())?;
            if target < old_pos {
                return Err("Overlapping old hunks".into());
            }
            while old_pos < target {
                let line = input.next().ok_or("Old hunk starts beyond blob")?;
                append_reconstructed_line(&mut out, line, &mut ended)?;
                old_pos += 1;
                new_pos += 1;
            }
            if hunk_line_offset(hunk.new_start(), hunk.new_lines())? != new_pos {
                return Err("New hunk coordinates do not match transformation".into());
            }
            let old_start = old_pos;
            let new_start = new_pos;
            let mut n = 0;
            while n < count {
                let line = patch.line_in_hunk(h, n).map_err(|e| e.to_string())?;
                let kind = line.origin_value();
                let mut content = line.content();
                let expected_marker = match kind {
                    DiffLineType::Context => DiffLineType::ContextEOFNL,
                    DiffLineType::Addition => DiffLineType::DeleteEOFNL,
                    DiffLineType::Deletion => DiffLineType::AddEOFNL,
                    _ => return Err("Unattached EOF marker".into()),
                };
                if n + 1 < count {
                    let next = patch.line_in_hunk(h, n + 1).map_err(|e| e.to_string())?;
                    if matches!(
                        next.origin_value(),
                        DiffLineType::ContextEOFNL
                            | DiffLineType::AddEOFNL
                            | DiffLineType::DeleteEOFNL
                    ) {
                        if next.origin_value() != expected_marker {
                            return Err("Contradictory EOF marker".into());
                        }
                        content = content
                            .strip_suffix(b"\n")
                            .ok_or("EOF marker without patch newline")?;
                        n += 1;
                    }
                }
                match kind {
                    DiffLineType::Context | DiffLineType::Deletion => {
                        if line.old_lineno() != Some((old_pos + 1) as u32) {
                            return Err("Wrong old coordinate".into());
                        }
                        if input.next() != Some(content) {
                            return Err("Old bytes do not match exact hunk position".into());
                        }
                        old_pos += 1;
                    }
                    _ => {
                        if line.old_lineno().is_some() {
                            return Err("Addition has old coordinate".into());
                        }
                    }
                }
                match kind {
                    DiffLineType::Context | DiffLineType::Addition => {
                        if line.new_lineno() != Some((new_pos + 1) as u32) {
                            return Err("Wrong new coordinate".into());
                        }
                        append_reconstructed_line(&mut out, content, &mut ended)?;
                        new_pos += 1;
                    }
                    _ => {
                        if line.new_lineno().is_some() {
                            return Err("Deletion has new coordinate".into());
                        }
                    }
                }
                n += 1;
            }
            if old_pos - old_start != hunk.old_lines() as usize
                || new_pos - new_start != hunk.new_lines() as usize
            {
                return Err("Hunk size changed".into());
            }
        }
        for line in input {
            append_reconstructed_line(&mut out, line, &mut ended)?;
        }
        Ok(out)
    }

    pub fn parse(bytes: Vec<u8>) -> Result<Self> {
        if !bytes.starts_with(b"diff --git ") {
            return Err("Expected a Git patch record".into());
        }
        if bytes.len() > 32 * 1024 * 1024 {
            return Err("Patch file exceeds 32 MiB".into());
        }
        if bytes.iter().filter(|b| **b == b'\n').take(250001).count() > 250000 {
            return Err("Patch record exceeds the 250000-line resident parsing budget".into());
        }
        validate_hunk_framing(&bytes)?;
        let metadata = RecordMetadata::parse(&bytes)?;
        let diff = Self::parse_diff(&bytes)?;
        if diff.deltas().len() != 1 {
            return Err("Expected exactly one patch file".into());
        }
        let delta = diff.get_delta(0).ok_or("Missing patch file")?;
        let old = delta.old_file();
        let new = delta.new_file();
        let path = new
            .path()
            .or_else(|| old.path())
            .ok_or("Missing patch path")?
            .to_owned();
        safe_path(&path)?;
        if let Some(path) = old.path() {
            safe_path(path)?;
        }
        if matches!(old.mode(), git2::FileMode::Commit)
            || matches!(new.mode(), git2::FileMode::Commit)
        {
            return Err("Remote submodules are unsupported".into());
        }
        if !matches!(
            delta.status(),
            git2::Delta::Added
                | git2::Delta::Deleted
                | git2::Delta::Modified
                | git2::Delta::Renamed
                | git2::Delta::Typechange
        ) {
            return Err("Unsupported patch status".into());
        }
        let patch = git2::Patch::from_diff(&diff, 0)?;
        if matches!(delta.status(), git2::Delta::Added | git2::Delta::Deleted)
            && patch.as_ref().is_some_and(|p| p.num_hunks() == 0)
            && !delta.flags().contains(git2::DiffFlags::BINARY)
        {
            let present = if delta.status() == git2::Delta::Added {
                new.id()
            } else {
                old.id()
            };
            if !present.to_string().starts_with("e69de29") {
                return Err("An empty file record has a nonempty blob identity".into());
            }
        }
        let (_, additions, deletions) = patch
            .as_ref()
            .map(|p| p.line_stats())
            .transpose()?
            .unwrap_or((0, 0, 0));
        let content_kind = if patch.as_ref().is_some_and(|patch| patch.num_hunks() > 0) {
            PatchContentKind::Text
        } else if delta.flags().contains(git2::DiffFlags::BINARY) {
            PatchContentKind::BinaryObjects
        } else {
            PatchContentKind::Metadata
        };
        let binary = old.is_binary()
            || new.is_binary()
            || delta.flags().contains(git2::DiffFlags::BINARY)
            || std::str::from_utf8(&bytes).is_err();
        let status = match delta.status() {
            git2::Delta::Added => FileStatus::Added,
            git2::Delta::Deleted => FileStatus::Deleted,
            git2::Delta::Renamed => FileStatus::Renamed,
            _ => FileStatus::Modified,
        };
        let entry = FileEntry {
            display_name: path.to_string_lossy().into_owned(),
            old_path: old.path().filter(|p| *p != path).map(Path::to_owned),
            path,
            status,
            insertions: additions,
            deletions,
            binary,
            old_source_path: None,
            new_source_path: None,
        };
        Ok(Self {
            entry,
            old_mode: old.mode().into(),
            new_mode: new.mode().into(),
            bytes: bytes.into(),
            metadata,
            content_kind,
        })
    }

    fn parse_diff(bytes: &[u8]) -> Result<git2::Diff<'static>> {
        let mut framed = Vec::with_capacity(bytes.len() + 4);
        framed.extend_from_slice(bytes);
        framed.extend_from_slice(b"-- \n");
        Ok(git2::Diff::from_buffer(&framed)?)
    }

    pub fn validate_sides(&self, old: Option<BlobSide>, new: Option<BlobSide>) -> Result<()> {
        if old.is_none() != (self.entry.status == FileStatus::Added)
            || new.is_none() != (self.entry.status == FileStatus::Deleted)
            || old.is_none() && new.is_none()
        {
            return Err("Patch operation disagrees with immutable object presence".into());
        }
        for (reported, side) in [(self.metadata.modes.0, old), (self.metadata.modes.1, new)] {
            if let Some(mode) = reported {
                if mode != side.map_or(0, |side| side.mode) {
                    return Err("Patch mode disagrees with immutable tree".into());
                }
            }
        }
        if (self.metadata.modes.0.is_none() || self.metadata.modes.1.is_none())
            && old.map(|side| side.mode) != new.map(|side| side.mode)
        {
            return Err("Patch omitted a mode or presence change".into());
        }
        if let Some((old_id, new_id)) = &self.metadata.ids {
            for (prefix, side) in [(old_id, old), (new_id, new)] {
                let valid = side.map_or_else(
                    || prefix.bytes().all(|byte| byte == b'0'),
                    |side| side.id.0.to_string().starts_with(prefix),
                );
                if !valid
                    || self.content_kind == PatchContentKind::BinaryObjects && prefix.len() != 40
                {
                    return Err("Patch identity disagrees with immutable tree or lacks a full binary identity".into());
                }
            }
        } else if self.has_changed_lines() || self.entry.binary {
            return Err("Changed patch contents lack object identity headers".into());
        }
        if !self.entry.binary
            && !self.has_changed_lines()
            && old.map(|side| side.id) != new.map(|side| side.id)
        {
            let empty = BlobId::parse("e69de29bb2d1d6434b8b29ae775ad8c2e48c5391")?;
            if old.map_or(new.map(|side| side.id) != Some(empty), |side| {
                new.is_some() || side.id != empty
            }) {
                return Err("Metadata-only patch changes object contents".into());
            }
        }
        Ok(())
    }

    pub fn content_kind(&self) -> PatchContentKind {
        self.content_kind
    }

    pub fn display_diff(&self) -> Result<DiffResult> {
        let diff = Self::parse_diff(&self.bytes)?;
        let mut result = DiffResult {
            changes: Vec::new(),
            significant_changes: Vec::new(),
            hunks: Vec::new(),
            insertions: 0,
            deletions: 0,
        };
        if self.entry.binary {
            result.changes.push(Change::single(
                0,
                ChangeSpan::equal("Binary file. Full text is unavailable."),
            ));
            return Ok(result);
        }
        if let Some(patch) = git2::Patch::from_diff(&diff, 0)? {
            for hi in 0..patch.num_hunks() {
                let (hunk, count) = patch.hunk(hi)?;
                let mut group = Hunk {
                    id: hi,
                    change_ids: Vec::new(),
                    old_start: Some(hunk.old_start() as usize),
                    new_start: Some(hunk.new_start() as usize),
                    insertions: 0,
                    deletions: 0,
                };
                for li in 0..count {
                    let line = patch.line_in_hunk(hi, li)?;
                    let text = terminal_text(
                        std::str::from_utf8(line.content())?
                            .lines()
                            .next()
                            .unwrap_or_default(),
                    );
                    let span = match line.origin() {
                        '+' => {
                            group.insertions += 1;
                            ChangeSpan::insert(text)
                        }
                        '-' => {
                            group.deletions += 1;
                            ChangeSpan::delete(text)
                        }
                        ' ' => ChangeSpan::equal(text),
                        _ => continue,
                    }
                    .with_lines(
                        line.old_lineno().map(|n| n as usize),
                        line.new_lineno().map(|n| n as usize),
                    );
                    let id = result.changes.len();
                    if span.is_change() {
                        result.significant_changes.push(id);
                        group.change_ids.push(id);
                    }
                    result.changes.push(Change::single(id, span));
                }
                result.insertions += group.insertions;
                result.deletions += group.deletions;
                result.hunks.push(group);
            }
        }
        if result.changes.is_empty() {
            let text = if self.entry.binary {
                "Binary patch. Full text is unavailable.".to_owned()
            } else if self.entry.status == FileStatus::Renamed {
                format!(
                    "Renamed from {}.",
                    terminal_text(
                        &self
                            .entry
                            .old_path
                            .as_ref()
                            .map(|path| path.to_string_lossy())
                            .unwrap_or_default()
                    )
                )
            } else if self.old_mode != self.new_mode {
                match self.entry.status {
                    FileStatus::Added => "Empty file added.".to_owned(),
                    FileStatus::Deleted => "Empty file deleted.".to_owned(),
                    _ => format!("Mode changed {:o} -> {:o}.", self.old_mode, self.new_mode),
                }
            } else {
                "Metadata change.".to_owned()
            };
            result
                .changes
                .push(Change::single(0, ChangeSpan::equal(text)));
        }
        Ok(result)
    }

    pub fn has_changed_lines(&self) -> bool {
        !self.entry.binary && (self.entry.insertions > 0 || self.entry.deletions > 0)
    }

    pub fn byte_len(&self) -> usize {
        self.bytes.len()
    }
}

pub fn read_patch(
    input: &mut impl BufRead,
    mut accept: impl FnMut(PatchRecord) -> Result<()>,
) -> Result<()> {
    let mut file = Vec::new();
    let mut line = Vec::new();
    let mut total = 0usize;
    let mut paths = HashSet::new();
    loop {
        line.clear();
        loop {
            let available = input.fill_buf()?;
            if available.is_empty() {
                break;
            }
            let end = available
                .iter()
                .position(|b| *b == b'\n')
                .map(|n| n + 1)
                .unwrap_or(available.len());
            total = total.checked_add(end).ok_or("Patch length overflow")?;
            if total > 48 * 1024 * 1024 || line.len() + end > 32 * 1024 * 1024 {
                return Err("Patch exceeds loading limit".into());
            }
            line.extend_from_slice(&available[..end]);
            input.consume(end);
            if line.ends_with(b"\n") {
                break;
            }
        }
        if line.is_empty() {
            break;
        }
        if line.starts_with(b"diff --git ") && !file.is_empty() {
            let record = PatchRecord::parse(std::mem::take(&mut file))?;
            if !paths.insert(record.entry.path.clone()) {
                return Err("Patch repeats a file path".into());
            }
            accept(record)?;
        }
        if file.len() + line.len() > 32 * 1024 * 1024 {
            return Err("Patch file exceeds loading limit".into());
        }
        file.extend_from_slice(&line);
    }
    if !file.is_empty() {
        let record = PatchRecord::parse(file)?;
        if !paths.insert(record.entry.path.clone()) {
            return Err("Patch repeats a file path".into());
        }
        accept(record)?;
    }
    Ok(())
}

pub fn terminal_text(value: &str) -> String {
    value
        .chars()
        .flat_map(|c| {
            if c.is_control() && c != '\t' {
                c.escape_debug().collect::<Vec<_>>()
            } else {
                vec![c]
            }
        })
        .collect()
}

pub fn terminal_document_text(value: &str) -> String {
    value
        .split_inclusive('\n')
        .map(|line| match line.strip_suffix('\n') {
            Some(line) => terminal_text(line.strip_suffix('\r').unwrap_or(line)) + "\n",
            None => terminal_text(line),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::multi::{ContentSource, DiffStatus, MultiFileDiff};
    use std::io::{BufReader, Cursor};

    #[test]
    fn exact_text_proof_checks_bytes_coordinates_newlines_and_complete_objects() {
        #[derive(serde::Deserialize)]
        struct Case {
            name: String,
            old: Vec<u8>,
            new: Vec<u8>,
            old_id: String,
            new_id: String,
            patch: Vec<u8>,
            verified: bool,
        }
        let cases: Vec<Case> =
            serde_json::from_str(include_str!("../tests/fixtures/exact_patch.json")).unwrap();
        for case in cases {
            let result = PatchRecord::parse(case.patch).and_then(|record| {
                BlobId::parse(&case.old_id)?.verify(&case.old)?;
                let reconstructed = record.reconstruct_text(&case.old)?;
                BlobId::parse(&case.new_id)?.verify(&reconstructed)?;
                assert_eq!(reconstructed, case.new, "{}", case.name);
                Ok(())
            });
            assert_eq!(result.is_ok(), case.verified, "{}: {result:?}", case.name);
        }
        assert!(BlobId::parse("1234567").is_err());
        assert!(BlobId::parse(&"a".repeat(64)).is_err());
    }

    #[test]
    fn immutable_side_validation_preserves_metadata_and_rejects_forged_headers() {
        let id = BlobId::parse("e69de29bb2d1d6434b8b29ae775ad8c2e48c5391").unwrap();
        let plain = BlobSide { id, mode: 0o100644 };
        let executable = BlobSide { id, mode: 0o100755 };
        let rename = PatchRecord::parse(
            b"diff --git a/a b/b\nsimilarity index 100%\nrename from a\nrename to b\n".to_vec(),
        )
        .unwrap();
        assert!(rename.validate_sides(Some(plain), Some(plain)).is_ok());
        assert!(rename
            .validate_sides(Some(plain), Some(executable))
            .is_err());
        let mode =
            PatchRecord::parse(b"diff --git a/f b/f\nold mode 100644\nnew mode 100755\n".to_vec())
                .unwrap();
        assert!(mode.validate_sides(Some(plain), Some(executable)).is_ok());
        assert!(mode.validate_sides(Some(plain), Some(plain)).is_err());
        let empty = PatchRecord::parse(
            b"diff --git a/f b/f\nnew file mode 100644\nindex 0000000..e69de29\n".to_vec(),
        )
        .unwrap();
        assert!(empty.validate_sides(None, Some(plain)).is_ok());
        assert!(empty.validate_sides(Some(plain), Some(plain)).is_err());
        let binary = PatchRecord::parse(
            format!(
                "diff --git a/f b/f\nindex {id}..{id} 100644\nBinary files a/f and b/f differ\n"
            )
            .into_bytes(),
        )
        .unwrap();
        assert!(binary.validate_sides(Some(plain), Some(plain)).is_ok());
        let abbreviated = PatchRecord::parse(
            b"diff --git a/f b/f\nindex e69de29..e69de29 100644\nBinary files a/f and b/f differ\n"
                .to_vec(),
        )
        .unwrap();
        assert!(abbreviated
            .validate_sides(Some(plain), Some(plain))
            .is_err());
        assert!(BlobId::parse(&"0".repeat(40)).is_err());
        assert!(PatchRecord::parse(b"diff --git a/f b/f\nindex 1111111..2222222 100644\nindex 1111111..2222222 100644\n--- a/f\n+++ b/f\n@@ -1 +1 @@\n-old\n+new\n".to_vec()).is_err());
    }

    #[test]
    fn unrenderable_text_uses_exact_byte_proof_and_retains_binary_display() {
        let old = b"old\xff\n";
        let new = b"new\xfe\n";
        let old_id = git2::Oid::hash_object(git2::ObjectType::Blob, old)
            .unwrap()
            .to_string();
        let new_id = git2::Oid::hash_object(git2::ObjectType::Blob, new)
            .unwrap()
            .to_string();
        let mut raw = format!(
            "diff --git a/f b/f\nindex {}..{} 100644\n--- a/f\n+++ b/f\n@@ -1 +1 @@\n",
            &old_id[..7],
            &new_id[..7]
        )
        .into_bytes();
        raw.extend_from_slice(b"-old\xff\n+new\xfe\n");
        let record = PatchRecord::parse(raw).unwrap();
        assert_eq!(record.content_kind(), PatchContentKind::Text);
        assert!(record.entry.binary);
        assert!(!record.has_changed_lines());
        let side = |id: &str| {
            Some(BlobSide {
                id: BlobId::parse(id).unwrap(),
                mode: 0o100644,
            })
        };
        record.validate_sides(side(&old_id), side(&new_id)).unwrap();
        assert_eq!(record.reconstruct_text(old).unwrap(), new);
        assert_eq!(
            record.display_diff().unwrap().changes[0].spans[0].text,
            "Binary file. Full text is unavailable."
        );
    }

    fn text_patch() -> Vec<u8> {
        b"diff --git a/f b/f\nindex 1111111..2222222 100644\n--- a/f\n+++ b/f\n@@ -40 +40 @@\n-old\n+new\n@@ -80 +80 @@\n-before\n+after\n".to_vec()
    }

    #[test]
    fn newline_markers_end_the_marked_side_across_all_hunks() {
        for first in [
            "-old\n+new\n\\ No newline at end of file\n",
            "-old\n\\ No newline at end of file\n+new\n",
            " same\n\\ No newline at end of file\n",
        ] {
            let patch = format!("diff --git a/f b/f\nindex 1111111..2222222 100644\n--- a/f\n+++ b/f\n@@ -1 +1 @@\n{first}@@ -3 +3 @@\n-later\n+replacement\n");
            assert!(PatchRecord::parse(patch.into_bytes()).is_err());
        }
    }

    #[test]
    fn sparse_navigation_keeps_original_coordinates_without_full_text() {
        let patch = PatchRecord::parse(text_patch()).unwrap();
        let mut multi = MultiFileDiff::from_pending_files(None, Vec::new(), true);
        let index = multi.append_patch(patch, ContentSource::Empty, ContentSource::Empty);
        assert!(multi.file_contents(index).is_none());
        assert!(multi.file_contents_arc(index).is_none());
        multi.ensure_full_navigator(index);
        let nav = multi.current_navigator();
        assert!(nav.old_content().is_none());
        assert!(nav.new_content().is_none());
        assert_eq!(nav.diff().changes[0].spans[0].old_line, Some(40));
        assert_eq!(nav.diff().changes[2].spans[0].old_line, Some(80));
        assert_eq!(nav.diff().changes.len(), 4);
        assert!(multi.request_full_content(index));
        assert!(!multi.request_full_content(index));
        multi.take_pending_content();
        assert!(multi
            .apply_prepared_content(index, MultiFileDiff::failed_file_content("network failed")));
        assert!(multi.file_is_sparse(index));
        assert_eq!(multi.diff_status(index), DiffStatus::Ready);
        assert_eq!(
            multi.current_navigator().diff().changes[0].spans[0].old_line,
            Some(40)
        );
        assert!(!multi.ensure_full_content(index));
        assert!(multi.request_full_content(index));
        multi.take_pending_content();
        assert!(multi.apply_prepared_content(
            index,
            MultiFileDiff::prepare_remote_file_content(b"old\n".to_vec(), b"new\n".to_vec())
        ));
        assert_eq!(multi.file_contents(index), Some(("old\n", "new\n")));
        assert_eq!(multi.current_navigator().old_content(), Some("old\n"));
        assert_eq!(multi.file_modes(index), Some((0o100644, 0o100644)));
    }

    #[test]
    fn metadata_and_binary_records_remain_files() {
        let records = [
            b"diff --git a/e b/e\nnew file mode 100644\nindex 0000000..e69de29\n".as_slice(),
            b"diff --git a/e b/e\ndeleted file mode 100644\nindex e69de29..0000000\n".as_slice(),
            b"diff --git a/old b/new\nsimilarity index 100%\nrename from old\nrename to new\n"
                .as_slice(),
            b"diff --git a/m b/m\nold mode 100644\nnew mode 100755\n".as_slice(),
            b"diff --git a/b b/b\nindex 1111111..2222222 100644\nBinary files a/b and b/b differ\n"
                .as_slice(),
        ];
        for bytes in records {
            let record = PatchRecord::parse(bytes.to_vec()).unwrap();
            assert!(!record.display_diff().unwrap().changes.is_empty());
        }
        assert!(
            PatchRecord::parse(records[4].to_vec())
                .unwrap()
                .entry
                .binary
        );
        let renamed = PatchRecord::parse(records[2].to_vec()).unwrap();
        assert_eq!(renamed.entry.old_path.as_deref(), Some(Path::new("old")));
        assert_eq!(renamed.entry.path, Path::new("new"));
    }

    #[test]
    fn byte_chunk_boundaries_preserve_quoted_unicode_and_symlink_records() {
        let bytes = b"diff --git \"a/space \\303\\251\" \"b/space \\303\\251\"\nnew file mode 120000\nindex 0000000..2222222\n--- /dev/null\n+++ \"b/space \\303\\251\"\n@@ -0,0 +1 @@\n+target\n\\ No newline at end of file\n";
        for size in 1..=bytes.len() {
            let mut reader = BufReader::with_capacity(size, Cursor::new(bytes));
            let mut found = Vec::new();
            read_patch(&mut reader, |record| {
                found.push(record);
                Ok(())
            })
            .unwrap();
            assert_eq!(found.len(), 1);
            assert_eq!(found[0].entry.path.to_str(), Some("space \u{e9}"));
            assert_eq!(found[0].new_mode, 0o120000);
            assert_eq!(
                found[0].display_diff().unwrap().changes[0].spans[0].new_line,
                Some(1)
            );
        }
    }

    #[test]
    fn truncation_junk_duplicates_unsafe_paths_and_combined_diffs_fail() {
        let patch = text_patch();
        for bytes in [
            patch[..patch.len() - 3].to_vec(),
            [patch.as_slice(), b"junk\n"].concat(),
            b"diff --cc f\nindex 1111111,2222222..3333333\n".to_vec(),
        ] {
            assert!(PatchRecord::parse(bytes).is_err());
        }
        let doubled = [patch.as_slice(), patch.as_slice()].concat();
        assert!(read_patch(&mut Cursor::new(doubled), |_| Ok(())).is_err());
        let unsafe_path = String::from_utf8(patch.clone())
            .unwrap()
            .replace("a/f", "a/../f")
            .replace("b/f", "b/../f");
        assert!(PatchRecord::parse(unsafe_path.into_bytes()).is_err());
    }

    #[test]
    fn binary_tails_and_invalid_newline_markers_are_not_complete_records() {
        let binary=b"diff --git a/b b/b\nindex 1111111..2222222 100644\nBinary files a/b and b/b differ\ntrailing junk\n";
        assert!(PatchRecord::parse(binary.to_vec()).is_err());
        assert!(PatchRecord::parse([text_patch(), b"\\ garbage\n".to_vec()].concat()).is_err());
        let valid=b"diff --git a/f b/f\nindex 1111111..2222222 100644\n--- a/f\n+++ b/f\n@@ -1 +1 @@\n-old\n\\ No newline at end of file\n+new\n\\ No newline at end of file\n";
        assert!(PatchRecord::parse(valid.to_vec()).is_ok());
        assert!(PatchRecord::parse(
            b"diff --git a/f b/f\nnew file mode 100644\nindex 0000000..2222222\n".to_vec()
        )
        .is_err());
    }

    #[test]
    fn real_git_binary_payload_and_invalid_text_have_explicit_semantics() {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let directory = std::env::temp_dir().join(format!(
            "oyo-binary-patch-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let old = directory.join("old");
        let new = directory.join("new");
        std::fs::write(&old, b"old\0binary").unwrap();
        std::fs::write(&new, b"new\0binary").unwrap();
        let output = std::process::Command::new("git")
            .args(["diff", "--no-index", "--binary"])
            .arg(&old)
            .arg(&new)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1));
        assert!(output
            .stdout
            .windows(b"GIT binary patch".len())
            .any(|w| w == b"GIT binary patch"));
        let record = PatchRecord::parse(output.stdout.clone()).unwrap();
        assert!(record.entry.binary);
        assert!(PatchRecord::parse([output.stdout, b"trailing junk\n".to_vec()].concat()).is_err());
        std::fs::remove_dir_all(directory).unwrap();
        let invalid=b"diff --git a/f b/f\nindex 1111111..2222222 100644\n--- a/f\n+++ b/f\n@@ -1 +1 @@\n-old\n+\xffnew\n";
        let record = PatchRecord::parse(invalid.to_vec()).unwrap();
        assert!(record.entry.binary);
        assert!(!record.has_changed_lines());
        assert!(record.display_diff().is_ok());
    }

    #[test]
    fn terminal_controls_are_display_text_not_escape_sequences() {
        let bytes = String::from_utf8(text_patch())
            .unwrap()
            .replace("+new", "+\u{1b}]52;c;payload\u{7}new")
            .into_bytes();
        let record = PatchRecord::parse(bytes).unwrap();
        let result = record.display_diff().unwrap();
        let text = &result.changes[1].spans[0].text;
        assert!(!text.chars().any(|c| c == '\u{1b}' || c == '\u{7}'));
        assert!(text.contains("\\u{1b}]52"));
    }
}
