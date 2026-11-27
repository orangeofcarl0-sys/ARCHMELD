// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! AM-C14N-1 canonical JSON and content digests.
//!
//! This is a deliberately narrow, self-described canonical form (S19 §1); it
//! does not claim RFC 8785 compliance. Rules:
//!
//! * object keys limited to ASCII, sorted by code point (== byte order);
//! * no insignificant whitespace; UTF-8 output;
//! * strings keep their original Unicode sequence (no NFC/NFD), non-ASCII is
//!   never escaped, `/` is never escaped, control characters use the short
//!   forms or lowercase `\u00xx`;
//! * booleans/null lowercase, arrays keep input order;
//! * integers only, `|n| <= 2^53-1`; floats, NaN, Infinity and duplicate keys
//!   are rejected; `-0` normalizes to `0`.
//!
//! Blob digests hash raw bytes directly and never pass through this module.

use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use sha2::{Digest as ShaDigest, Sha256};
use std::collections::HashSet;
use std::fmt;

pub const MAX_SAFE_INTEGER: i128 = 9_007_199_254_740_991; // 2^53 - 1

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CanonicalError {
    #[error("floating point numbers are not allowed")]
    Float,
    #[error("integer {0} exceeds the |2^53-1| bound")]
    IntegerRange(i128),
    #[error("duplicate object key: {0}")]
    DuplicateKey(String),
    #[error("object key is not ASCII: {0}")]
    NonAsciiKey(String),
    #[error("invalid JSON: {0}")]
    InvalidJson(String),
}

/// Parse JSON while rejecting duplicate keys, non-ASCII keys, floats and
/// out-of-range integers. Use this before hashing any external payload.
pub fn parse_strict(text: &str) -> Result<serde_json::Value, CanonicalError> {
    let mut de = serde_json::Deserializer::from_str(text);
    let strict = StrictValue::deserialize(&mut de).map_err(|e| CanonicalError::InvalidJson(e.to_string()))?;
    de.end()
        .map_err(|e| CanonicalError::InvalidJson(e.to_string()))?;
    Ok(strict.0)
}

/// Canonicalize an already-parsed JSON value. Values that could not have come
/// from [`parse_strict`] (floats, out-of-range integers, non-ASCII keys) are
/// rejected here as well.
pub fn canonical_json(value: &serde_json::Value) -> Result<String, CanonicalError> {
    let mut out = String::new();
    write_value(value, &mut out)?;
    Ok(out)
}

/// `H(x) = "sha256:" + hex_lower(SHA256(UTF8(AM-C14N-1(x))))`.
pub fn digest_value(value: &serde_json::Value) -> Result<String, CanonicalError> {
    let canonical = canonical_json(value)?;
    Ok(digest_bytes(canonical.as_bytes()))
}

/// Digest raw bytes (files, blobs, artifacts) without JSON normalization.
pub fn digest_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("sha256:{}", hex_lower(&hasher.finalize()))
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn write_value(value: &serde_json::Value, out: &mut String) -> Result<(), CanonicalError> {
    use serde_json::Value;
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Number(number) => {
            if let Some(i) = number.as_i64() {
                if (i as i128).unsigned_abs() > MAX_SAFE_INTEGER as u128 {
                    return Err(CanonicalError::IntegerRange(i as i128));
                }
                out.push_str(&i.to_string());
            } else if let Some(u) = number.as_u64() {
                if (u as i128) > MAX_SAFE_INTEGER {
                    return Err(CanonicalError::IntegerRange(u as i128));
                }
                out.push_str(&u.to_string());
            } else {
                return Err(CanonicalError::Float);
            }
        }
        Value::String(text) => write_string(text, out),
        Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_value(item, out)?;
            }
            out.push(']');
        }
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            for key in &keys {
                if !key.is_ascii() {
                    return Err(CanonicalError::NonAsciiKey((*key).clone()));
                }
            }
            keys.sort_unstable();
            out.push('{');
            for (index, key) in keys.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_string(key, out);
                out.push(':');
                write_value(&map[*key], out)?;
            }
            out.push('}');
        }
    }
    Ok(())
}

fn write_string(text: &str, out: &mut String) {
    out.push('"');
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{0008}' => out.push_str("\\b"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\u{000c}' => out.push_str("\\f"),
            '\r' => out.push_str("\\r"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

struct StrictValue(serde_json::Value);

impl<'de> Deserialize<'de> for StrictValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(StrictVisitor)
    }
}

struct StrictVisitor;

impl<'de> Visitor<'de> for StrictVisitor {
    type Value = StrictValue;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON value in the AM-C14N-1 domain")
    }

    fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E> {
        Ok(StrictValue(serde_json::Value::Bool(value)))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        if (value as i128).unsigned_abs() > MAX_SAFE_INTEGER as u128 {
            return Err(E::custom(format!(
                "integer {value} exceeds the |2^53-1| bound"
            )));
        }
        Ok(StrictValue(serde_json::Value::Number(serde_json::Number::from(
            value,
        ))))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        if (value as i128) > MAX_SAFE_INTEGER {
            return Err(E::custom(format!(
                "integer {value} exceeds the |2^53-1| bound"
            )));
        }
        Ok(StrictValue(serde_json::Value::Number(serde_json::Number::from(
            value,
        ))))
    }

    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Err(E::custom(format!(
            "floating point numbers are not allowed ({value})"
        )))
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(StrictValue(serde_json::Value::String(value.to_owned())))
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E> {
        Ok(StrictValue(serde_json::Value::String(value)))
    }

    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(StrictValue(serde_json::Value::Null))
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(StrictValue(serde_json::Value::Null))
    }

    fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut items = Vec::new();
        while let Some(item) = seq.next_element::<StrictValue>()? {
            items.push(item.0);
        }
        Ok(StrictValue(serde_json::Value::Array(items)))
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut object = serde_json::Map::new();
        let mut seen = HashSet::new();
        while let Some(key) = map.next_key::<String>()? {
            if !key.is_ascii() {
                return Err(de::Error::custom(format!(
                    "object key is not ASCII: {key}"
                )));
            }
            if !seen.insert(key.clone()) {
                return Err(de::Error::custom(format!("duplicate object key: {key}")));
            }
            let value = map.next_value::<StrictValue>()?;
            object.insert(key, value.0);
        }
        Ok(StrictValue(serde_json::Value::Object(object)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sorts_ascii_keys_and_keeps_unicode() {
        let canonical = canonical_json(&json!({"z": 1, "a": "中文😀"})).unwrap();
        assert_eq!(canonical, r#"{"a":"中文😀","z":1}"#);
    }

    #[test]
    fn escapes_control_characters() {
        let canonical =
            canonical_json(&json!({"a": [null, "line\r\n", "\u{0001}", -3], "b": true})).unwrap();
        assert_eq!(canonical, r#"{"a":[null,"line\r\n","\u0001",-3],"b":true}"#);
    }

    #[test]
    fn rejects_duplicate_keys_at_parse_time() {
        let err = parse_strict(r#"{"a":1,"a":2}"#).unwrap_err();
        assert!(err.to_string().contains("duplicate object key"));
    }

    #[test]
    fn rejects_floats_and_large_integers() {
        assert!(parse_strict("1.5").is_err());
        assert!(parse_strict("9007199254740992").is_err());
        assert!(parse_strict("9007199254740991").is_ok());
    }

    #[test]
    fn digest_matches_spec_vector() {
        let value = json!({"z": 1, "a": "中文😀"});
        assert_eq!(
            digest_value(&value).unwrap(),
            "sha256:8fbd973f9e57bb947d29ee5cf71a58798b1432a2cc175f609ac4776b58425e42"
        );
    }
}
