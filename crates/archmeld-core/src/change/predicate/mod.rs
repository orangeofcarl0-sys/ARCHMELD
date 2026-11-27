// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! Deterministic predicate engines for numeric, invariant and performance
//! contracts (S10 §5, S17 §3, S19 §5).
//!
//! These evaluators never guess tolerances, units or thresholds: every value
//! comes from the frozen contract, and an artifact that misses a declared
//! metric or environment is `inconclusive` rather than silently accepted.
//! Artifacts may be CAS digests or workspace-relative files produced by the
//! check command; resolved file bytes are stored in the CAS so the evidence
//! record keeps the exact bytes that were compared.

use crate::apply::safe_resolve;
use crate::store::{validate, Store};
use serde_json::{json, Map, Value};
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

pub(crate) const NUMERIC_TOLERANCE_CHECKER: &str = "numeric.tolerance.v1";
pub(crate) const NUMERIC_INVARIANTS_CHECKER: &str = "numeric.invariants.v1";
pub(crate) const PERFORMANCE_BUDGET_CHECKER: &str = "performance.budget.v1";

const MAX_ARTIFACT_BYTES: usize = 32 * 1024 * 1024;
const MAX_ELEMENTS: usize = 1_000_000;
const MAX_MISMATCHES: usize = 20;
const MAX_SAMPLES: usize = 100_000;
const MAX_REPORTED_SAMPLES: usize = 10_000;

type Outcome = (String, Value, Vec<Value>);

#[derive(Debug, Clone)]
struct NumericTable {
    columns: Vec<String>,
    rows: Vec<Vec<f64>>,
    units: Vec<Option<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NonFinitePolicy {
    Reject,
    Match,
    IgnoreNan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UnitMode {
    Exact,
    Dimension,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum ToleranceModel {
    AbsoluteRelative { atol: f64, rtol: f64 },
    MatlabEps { atol: f64, rtol: f64, multiplier: u64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StatisticalNormalization {
    Population,
    Sample,
}

#[derive(Debug)]
struct ResolvedArtifact {
    bytes: Vec<u8>,
    digest: Option<String>,
    media_type: String,
}

/// Read a predicate artifact either from the CAS (a `sha256:` digest) or from
/// the workspace as a root-relative path. Relative paths pass through the same
/// symlink/junction checks as staged changes.
fn resolve_artifact(
    store: &mut Store,
    reference: &str,
    root: &Path,
) -> std::result::Result<ResolvedArtifact, String> {
    if reference.starts_with("sha256:") {
        validate::require_digest("artifact", reference).map_err(|err| err.to_string())?;
        let bytes = store
            .read_blob_bytes(reference)
            .map_err(|err| err.to_string())?
            .ok_or_else(|| format!("artifact {reference} is missing from the CAS"))?;
        if bytes.len() > MAX_ARTIFACT_BYTES {
            return Err(format!(
                "artifact {reference} has {} bytes, above the {MAX_ARTIFACT_BYTES} byte limit",
                bytes.len()
            ));
        }
        return Ok(ResolvedArtifact {
            bytes,
            digest: Some(reference.to_string()),
            media_type: "application/octet-stream".to_string(),
        });
    }
    let path = safe_resolve(root, reference).map_err(|err| err.to_string())?;
    let metadata = fs::metadata(&path)
        .map_err(|err| format!("artifact {reference} is unreadable: {err}"))?;
    if !metadata.is_file() {
        return Err(format!("artifact {reference} is not a regular file"));
    }
    if metadata.len() as usize > MAX_ARTIFACT_BYTES {
        return Err(format!(
            "artifact {reference} has {} bytes, above the {MAX_ARTIFACT_BYTES} byte limit",
            metadata.len()
        ));
    }
    let bytes = fs::read(&path).map_err(|err| format!("artifact {reference}: {err}"))?;
    let media_type = if looks_like_json(&bytes) {
        "application/json"
    } else {
        "text/csv"
    };
    let digest = store
        .put_blob(&bytes, media_type)
        .map_err(|err| err.to_string())?;
    Ok(ResolvedArtifact {
        bytes,
        digest: Some(digest),
        media_type: media_type.to_string(),
    })
}

fn looks_like_json(bytes: &[u8]) -> bool {
    bytes
        .iter()
        .copied()
        .find(|byte| !byte.is_ascii_whitespace())
        .map(|byte| byte == b'{' || byte == b'[')
        .unwrap_or(false)
}

fn artifact_entry(role: &str, artifact: &ResolvedArtifact) -> Value {
    json!({
        "role": role,
        "digest": artifact.digest,
        "media_type": artifact.media_type,
    })
}

fn param<'a>(predicate: &'a Value, name: &str) -> Option<&'a Value> {
    predicate
        .get(name)
        .or_else(|| predicate.get("parameters").and_then(|value| value.get(name)))
}

fn required_string(predicate: &Value, name: &str) -> std::result::Result<String, String> {
    param(predicate, name)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| format!("predicate parameter {name} must be a non-empty string"))
}

fn optional_string(predicate: &Value, name: &str) -> Option<String> {
    param(predicate, name)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn decimal_string(predicate: &Value, name: &str) -> std::result::Result<f64, String> {
    let text = required_string(predicate, name)?;
    parse_decimal(&text, name)
}

fn optional_decimal(
    predicate: &Value,
    name: &str,
    default: f64,
) -> std::result::Result<f64, String> {
    match optional_string(predicate, name) {
        Some(text) => parse_decimal(&text, name),
        None => Ok(default),
    }
}

/// Contracts hash floats as decimal strings (AM-C14N-1). Parsing them here is
/// the only place where they become binary floating point.
fn parse_decimal(text: &str, field: &str) -> std::result::Result<f64, String> {
    let value: f64 = text
        .parse()
        .map_err(|_| format!("{field} must be a decimal string, got {text:?}"))?;
    if !value.is_finite() {
        return Err(format!("{field} must be finite, got {text:?}"));
    }
    Ok(value)
}

fn nonnegative_decimal(predicate: &Value, name: &str) -> std::result::Result<f64, String> {
    let value = decimal_string(predicate, name)?;
    if value < 0.0 {
        return Err(format!("{name} must not be negative"));
    }
    Ok(value)
}

fn nonnegative_u64(predicate: &Value, name: &str) -> std::result::Result<u64, String> {
    let text = required_string(predicate, name)?;
    text.parse::<u64>()
        .map_err(|_| format!("{name} must be a non-negative integer, got {text:?}"))
}

fn unit_mode(predicate: &Value) -> std::result::Result<UnitMode, String> {
    match optional_string(predicate, "unit_mode").as_deref().unwrap_or("exact") {
        "exact" => Ok(UnitMode::Exact),
        "dimension" => Ok(UnitMode::Dimension),
        other => Err(format!(
            "unit_mode must be exact or dimension, got {other:?}"
        )),
    }
}

fn unit_registry(predicate: &Value) -> std::result::Result<crate::units::Registry, String> {
    let definitions = match param(predicate, "unit_definitions") {
        None => std::collections::BTreeMap::new(),
        Some(value) => {
            let object = value
                .as_object()
                .ok_or_else(|| "unit_definitions must be an object".to_string())?;
            let mut definitions = std::collections::BTreeMap::new();
            for (name, expression) in object {
                let expression = expression
                    .as_str()
                    .filter(|expression| !expression.is_empty())
                    .ok_or_else(|| {
                        format!("unit definition {name} must be a non-empty expression string")
                    })?;
                definitions.insert(name.clone(), expression.to_string());
            }
            definitions
        }
    };
    crate::units::Registry::new(&definitions).map_err(|error| error.to_string())
}

fn tolerance_model(
    predicate: &Value,
) -> std::result::Result<ToleranceModel, String> {
    match optional_string(predicate, "tolerance_model")
        .as_deref()
        .unwrap_or("atol_rtol")
    {
        "atol_rtol" => Ok(ToleranceModel::AbsoluteRelative {
            atol: nonnegative_decimal(predicate, "atol")?,
            rtol: nonnegative_decimal(predicate, "rtol")?,
        }),
        "matlab_eps" => Ok(ToleranceModel::MatlabEps {
            atol: optional_decimal(predicate, "atol", 0.0)?,
            rtol: optional_decimal(predicate, "rtol", 0.0)?,
            multiplier: nonnegative_u64(predicate, "eps_multiplier")?,
        }),
        other => Err(format!(
            "tolerance_model must be atol_rtol or matlab_eps, got {other:?}"
        )),
    }
}

fn statistical_normalization(
    predicate: &Value,
) -> std::result::Result<StatisticalNormalization, String> {
    match optional_string(predicate, "normalization")
        .as_deref()
        .unwrap_or("sample")
    {
        "population" => Ok(StatisticalNormalization::Population),
        "sample" => Ok(StatisticalNormalization::Sample),
        other => Err(format!(
            "normalization must be population or sample, got {other:?}"
        )),
    }
}

fn nonfinite_policy(predicate: &Value) -> std::result::Result<NonFinitePolicy, String> {
    match required_string(predicate, "nonfinite_policy")?.as_str() {
        "reject" => Ok(NonFinitePolicy::Reject),
        "match" => Ok(NonFinitePolicy::Match),
        "ignore_nan" => Ok(NonFinitePolicy::IgnoreNan),
        other => Err(format!(
            "nonfinite_policy must be reject, match or ignore_nan, got {other:?}"
        )),
    }
}

fn command_exit_failure(predicate: &Value, exit_code: Option<i32>) -> Option<Outcome> {
    let code = exit_code?;
    if code == 0 || param(predicate, "allow_nonzero_exit").and_then(Value::as_bool) == Some(true) {
        return None;
    }
    Some((
        "failed".to_string(),
        json!({
            "reason": "the command exited non-zero before its artifacts could be trusted",
            "exit_code": code,
        }),
        vec![],
    ))
}

/// Predicate details are canonical JSON (AM-C14N-1), which rejects binary
/// floating point. Finite measurements are therefore emitted as the same
/// decimal strings used by the frozen contract; only counts stay integers.
fn number_value(value: f64) -> Value {
    if value.is_nan() {
        Value::String("NaN".to_string())
    } else if value.is_infinite() {
        Value::String(if value.is_sign_positive() {
            "Infinity".to_string()
        } else {
            "-Infinity".to_string()
        })
    } else {
        Value::String(format!("{value}"))
    }
}

fn parse_number_like(value: &Value, field: &str) -> std::result::Result<f64, String> {
    match value {
        Value::Number(number) => number
            .as_f64()
            .ok_or_else(|| format!("{field} is not representable as a finite number")),
        Value::String(text) => parse_measurement_number(text, field),
        _ => Err(format!("{field} must be a number or a decimal string")),
    }
}

fn parse_measurement_number(text: &str, field: &str) -> std::result::Result<f64, String> {
    let normalized = text.trim();
    let lower = normalized.to_ascii_lowercase();
    match lower.as_str() {
        "nan" => Ok(f64::NAN),
        "inf" | "infinity" | "+inf" | "+infinity" => Ok(f64::INFINITY),
        "-inf" | "-infinity" => Ok(f64::NEG_INFINITY),
        _ => normalized
            .parse()
            .map_err(|_| format!("{field} is not numeric: {text:?}")),
    }
}

fn split_delimited_line(line: &str, delimiter: char) -> std::result::Result<Vec<String>, String> {
    let mut fields = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '"' if quoted => {
                if chars.peek() == Some(&'"') {
                    current.push('"');
                    chars.next();
                } else {
                    quoted = false;
                }
            }
            '"' if current.trim().is_empty() && !quoted => {
                current.clear();
                quoted = true;
            }
            ch if ch == delimiter && !quoted => {
                fields.push(current.trim().to_string());
                current.clear();
            }
            _ => current.push(ch),
        }
    }
    if quoted {
        return Err("unterminated quoted field in delimited artifact".to_string());
    }
    fields.push(current.trim().to_string());
    Ok(fields)
}

fn parse_delimited(bytes: &[u8], label: &str) -> std::result::Result<NumericTable, String> {
    let text = std::str::from_utf8(bytes)
        .map_err(|_| format!("{label} is not UTF-8 text"))?
        .trim_start_matches('\u{feff}');
    let lines: Vec<&str> = text
        .lines()
        .map(|line| line.trim_end_matches('\r'))
        .filter(|line| !line.trim().is_empty())
        .collect();
    if lines.is_empty() {
        return Err(format!("{label} contains no rows"));
    }
    let delimiter = if lines[0].contains('\t') && !lines[0].contains(',') {
        '\t'
    } else if lines[0].contains(',') {
        ','
    } else if lines[0].contains(';') {
        ';'
    } else {
        ','
    };
    let first = split_delimited_line(lines[0], delimiter)?;
    let first_is_data = first
        .iter()
        .all(|field| parse_measurement_number(field, "cell").is_ok());
    let (columns, data_start) = if first_is_data {
        (
            (0..first.len()).map(|index| format!("c{index}")).collect::<Vec<_>>(),
            0,
        )
    } else {
        let mut seen = BTreeSet::new();
        for name in &first {
            if name.is_empty() {
                return Err(format!("{label} has an empty column name"));
            }
            if !seen.insert(name.clone()) {
                return Err(format!("{label} has duplicate column {name:?}"));
            }
        }
        (first.clone(), 1)
    };
    let width = columns.len();
    let mut rows = Vec::new();
    for (offset, line) in lines.iter().enumerate().skip(data_start) {
        let fields = split_delimited_line(line, delimiter)?;
        if fields.len() != width {
            return Err(format!(
                "{label} row {} has {} fields, expected {width}",
                offset + 1,
                fields.len()
            ));
        }
        let mut row = Vec::with_capacity(width);
        for (index, field) in fields.iter().enumerate() {
            row.push(parse_measurement_number(field, &format!(
                "{label} row {} column {}",
                offset + 1,
                columns[index]
            ))?);
        }
        rows.push(row);
        if rows.len().saturating_mul(width) > MAX_ELEMENTS {
            return Err(format!(
                "{label} exceeds the {MAX_ELEMENTS} element limit"
            ));
        }
    }
    Ok(NumericTable {
        columns,
        rows,
        units: vec![None; width],
    })
}

fn parse_json_table(bytes: &[u8], label: &str) -> std::result::Result<NumericTable, String> {
    let value: Value = serde_json::from_slice(bytes)
        .map_err(|err| format!("{label} is not valid JSON: {err}"))?;
    match &value {
        Value::Array(items) if items.is_empty() => Err(format!("{label} contains no rows")),
        Value::Array(items) if items.iter().all(Value::is_array) => {
            let width = items[0].as_array().map(Vec::len).unwrap_or(0);
            if width == 0 {
                return Err(format!("{label} contains an empty row"));
            }
            let mut rows = Vec::new();
            for (row_index, item) in items.iter().enumerate() {
                let fields = item.as_array().unwrap();
                if fields.len() != width {
                    return Err(format!(
                        "{label} row {row_index} has {} fields, expected {width}",
                        fields.len()
                    ));
                }
                let mut row = Vec::with_capacity(width);
                for (column, field) in fields.iter().enumerate() {
                    row.push(parse_number_like(
                        field,
                        &format!("{label} row {row_index} column {column}"),
                    )?);
                }
                rows.push(row);
            }
            check_element_limit(rows.len(), width, label)?;
            Ok(NumericTable {
                columns: (0..width).map(|index| format!("c{index}")).collect(),
                rows,
                units: vec![None; width],
            })
        }
        Value::Array(items) if items.iter().all(Value::is_object) => {
            let mut names = BTreeSet::new();
            for item in items {
                for key in item.as_object().unwrap().keys() {
                    names.insert(key.clone());
                }
            }
            if names.is_empty() {
                return Err(format!("{label} has no columns"));
            }
            let columns: Vec<String> = names.into_iter().collect();
            let width = columns.len();
            let mut rows = Vec::new();
            for (row_index, item) in items.iter().enumerate() {
                let object = item.as_object().unwrap();
                if object.len() != columns.len() {
                    return Err(format!(
                        "{label} row {row_index} does not have exactly the declared columns"
                    ));
                }
                let mut row = Vec::with_capacity(columns.len());
                for column in &columns {
                    let value = object
                        .get(column)
                        .ok_or_else(|| format!("{label} row {row_index} is missing {column}"))?;
                    row.push(parse_number_like(
                        value,
                        &format!("{label} row {row_index} column {column}"),
                    )?);
                }
                rows.push(row);
            }
            check_element_limit(rows.len(), columns.len(), label)?;
            Ok(NumericTable {
                columns,
                rows,
                units: vec![None; width],
            })
        }
        Value::Array(items)
            if items
                .iter()
                .all(|item| item.is_number() || item.is_string()) =>
        {
            let mut rows = Vec::new();
            for (row_index, item) in items.iter().enumerate() {
                rows.push(vec![parse_number_like(
                    item,
                    &format!("{label} row {row_index}"),
                )?]);
            }
            check_element_limit(rows.len(), 1, label)?;
            Ok(NumericTable {
                columns: vec!["value".to_string()],
                rows,
                units: vec![None],
            })
        }
        Value::Object(object) => table_from_object(object, label),
        _ => Err(format!(
            "{label} must be an array of numbers, an array of arrays/objects, or an object"
        )),
    }
}

fn table_from_object(
    object: &Map<String, Value>,
    label: &str,
) -> std::result::Result<NumericTable, String> {
    if let Some(data) = object.get("data") {
        let declared = match object.get("columns") {
            Some(value) => Some(
                value
                    .as_array()
                    .ok_or_else(|| format!("{label} columns must be an array"))?
                    .iter()
                    .map(|item| {
                        item.as_str()
                            .map(str::to_string)
                            .ok_or_else(|| format!("{label} column names must be strings"))
                    })
                    .collect::<std::result::Result<Vec<_>, _>>()?,
            ),
            None => None,
        };
        let data_array = data
            .as_array()
            .ok_or_else(|| format!("{label} data must be an array"))?;
        if data_array.is_empty() {
            return Err(format!("{label} contains no rows"));
        }
        let mut table = if data_array.iter().all(Value::is_array) {
            let width = data_array[0].as_array().map(Vec::len).unwrap_or(0);
            let mut rows = Vec::new();
            for (row_index, row) in data_array.iter().enumerate() {
                let fields = row.as_array().unwrap();
                if fields.len() != width {
                    return Err(format!(
                        "{label} data row {row_index} has {} fields, expected {width}",
                        fields.len()
                    ));
                }
                let mut parsed = Vec::with_capacity(width);
                for (column, field) in fields.iter().enumerate() {
                    parsed.push(parse_number_like(
                        field,
                        &format!("{label} data row {row_index} column {column}"),
                    )?);
                }
                rows.push(parsed);
            }
            let columns = match declared {
                Some(columns) if columns.len() == width => columns,
                Some(columns) => {
                    return Err(format!(
                        "{label} declares {} columns but data rows have {width}",
                        columns.len()
                    ))
                }
                None => (0..width).map(|index| format!("c{index}")).collect(),
            };
            NumericTable {
                columns,
                rows,
                units: vec![None; width],
            }
        } else if data_array.iter().all(Value::is_object) {
            let columns = declared.ok_or_else(|| {
                format!("{label} object data requires an explicit columns array")
            })?;
            let mut rows = Vec::new();
            for (row_index, row) in data_array.iter().enumerate() {
                let object = row.as_object().unwrap();
                let mut parsed = Vec::with_capacity(columns.len());
                for column in &columns {
                    let value = object.get(column).ok_or_else(|| {
                        format!("{label} data row {row_index} is missing {column}")
                    })?;
                    parsed.push(parse_number_like(
                        value,
                        &format!("{label} data row {row_index} column {column}"),
                    )?);
                }
                rows.push(parsed);
            }
            NumericTable {
                columns,
                rows,
                units: vec![None; declared_len(&data_array)],
            }
        } else if data_array
            .iter()
            .all(|item| item.is_number() || item.is_string())
        {
            let columns = declared.unwrap_or_else(|| vec!["value".to_string()]);
            if columns.len() != 1 {
                return Err(format!(
                    "{label} scalar data requires exactly one declared column"
                ));
            }
            let mut rows = Vec::new();
            for (row_index, item) in data_array.iter().enumerate() {
                rows.push(vec![parse_number_like(
                    item,
                    &format!("{label} data row {row_index}"),
                )?]);
            }
            NumericTable {
                columns,
                rows,
                units: vec![None],
            }
        } else {
            return Err(format!("{label} data must be uniform"));
        };
        table.units = parse_units(object.get("units"), &table.columns, label)?;
        check_element_limit(table.rows.len(), table.columns.len(), label)?;
        return Ok(table);
    }

    if object.is_empty() {
        return Err(format!("{label} is empty"));
    }
    let mut columns: Vec<String> = object.keys().cloned().collect();
    columns.sort();
    let mut series: Vec<Vec<f64>> = Vec::new();
    let mut scalar = false;
    for column in &columns {
        let value = object.get(column).unwrap();
        if let Some(items) = value.as_array() {
            if scalar {
                return Err(format!("{label} mixes scalar and array columns"));
            }
            let mut parsed = Vec::new();
            for (index, item) in items.iter().enumerate() {
                parsed.push(parse_number_like(
                    item,
                    &format!("{label} column {column} row {index}"),
                )?);
            }
            series.push(parsed);
        } else {
            if !series.is_empty() {
                return Err(format!("{label} mixes scalar and array columns"));
            }
            scalar = true;
            series.push(vec![parse_number_like(value, &format!("{label} {column}"))?]);
        }
    }
    if series.is_empty() {
        return Err(format!("{label} has no columns"));
    }
    let length = series[0].len();
    if series.iter().any(|values| values.len() != length) {
        return Err(format!("{label} columns have different lengths"));
    }
    let mut rows = Vec::with_capacity(length);
    for index in 0..length {
        rows.push(series.iter().map(|values| values[index]).collect());
    }
    check_element_limit(rows.len(), columns.len(), label)?;
    Ok(NumericTable {
        columns,
        rows,
        units: vec![None; object.len()],
    })
}

fn declared_len(data: &[Value]) -> usize {
    data.first()
        .and_then(Value::as_object)
        .map(Map::len)
        .unwrap_or(0)
}

fn parse_units(
    units: Option<&Value>,
    columns: &[String],
    label: &str,
) -> std::result::Result<Vec<Option<String>>, String> {
    let mut parsed = vec![None; columns.len()];
    let Some(units) = units else {
        return Ok(parsed);
    };
    match units {
        Value::Array(items) => {
            if items.len() != columns.len() {
                return Err(format!(
                    "{label} declares {} units for {} columns",
                    items.len(),
                    columns.len()
                ));
            }
            for (index, item) in items.iter().enumerate() {
                parsed[index] = match item {
                    Value::Null => None,
                    Value::String(text) if text.is_empty() => None,
                    Value::String(text) => Some(text.clone()),
                    _ => return Err(format!("{label} unit {index} must be a string or null")),
                };
            }
        }
        Value::Object(map) => {
            for (key, item) in map {
                let index = columns
                    .iter()
                    .position(|column| column == key)
                    .ok_or_else(|| format!("{label} declares a unit for unknown column {key}"))?;
                parsed[index] = match item {
                    Value::Null => None,
                    Value::String(text) if text.is_empty() => None,
                    Value::String(text) => Some(text.clone()),
                    _ => return Err(format!("{label} unit for {key} must be a string")),
                };
            }
        }
        _ => return Err(format!("{label} units must be an array or object")),
    }
    Ok(parsed)
}

fn check_element_limit(rows: usize, columns: usize, label: &str) -> std::result::Result<(), String> {
    if rows.saturating_mul(columns) > MAX_ELEMENTS {
        return Err(format!(
            "{label} exceeds the {MAX_ELEMENTS} element limit"
        ));
    }
    Ok(())
}

fn parse_table(bytes: &[u8], label: &str) -> std::result::Result<NumericTable, String> {
    if looks_like_json(bytes) {
        parse_json_table(bytes, label)
    } else {
        parse_delimited(bytes, label)
    }
}

fn requested_columns(predicate: &Value) -> std::result::Result<Vec<String>, String> {
    let Some(value) = param(predicate, "columns") else {
        return Ok(Vec::new());
    };
    let items = value
        .as_array()
        .ok_or_else(|| "columns must be an array of strings".to_string())?;
    let mut columns = Vec::new();
    for item in items {
        let name = item
            .as_str()
            .filter(|name| !name.is_empty())
            .ok_or_else(|| "columns must be non-empty strings".to_string())?;
        if columns.iter().any(|existing| existing == name) {
            return Err(format!("columns contains {name:?} more than once"));
        }
        columns.push(name.to_string());
    }
    Ok(columns)
}

fn project(
    table: &NumericTable,
    requested: &[String],
) -> std::result::Result<NumericTable, String> {
    if requested.is_empty() {
        return Ok(table.clone());
    }
    let mut indices = Vec::new();
    for name in requested {
        let index = table
            .columns
            .iter()
            .position(|column| column == name)
            .ok_or_else(|| {
                format!(
                    "column {name:?} is not present; available columns: {}",
                    table.columns.join(", ")
                )
            })?;
        indices.push(index);
    }
    let rows = table
        .rows
        .iter()
        .map(|row| indices.iter().map(|index| row[*index]).collect())
        .collect();
    let units = indices.iter().map(|index| table.units[*index].clone()).collect();
    Ok(NumericTable {
        columns: requested.to_vec(),
        rows,
        units,
    })
}

fn compare_metadata(
    expected: &NumericTable,
    actual: &NumericTable,
    unit_mode: UnitMode,
    registry: &crate::units::Registry,
) -> Option<(String, Value)> {
    let expected_shape = [expected.rows.len(), expected.columns.len()];
    let actual_shape = [actual.rows.len(), actual.columns.len()];
    if expected_shape != actual_shape {
        return Some((
            "failed".to_string(),
            json!({
                "reason": "shape mismatch",
                "expected_shape": expected_shape,
                "actual_shape": actual_shape,
                "expected_columns": expected.columns,
                "actual_columns": actual.columns,
            }),
        ));
    }
    if expected.columns != actual.columns {
        return Some((
            "failed".to_string(),
            json!({
                "reason": "column mismatch",
                "expected_columns": expected.columns,
                "actual_columns": actual.columns,
            }),
        ));
    }
    let mut unit_mismatches = Vec::new();
    let mut inconclusive = Vec::new();
    for (index, column) in expected.columns.iter().enumerate() {
        let (left, right) = (&expected.units[index], &actual.units[index]);
        match unit_mode {
            UnitMode::Exact => match (left, right) {
                (None, None) => {}
                (Some(left), Some(right)) if left == right => {}
                (left, right) => unit_mismatches.push(json!({
                    "column": column,
                    "expected_unit": left,
                    "actual_unit": right,
                })),
            },
            UnitMode::Dimension => match (left, right) {
                (Some(left), Some(right)) => {
                    match (
                        crate::units::parse_with(left, registry),
                        crate::units::parse_with(right, registry),
                    ) {
                        (Ok(expected_unit), Ok(actual_unit)) => {
                            if !expected_unit.equivalent(&actual_unit) {
                                unit_mismatches.push(json!({
                                    "column": column,
                                    "expected_unit": left,
                                    "actual_unit": right,
                                    "expected_dimension": expected_unit.describe(),
                                    "actual_dimension": actual_unit.describe(),
                                }));
                            }
                        }
                        (Err(error), _) | (_, Err(error)) => inconclusive.push(json!({
                            "column": column,
                            "expected_unit": left,
                            "actual_unit": right,
                            "reason": error.to_string(),
                        })),
                    }
                }
                (left, right) => inconclusive.push(json!({
                    "column": column,
                    "expected_unit": left,
                    "actual_unit": right,
                    "reason": "dimension mode requires a unit on both sides",
                })),
            },
        }
    }
    if !inconclusive.is_empty() {
        return Some((
            "inconclusive".to_string(),
            json!({
                "reason": "unit dimensions could not be compared",
                "unit_mode": match unit_mode { UnitMode::Exact => "exact", UnitMode::Dimension => "dimension" },
                "unit_inconclusive": inconclusive,
            }),
        ));
    }
    if !unit_mismatches.is_empty() {
        return Some((
            "failed".to_string(),
            json!({
                "reason": match unit_mode {
                    UnitMode::Exact => "unit mismatch (checked before numeric values)",
                    UnitMode::Dimension => "unit dimension or scale mismatch (checked before numeric values)",
                },
                "unit_mode": match unit_mode { UnitMode::Exact => "exact", UnitMode::Dimension => "dimension" },
                "unit_mismatches": unit_mismatches,
                "shape": expected_shape,
            }),
        ));
    }
    None
}

fn matlab_eps(value: f64) -> f64 {
    let magnitude = value.abs();
    if magnitude == 0.0 {
        return f64::from_bits(1);
    }
    let bits = magnitude.to_bits();
    let next = f64::from_bits(bits.saturating_add(1));
    if next.is_finite() {
        next - magnitude
    } else {
        magnitude - f64::from_bits(bits - 1)
    }
}

fn tolerance_value(tolerance: ToleranceModel, expected: f64, actual: f64) -> f64 {
    let scale = expected.abs().max(actual.abs());
    match tolerance {
        ToleranceModel::AbsoluteRelative { atol, rtol } => atol + rtol * scale,
        ToleranceModel::MatlabEps {
            atol,
            rtol,
            multiplier,
        } => atol + rtol * scale + multiplier as f64 * matlab_eps(scale),
    }
}

fn compare_tables(
    expected: &NumericTable,
    actual: &NumericTable,
    tolerance: ToleranceModel,
    policy: NonFinitePolicy,
    unit_mode: UnitMode,
    registry: &crate::units::Registry,
) -> (String, Value) {
    let expected_shape = [expected.rows.len(), expected.columns.len()];
    if let Some(failure) = compare_metadata(expected, actual, unit_mode, registry) {
        return failure;
    }
    let mut mismatches = Vec::new();
    let mut mismatch_count = 0usize;
    let mut nonfinite_count = 0usize;
    let mut ignored_nan = 0usize;
    let mut compared = 0usize;
    let mut max_abs_error = 0.0_f64;
    let mut max_rel_error = 0.0_f64;
    for (row_index, (expected_row, actual_row)) in
        expected.rows.iter().zip(actual.rows.iter()).enumerate()
    {
        for (column_index, (expected_value, actual_value)) in
            expected_row.iter().zip(actual_row.iter()).enumerate()
        {
            compared += 1;
            let column = &expected.columns[column_index];
            if !expected_value.is_finite() || !actual_value.is_finite() {
                nonfinite_count += 1;
                if policy == NonFinitePolicy::IgnoreNan
                    && (expected_value.is_nan() || actual_value.is_nan())
                {
                    ignored_nan += 1;
                    continue;
                }
                let matched = match policy {
                    NonFinitePolicy::Reject => false,
                    NonFinitePolicy::Match | NonFinitePolicy::IgnoreNan => {
                        (expected_value.is_nan() && actual_value.is_nan())
                            || (expected_value.is_infinite()
                                && actual_value.is_infinite()
                                && expected_value.is_sign_positive()
                                    == actual_value.is_sign_positive())
                    }
                };
                if !matched {
                    mismatch_count += 1;
                    if mismatches.len() < MAX_MISMATCHES {
                        mismatches.push(json!({
                            "row": row_index,
                            "column": column,
                            "expected": number_value(*expected_value),
                            "actual": number_value(*actual_value),
                            "reason": match policy {
                                NonFinitePolicy::Reject => "non-finite value rejected",
                                NonFinitePolicy::Match => "non-finite values do not match",
                                NonFinitePolicy::IgnoreNan => "infinite values do not match",
                            },
                        }));
                    }
                }
                continue;
            }
            let difference = (expected_value - actual_value).abs();
            let scale = expected_value.abs().max(actual_value.abs());
            let allowed = tolerance_value(tolerance, *expected_value, *actual_value);
            if difference.is_finite() {
                max_abs_error = max_abs_error.max(difference);
                let relative = if scale == 0.0 {
                    0.0
                } else {
                    difference / scale
                };
                max_rel_error = max_rel_error.max(relative);
            }
            if !(difference <= allowed) {
                mismatch_count += 1;
                if mismatches.len() < MAX_MISMATCHES {
                    mismatches.push(json!({
                        "row": row_index,
                        "column": column,
                        "expected": number_value(*expected_value),
                        "actual": number_value(*actual_value),
                        "abs_error": number_value(difference),
                        "tolerance": number_value(allowed),
                    }));
                }
            }
        }
    }
    let state = if mismatch_count == 0 { "passed" } else { "failed" };
    let (model, eps_multiplier) = match tolerance {
        ToleranceModel::AbsoluteRelative { .. } => ("atol_rtol", Value::Null),
        ToleranceModel::MatlabEps { multiplier, .. } => {
            ("matlab_eps", json!(multiplier.to_string()))
        }
    };
    (
        state.to_string(),
        json!({
            "shape": expected_shape,
            "columns": expected.columns,
            "units": expected.units,
            "atol": match tolerance {
                ToleranceModel::AbsoluteRelative { atol, .. } | ToleranceModel::MatlabEps { atol, .. } => number_value(atol),
            },
            "rtol": match tolerance {
                ToleranceModel::AbsoluteRelative { rtol, .. } | ToleranceModel::MatlabEps { rtol, .. } => number_value(rtol),
            },
            "tolerance_model": model,
            "eps_multiplier": eps_multiplier,
            "nonfinite_policy": match policy {
                NonFinitePolicy::Reject => "reject",
                NonFinitePolicy::Match => "match",
                NonFinitePolicy::IgnoreNan => "ignore_nan",
            },
            "unit_mode": match unit_mode { UnitMode::Exact => "exact", UnitMode::Dimension => "dimension" },
            "compared": compared,
            "mismatch_count": mismatch_count,
            "nonfinite_count": nonfinite_count,
            "ignored_nan": ignored_nan,
            "max_abs_error": number_value(max_abs_error),
            "max_rel_error": number_value(max_rel_error),
            "mismatches": mismatches,
            "mismatches_truncated": mismatch_count > MAX_MISMATCHES,
        }),
    )
}

pub fn evaluate_numeric_tolerance_artifacts(
    store: &mut Store,
    predicate: &Value,
    root: &Path,
) -> crate::Result<Value> {
    let (state, detail, artifacts) = evaluate_numeric_tolerance(
        store,
        predicate,
        root,
        Some(NUMERIC_TOLERANCE_CHECKER),
        None,
    );
    Ok(json!({
        "state": state,
        "detail": detail,
        "artifacts": artifacts,
    }))
}

mod numeric;
mod invariant;
mod performance;

pub(crate) use numeric::evaluate_numeric_tolerance;
pub(crate) use invariant::evaluate_invariant;
pub(crate) use performance::evaluate_performance_budget;
