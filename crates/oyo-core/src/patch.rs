use std::collections::HashSet;
use std::io::BufRead;
use std::path::Path;
use std::sync::Arc;

use crate::git::FileStatus;
use crate::multi::FileEntry;
use crate::{Change, ChangeSpan, DiffResult, Hunk};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Debug, Clone)]
pub struct PatchRecord {
    pub entry: FileEntry,
    pub old_mode: u32,
    pub new_mode: u32,
    bytes: Arc<[u8]>,
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

impl PatchRecord {
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
        })
    }

    fn parse_diff(bytes: &[u8]) -> Result<git2::Diff<'static>> {
        let mut framed = Vec::with_capacity(bytes.len() + 4);
        framed.extend_from_slice(bytes);
        framed.extend_from_slice(b"-- \n");
        Ok(git2::Diff::from_buffer(&framed)?)
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
            } else {
                format!(
                    "Metadata change. Modes {:o} -> {:o}.",
                    self.old_mode, self.new_mode
                )
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
