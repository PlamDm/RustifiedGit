use anyhow::{Context, Result, bail};
use std::ops::Range;

#[derive(Clone, Debug)]
pub struct Hunk {
    pub lines: Range<usize>,
    pub header: Vec<u8>,
}

#[derive(Clone, Debug, Default)]
pub struct Patch {
    pub lines: Vec<Vec<u8>>,
    pub hunks: Vec<Hunk>,
}

impl Patch {
    pub fn parse(bytes: &[u8]) -> Self {
        let lines: Vec<_> = bytes
            .split_inclusive(|byte| *byte == b'\n')
            .map(Vec::from)
            .collect();
        let mut hunks: Vec<Hunk> = Vec::new();
        let mut file_start = 0;
        let mut header_end = 0;
        for (index, line) in lines.iter().enumerate() {
            if line.starts_with(b"diff --git ") {
                if let Some(hunk) = hunks.last_mut() {
                    hunk.lines.end = index;
                }
                file_start = index;
                header_end = index;
            }
            if line.starts_with(b"@@ ") {
                if let Some(hunk) = hunks.last_mut() {
                    hunk.lines.end = hunk.lines.end.min(index);
                }
                if header_end == file_start {
                    header_end = index;
                }
                let header = lines[file_start..header_end].concat();
                hunks.push(Hunk {
                    lines: index..lines.len(),
                    header,
                });
            }
        }
        Self { lines, hunks }
    }

    pub fn hunk_at(&self, line: usize) -> Option<usize> {
        self.hunks
            .iter()
            .position(|hunk| hunk.lines.contains(&line))
    }

    pub fn hunk_patch(&self, index: usize) -> Result<Vec<u8>> {
        let hunk = self.hunks.get(index).context("Select a text hunk first")?;
        let mut bytes = hunk.header.clone();
        bytes.extend(self.lines[hunk.lines.clone()].concat());
        Ok(bytes)
    }

    pub fn line_patch(&self, index: usize, reverse: bool) -> Result<Vec<u8>> {
        let hunk = self
            .hunks
            .get(
                self.hunk_at(index)
                    .context("Select an added or removed line")?,
            )
            .unwrap();
        let line = &self.lines[index];
        if !matches!(line.first(), Some(b'+' | b'-')) {
            bail!("Select an added or removed line");
        }
        let header = String::from_utf8_lossy(&self.lines[hunk.lines.start]);
        let old_start = header
            .split_whitespace()
            .nth(if reverse { 2 } else { 1 })
            .context("Invalid hunk header")?
            .trim_start_matches(['-', '+'])
            .split(',')
            .next()
            .context("Invalid hunk range")?
            .parse::<usize>()?;
        let mut body = Vec::new();
        let mut old_count = 0;
        let mut new_count = 0;
        let mut retained_previous = false;
        for (current, line) in self
            .lines
            .iter()
            .enumerate()
            .take(hunk.lines.end)
            .skip(hunk.lines.start + 1)
        {
            let marker = line.first().copied().unwrap_or_default();
            let marker = if reverse {
                match marker {
                    b'+' => b'-',
                    b'-' => b'+',
                    marker => marker,
                }
            } else {
                marker
            };
            let selected = current == index;
            match marker {
                b' ' => {
                    body.extend_from_slice(line);
                    old_count += 1;
                    new_count += 1;
                    retained_previous = true;
                }
                b'+' if selected => {
                    body.push(b'+');
                    body.extend_from_slice(&line[1..]);
                    new_count += 1;
                    retained_previous = true;
                }
                b'-' if selected => {
                    body.push(b'-');
                    body.extend_from_slice(&line[1..]);
                    old_count += 1;
                    retained_previous = true;
                }
                b'-' => {
                    body.push(b' ');
                    body.extend_from_slice(&line[1..]);
                    old_count += 1;
                    new_count += 1;
                    retained_previous = true;
                }
                b'+' => {
                    retained_previous = false;
                }
                b'\\' if retained_previous => {
                    body.extend_from_slice(line);
                }
                _ => {}
            }
        }
        let mut bytes = partial_file_header(&hunk.header, old_count > 0 && new_count > 0);
        let old_position = if old_count == 0 {
            old_start
        } else {
            old_start.max(1)
        };
        let new_position = if old_count == 0 {
            old_start + 1
        } else if new_count == 0 {
            old_start.saturating_sub(1)
        } else {
            old_start.max(1)
        };
        if reverse {
            // File headers describe paths, so reversing a line uses Git's reverse application.
            // Invert the generated body back while keeping the index-side range calculation.
            let mut restored = Vec::with_capacity(body.len());
            for line in body.split_inclusive(|byte| *byte == b'\n') {
                restored.push(match line[0] {
                    b'+' => b'-',
                    b'-' => b'+',
                    marker => marker,
                });
                restored.extend_from_slice(&line[1..]);
            }
            bytes.extend(
                format!("@@ -{new_position},{new_count} +{old_position},{old_count} @@\n").bytes(),
            );
            bytes.extend(restored);
        } else {
            bytes.extend(
                format!("@@ -{old_position},{old_count} +{new_position},{new_count} @@\n").bytes(),
            );
            bytes.extend(body);
        }
        Ok(bytes)
    }
}

fn partial_file_header(header: &[u8], has_both_sides: bool) -> Vec<u8> {
    if !has_both_sides {
        return header.to_vec();
    }
    let lines: Vec<_> = header.split_inclusive(|byte| *byte == b'\n').collect();
    let creation = lines.iter().any(|line| line.starts_with(b"new file mode "));
    let deletion = lines
        .iter()
        .any(|line| line.starts_with(b"deleted file mode "));
    if !creation && !deletion {
        return header.to_vec();
    }
    let counterpart = |prefix: &[u8], replacement| {
        let mut name = lines
            .iter()
            .find_map(|line| line.strip_prefix(prefix))
            .unwrap_or_default()
            .to_vec();
        let index = usize::from(name.first() == Some(&b'"'));
        if name.get(index + 1) == Some(&b'/') {
            name[index] = replacement;
        }
        name
    };
    let mut result = Vec::new();
    for line in &lines {
        if line.starts_with(b"new file mode ")
            || line.starts_with(b"deleted file mode ")
            || line.starts_with(b"index ")
        {
            continue;
        }
        if creation && line.starts_with(b"--- /dev/null") {
            result.extend_from_slice(b"--- ");
            result.extend(counterpart(b"+++ ", b'a'));
        } else if deletion && line.starts_with(b"+++ /dev/null") {
            result.extend_from_slice(b"+++ ");
            result.extend(counterpart(b"--- ", b'b'));
        } else {
            result.extend_from_slice(line);
        }
    }
    result
}
