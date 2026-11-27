// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! Source coordinate conversions (S02 §4).
//!
//! Canonical ranges are zero-based UTF-8 **byte** offsets, half-open. Editor
//! protocols disagree (LSP negotiates UTF-8/UTF-16/UTF-32; SCIP carries its own
//! encoding metadata), so conversion lives here and never assumes UTF-16.

/// Line start byte offsets. A trailing newline produces a final empty line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineIndex {
    starts: Vec<u64>,
    len: u64,
}

impl LineIndex {
    pub fn new(bytes: &[u8]) -> Self {
        let mut starts = Vec::with_capacity(bytes.len() / 32 + 1);
        starts.push(0u64);
        for (index, byte) in bytes.iter().enumerate() {
            if *byte == b'\n' {
                starts.push(index as u64 + 1);
            }
        }
        Self {
            starts,
            len: bytes.len() as u64,
        }
    }

    pub fn len(&self) -> u64 {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn line_count(&self) -> u64 {
        self.starts.len() as u64
    }

    pub fn line_start(&self, line: u64) -> Option<u64> {
        self.starts.get(line as usize).copied()
    }

    /// Byte offset of a zero-based line and a byte column inside that line.
    /// The newline byte itself is addressable; positions past the line end are
    /// rejected.
    pub fn byte_offset(&self, line: u64, byte_column: u64) -> Option<u64> {
        let start = self.line_start(line)?;
        let end = self
            .starts
            .get(line as usize + 1)
            .copied()
            .unwrap_or(self.len);
        let content_end = if end > start && end <= self.len {
            // Exclude the terminating '\n' from the addressable content.
            end - 1
        } else {
            end
        };
        let offset = start.checked_add(byte_column)?;
        if offset > content_end {
            return None;
        }
        Some(offset)
    }

    /// Zero-based line and byte column for a byte offset.
    pub fn line_col(&self, byte_offset: u64) -> Option<(u64, u64)> {
        if byte_offset > self.len {
            return None;
        }
        let line = match self.starts.binary_search(&byte_offset) {
            Ok(index) => index,
            Err(index) => index - 1,
        } as u64;
        let start = self.starts[line as usize];
        Some((line, byte_offset - start))
    }

    /// Convert a zero-based UTF-16 code-unit column to a UTF-8 byte offset.
    /// Returns `None` when the column splits a surrogate pair or is past the
    /// line.
    pub fn utf16_col_to_byte(
        &self,
        bytes: &[u8],
        line: u64,
        utf16_column: u64,
    ) -> Option<u64> {
        let start = self.line_start(line)?;
        let content = self.line_content(bytes, line)?;
        let mut units = 0u64;
        for (index, ch) in content.char_indices() {
            if units == utf16_column {
                return Some(start + index as u64);
            }
            let width = ch.len_utf16() as u64;
            if utf16_column < units + width {
                return None;
            }
            units += width;
        }
        if units == utf16_column {
            Some(start + content.len() as u64)
        } else {
            None
        }
    }

    /// Convert a zero-based UTF-32 (code point) column to a UTF-8 byte offset.
    pub fn utf32_col_to_byte(
        &self,
        bytes: &[u8],
        line: u64,
        codepoint_column: u64,
    ) -> Option<u64> {
        let start = self.line_start(line)?;
        let content = self.line_content(bytes, line)?;
        let mut points = 0u64;
        for (index, _) in content.char_indices() {
            if points == codepoint_column {
                return Some(start + index as u64);
            }
            points += 1;
        }
        if points == codepoint_column {
            Some(start + content.len() as u64)
        } else {
            None
        }
    }

    /// Convert a byte offset to a zero-based UTF-16 code-unit column.
    pub fn byte_to_utf16_col(&self, bytes: &[u8], byte_offset: u64) -> Option<u64> {
        let (line, _) = self.line_col(byte_offset)?;
        let start = self.line_start(line)?;
        let content = self.line_content(bytes, line)?;
        let relative = (byte_offset - start) as usize;
        if relative > content.len() {
            return None;
        }
        if !content.is_char_boundary(relative) {
            return None;
        }
        let units: usize = content[..relative]
            .chars()
            .map(|ch| ch.len_utf16())
            .sum();
        Some(units as u64)
    }

    fn line_content<'a>(&self, bytes: &'a [u8], line: u64) -> Option<&'a str> {
        let start = self.line_start(line)? as usize;
        let end = self
            .starts
            .get(line as usize + 1)
            .copied()
            .map(|value| value as usize)
            .unwrap_or(bytes.len());
        let content_end = if end > start && end <= bytes.len() && bytes[end - 1] == b'\n' {
            end - 1
        } else {
            end
        };
        std::str::from_utf8(&bytes[start..content_end]).ok()
    }
}

/// `offset` is a UTF-8 character boundary (0 and `len` are boundaries).
pub fn is_char_boundary(bytes: &[u8], offset: u64) -> bool {
    let len = bytes.len() as u64;
    if offset > len {
        return false;
    }
    if offset == 0 || offset == len {
        return true;
    }
    (bytes[offset as usize] & 0b1100_0000) != 0b1000_0000
}

/// Validate a half-open byte range against raw bytes.
pub fn validate_range(bytes: &[u8], start: u64, end: u64) -> bool {
    start <= end
        && end <= bytes.len() as u64
        && is_char_boundary(bytes, start)
        && is_char_boundary(bytes, end)
}

pub fn excerpt(bytes: &[u8], start: u64, end: u64) -> Option<&[u8]> {
    if !validate_range(bytes, start, end) {
        return None;
    }
    Some(&bytes[start as usize..end as usize])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_starts_and_crlf() {
        let bytes = b"a\r\nb\r\n";
        let index = LineIndex::new(bytes);
        assert_eq!(index.line_count(), 3);
        assert_eq!(index.line_start(0), Some(0));
        assert_eq!(index.line_start(1), Some(3));
        assert_eq!(index.line_start(2), Some(6));
        assert_eq!(index.byte_offset(1, 1), Some(4));
        assert_eq!(index.line_col(4), Some((1, 1)));
    }

    #[test]
    fn utf16_columns_account_for_surrogate_pairs() {
        let text = "a中😀b";
        let bytes = text.as_bytes();
        let index = LineIndex::new(bytes);
        // UTF-16: a=1, 中=1, 😀=2, b=1
        assert_eq!(index.utf16_col_to_byte(bytes, 0, 0), Some(0));
        assert_eq!(index.utf16_col_to_byte(bytes, 0, 1), Some(1));
        assert_eq!(index.utf16_col_to_byte(bytes, 0, 2), Some(4));
        assert_eq!(index.utf16_col_to_byte(bytes, 0, 3), None, "inside surrogate pair");
        assert_eq!(index.utf16_col_to_byte(bytes, 0, 4), Some(8));
        assert_eq!(index.byte_to_utf16_col(bytes, 4), Some(2));
        assert_eq!(index.byte_to_utf16_col(bytes, 8), Some(4));
        assert_eq!(index.byte_to_utf16_col(bytes, 2), None, "inside 中");
    }

    #[test]
    fn ranges_must_be_utf8_boundaries() {
        let bytes = "中文".as_bytes();
        assert!(validate_range(bytes, 0, 3));
        assert!(!validate_range(bytes, 1, 3));
        assert!(excerpt(bytes, 0, 3).is_some());
        assert!(excerpt(bytes, 0, 2).is_none());
    }
}
