// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! Canonical anchor resolution (S02 §4).
//!
//! Parsers emit either byte-precise anchors or line-only anchors. Both source
//! reading and ContextPack snippets must resolve those anchors with exactly the
//! same rules; duplicating the conversion previously made a line-only anchor
//! silently start at byte zero.

use crate::error::{CoreError, Result};
use crate::source::LineIndex;
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnchorRange {
    pub start: u64,
    pub end: u64,
}

/// Resolve an anchor to a half-open UTF-8 byte range in `bytes`.
///
/// Line anchors are one-based and inclusive. A zero line is accepted as the
/// first line for adapters that emit zero-based rows. A null `line_end` means
/// the anchor covers only `line_start`; a null start means the first line.
pub fn byte_range(anchor: &Value, bytes: &[u8]) -> Result<AnchorRange> {
    let length = bytes.len() as u64;
    let range = match anchor["precision"].as_str().unwrap_or("") {
        "bytes" => {
            let start = anchor["byte_start"]
                .as_u64()
                .ok_or_else(|| CoreError::Storage("byte anchor has no byte_start".to_string()))?;
            let end = anchor["byte_end"]
                .as_u64()
                .ok_or_else(|| CoreError::Storage("byte anchor has no byte_end".to_string()))?;
            AnchorRange { start, end }
        }
        "line_only" => {
            let index = LineIndex::new(bytes);
            let first = zero_based_line(anchor["line_start"].as_u64().unwrap_or(0));
            let last = anchor["line_end"]
                .as_u64()
                .map(zero_based_line)
                .unwrap_or(first);
            if first > last {
                return Err(CoreError::invalid(
                    "line anchor start is after its end line",
                ));
            }
            let start = index
                .line_start(first)
                .ok_or_else(|| CoreError::invalid("line anchor start is out of range"))?;
            let end = index.line_start(last + 1).unwrap_or(index.len());
            AnchorRange { start, end }
        }
        other => {
            return Err(CoreError::unsupported(format!(
                "anchor precision {other:?} cannot be read as source"
            )))
        }
    };
    if range.start > range.end || range.end > length {
        return Err(CoreError::invalid("anchor range is outside the blob"));
    }
    Ok(range)
}

fn zero_based_line(line: u64) -> u64 {
    if line == 0 {
        0
    } else {
        line - 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn line_anchors_resolve_to_the_target_line_not_the_file_start() {
        let text = "first\nsecond\nthird\n";
        let range = byte_range(
            &json!({"precision":"line_only","line_start":2,"line_end":3}),
            text.as_bytes(),
        )
        .unwrap();
        assert_eq!(&text[range.start as usize..range.end as usize], "second\nthird\n");
    }

    #[test]
    fn crlf_and_utf8_line_anchors_stay_on_character_boundaries() {
        let text = "头\n第二行\r\n目标行标记\n";
        let range = byte_range(
            &json!({"precision":"line_only","line_start":3,"line_end":3}),
            text.as_bytes(),
        )
        .unwrap();
        assert_eq!(&text[range.start as usize..range.end as usize], "目标行标记\n");
    }

    #[test]
    fn byte_anchors_reject_ranges_outside_the_blob() {
        let error = byte_range(
            &json!({"precision":"bytes","byte_start":0,"byte_end":99}),
            b"short",
        )
        .unwrap_err();
        assert!(error.to_string().contains("outside the blob"));
    }
}
