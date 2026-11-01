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

pub(crate) fn evaluate_numeric_tolerance(
    store: &mut Store,
    predicate: &Value,
    root: &Path,
    checker: Option<&str>,
    exit_code: Option<i32>,
) -> Outcome {
    if let Some(checker) = checker {
        if checker != NUMERIC_TOLERANCE_CHECKER {
            return (
                "unsupported".to_string(),
                json!({
                    "reason": format!(
                        "checker {checker} is not registered for numeric_tolerance"
                    ),
                    "checker_id": checker,
                }),
                vec![],
            );
        }
    }
    if let Some(failure) = command_exit_failure(predicate, exit_code) {
        return failure;
    }
    let semantics = optional_string(predicate, "semantics")
        .unwrap_or_else(|| "ieee754_elementwise".to_string());
    if semantics != "ieee754_elementwise" {
        return (
            "unsupported".to_string(),
            json!({
                "reason": format!("numeric semantics {semantics:?} are not implemented"),
                "supported_semantics": ["ieee754_elementwise"],
                "unsupported_semantics": [
                    "MATLAB NaN-ignoring min/max",
                    "MATLAB implicit expansion",
                    "MATLAB column-major reshape and colon/linspace",
                    "MATLAB eps-based default tolerances"
                ],
                "implemented_comparison_features": [
                    "nonfinite_policy=ignore_nan",
                    "tolerance_model=matlab_eps",
                    "normalization=sample|population for variance/stddev"
                ],
                "scope_note": "ARCHMELD compares numeric artifacts; MATLAB expression-language semantics belong to the producer and are not emulated"
            }),
            vec![],
        );
    }
    let mode = optional_string(predicate, "mode")
        .unwrap_or_else(|| "absolute_relative".to_string());
    if !matches!(mode.as_str(), "absolute_relative" | "ulp" | "statistical") {
        return (
            "error".to_string(),
            json!({
                "reason": format!("unknown numeric comparison mode {mode:?}"),
                "supported_modes": ["absolute_relative", "ulp", "statistical"],
            }),
            vec![],
        );
    }
    let (expected_reference, actual_reference) = match (
        optional_string(predicate, "expected_artifact"),
        optional_string(predicate, "actual_artifact"),
    ) {
        (Some(expected), Some(actual)) => (expected, actual),
        (None, _) => {
            return (
                "error".to_string(),
                json!({"reason": "numeric_tolerance requires expected_artifact"}),
                vec![],
            )
        }
        (_, None) => {
            return (
                "error".to_string(),
                json!({"reason": "numeric_tolerance requires actual_artifact"}),
                vec![],
            )
        }
    };
    let tolerance = if mode == "absolute_relative" {
        match tolerance_model(predicate) {
            Ok(value) => Some(value),
            Err(error) => return ("error".to_string(), json!({"reason": error}), vec![]),
        }
    } else {
        if optional_string(predicate, "tolerance_model").is_some()
            || optional_string(predicate, "eps_multiplier").is_some()
        {
            return (
                "error".to_string(),
                json!({"reason": "tolerance_model and eps_multiplier apply only to absolute_relative mode"}),
                vec![],
            );
        }
        None
    };
    let policy = match nonfinite_policy(predicate) {
        Ok(policy) => policy,
        Err(error) => return ("error".to_string(), json!({"reason": error}), vec![]),
    };
    let columns = match requested_columns(predicate) {
        Ok(columns) => columns,
        Err(error) => return ("error".to_string(), json!({"reason": error}), vec![]),
    };
    let unit_mode = match unit_mode(predicate) {
        Ok(mode) => mode,
        Err(error) => return ("error".to_string(), json!({"reason": error}), vec![]),
    };
    let registry = match unit_registry(predicate) {
        Ok(registry) => registry,
        Err(error) => return ("error".to_string(), json!({"reason": error}), vec![]),
    };
    let normalization = match statistical_normalization(predicate) {
        Ok(value) => value,
        Err(error) => return ("error".to_string(), json!({"reason": error}), vec![]),
    };
    let max_ulps = if mode == "ulp" {
        match nonnegative_u64(predicate, "max_ulps") {
            Ok(value) => Some(value),
            Err(error) => return ("error".to_string(), json!({"reason": error}), vec![]),
        }
    } else {
        None
    };
    let thresholds = if mode == "statistical" {
        match statistical_thresholds(predicate) {
            Ok(value) => Some(value),
            Err(error) => return ("error".to_string(), json!({"reason": error}), vec![]),
        }
    } else {
        None
    };
    let expected_artifact = match resolve_artifact(store, &expected_reference, root) {
        Ok(artifact) => artifact,
        Err(error) => return ("error".to_string(), json!({"reason": error}), vec![]),
    };
    let actual_artifact = match resolve_artifact(store, &actual_reference, root) {
        Ok(artifact) => artifact,
        Err(error) => return ("error".to_string(), json!({"reason": error}), vec![]),
    };
    let expected = match parse_table(&expected_artifact.bytes, "expected_artifact")
        .and_then(|table| project(&table, &columns))
    {
        Ok(table) => table,
        Err(error) => return ("error".to_string(), json!({"reason": error}), vec![]),
    };
    let actual = match parse_table(&actual_artifact.bytes, "actual_artifact")
        .and_then(|table| project(&table, &columns))
    {
        Ok(table) => table,
        Err(error) => return ("error".to_string(), json!({"reason": error}), vec![]),
    };
    let artifacts = vec![
        artifact_entry("expected", &expected_artifact),
        artifact_entry("actual", &actual_artifact),
    ];
    let (state, mut detail) = match mode.as_str() {
        "ulp" => compare_ulp_tables(
            &expected,
            &actual,
            max_ulps.unwrap_or(0),
            policy,
            unit_mode,
            &registry,
        ),
        "statistical" => compare_statistical_tables(
            &expected,
            &actual,
            thresholds.as_deref().unwrap_or(&[]),
            normalization,
            policy,
            unit_mode,
            &registry,
        ),
        _ => compare_tables(
            &expected,
            &actual,
            tolerance.unwrap_or(ToleranceModel::AbsoluteRelative { atol: 0.0, rtol: 0.0 }),
            policy,
            unit_mode,
            &registry,
        ),
    };
    detail["kind"] = json!("numeric_tolerance");
    detail["mode"] = json!(mode);
    detail["semantics"] = json!(semantics);
    detail["custom_units"] = json!(registry.names());
    detail["expected_artifact"] = json!(expected_reference);
    detail["actual_artifact"] = json!(actual_reference);
    detail["artifacts"] = json!(artifacts);
    (state, detail, artifacts)
}

fn statistical_thresholds(
    predicate: &Value,
) -> std::result::Result<Vec<(&'static str, f64)>, String> {
    let object = param(predicate, "statistics")
        .and_then(Value::as_object)
        .ok_or_else(|| "statistical mode requires a statistics object".to_string())?;
    if object.is_empty() {
        return Err("statistical mode statistics must not be empty".to_string());
    }
    let mut thresholds = Vec::new();
    for (name, value) in object {
        let metric = match name.as_str() {
            "mean_abs_error_max" => "mean_abs_error",
            "rms_error_max" => "rms_error",
            "max_abs_error_max" => "max_abs_error",
            "p95_abs_error_max" => "p95_abs_error",
            "variance_max" => "variance",
            "stddev_max" => "stddev",
            other => {
                return Err(format!(
                    "unknown statistical threshold {other:?}; expected mean_abs_error_max, rms_error_max, max_abs_error_max, p95_abs_error_max, variance_max or stddev_max"
                ))
            }
        };
        let text = value
            .as_str()
            .ok_or_else(|| format!("statistical threshold {name} must be a decimal string"))?;
        let threshold = parse_decimal(text, name)?;
        if threshold < 0.0 {
            return Err(format!("statistical threshold {name} must not be negative"));
        }
        thresholds.push((metric, threshold));
    }
    Ok(thresholds)
}

fn ulp_distance(left: f64, right: f64) -> u64 {
    if left == right {
        return 0;
    }
    let key = |value: f64| -> u64 {
        let bits = value.to_bits();
        if bits & (1u64 << 63) != 0 {
            !bits
        } else {
            bits | (1u64 << 63)
        }
    };
    key(left).abs_diff(key(right))
}

fn compare_ulp_tables(
    expected: &NumericTable,
    actual: &NumericTable,
    max_ulps: u64,
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
    let mut max_distance = 0u64;
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
                            "reason": "non-finite value under ULP comparison",
                        }));
                    }
                }
                continue;
            }
            let distance = ulp_distance(*expected_value, *actual_value);
            max_distance = max_distance.max(distance);
            if distance > max_ulps {
                mismatch_count += 1;
                if mismatches.len() < MAX_MISMATCHES {
                    mismatches.push(json!({
                        "row": row_index,
                        "column": column,
                        "expected": number_value(*expected_value),
                        "actual": number_value(*actual_value),
                        "ulp_distance": distance.to_string(),
                    }));
                }
            }
        }
    }
    let state = if mismatch_count == 0 { "passed" } else { "failed" };
    (
        state.to_string(),
        json!({
            "shape": expected_shape,
            "columns": expected.columns,
            "units": expected.units,
            "unit_mode": match unit_mode { UnitMode::Exact => "exact", UnitMode::Dimension => "dimension" },
            "max_ulps": max_ulps.to_string(),
            "max_ulp_distance": max_distance.to_string(),
            "nonfinite_policy": match policy {
                NonFinitePolicy::Reject => "reject",
                NonFinitePolicy::Match => "match",
                NonFinitePolicy::IgnoreNan => "ignore_nan",
            },
            "compared": compared,
            "mismatch_count": mismatch_count,
            "nonfinite_count": nonfinite_count,
            "ignored_nan": ignored_nan,
            "mismatches": mismatches,
            "mismatches_truncated": mismatch_count > MAX_MISMATCHES,
        }),
    )
}

fn compare_statistical_tables(
    expected: &NumericTable,
    actual: &NumericTable,
    thresholds: &[(&'static str, f64)],
    normalization: StatisticalNormalization,
    policy: NonFinitePolicy,
    unit_mode: UnitMode,
    registry: &crate::units::Registry,
) -> (String, Value) {
    let expected_shape = [expected.rows.len(), expected.columns.len()];
    if let Some(failure) = compare_metadata(expected, actual, unit_mode, registry) {
        return failure;
    }
    let mut errors = Vec::new();
    let mut nonfinite_count = 0usize;
    let mut nonfinite_mismatch = 0usize;
    let mut ignored_nan = 0usize;
    for (expected_row, actual_row) in expected.rows.iter().zip(actual.rows.iter()) {
        for (expected_value, actual_value) in expected_row.iter().zip(actual_row.iter()) {
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
                    nonfinite_mismatch += 1;
                }
                continue;
            }
            errors.push((*expected_value - *actual_value).abs());
        }
    }
    if nonfinite_mismatch > 0 {
        return (
            "failed".to_string(),
            json!({
                "shape": expected_shape,
                "reason": "non-finite values do not match the declared policy",
                "nonfinite_count": nonfinite_count,
                "nonfinite_mismatch": nonfinite_mismatch,
                "ignored_nan": ignored_nan,
                "thresholds": thresholds
                    .iter()
                    .map(|(metric, threshold)| json!({"metric": metric, "max": number_value(*threshold)}))
                    .collect::<Vec<_>>(),
            }),
        );
    }
    if errors.is_empty() {
        return (
            "inconclusive".to_string(),
            json!({
                "shape": expected_shape,
                "reason": "there are no finite value pairs to compare",
                "nonfinite_count": nonfinite_count,
                "ignored_nan": ignored_nan,
            }),
        );
    }
    let count = errors.len() as f64;
    let mean_abs_error = errors.iter().sum::<f64>() / count;
    let rms_error = (errors.iter().map(|error| error * error).sum::<f64>() / count).sqrt();
    let max_abs_error = errors.iter().copied().fold(0.0_f64, f64::max);
    let mut sorted = errors.clone();
    sorted.sort_by(|left, right| left.total_cmp(right));
    let rank = ((sorted.len() as f64) * 0.95).ceil() as usize;
    let p95_abs_error = sorted[rank.saturating_sub(1).min(sorted.len() - 1)];
    let needs_variance = thresholds
        .iter()
        .any(|(metric, _)| *metric == "variance" || *metric == "stddev");
    if needs_variance && normalization == StatisticalNormalization::Sample && errors.len() < 2 {
        return (
            "inconclusive".to_string(),
            json!({
                "shape": expected_shape,
                "reason": "sample variance/stddev require at least two finite value pairs",
                "compared": errors.len(),
                "ignored_nan": ignored_nan,
            }),
        );
    }
    let divisor = match normalization {
        StatisticalNormalization::Population => count,
        StatisticalNormalization::Sample => count - 1.0,
    };
    let variance = if divisor > 0.0 {
        errors
            .iter()
            .map(|error| (error - mean_abs_error).powi(2))
            .sum::<f64>()
            / divisor
    } else {
        0.0
    };
    let stddev = variance.sqrt();
    let metrics: Vec<(&'static str, f64)> = vec![
        ("mean_abs_error", mean_abs_error),
        ("rms_error", rms_error),
        ("max_abs_error", max_abs_error),
        ("p95_abs_error", p95_abs_error),
        ("variance", variance),
        ("stddev", stddev),
    ];
    let mut failed = Vec::new();
    for (metric, threshold) in thresholds {
        if let Some((name, observed)) = metrics.iter().find(|(name, _)| name == metric) {
            if *observed > *threshold {
                failed.push(json!({
                    "metric": name,
                    "observed": number_value(*observed),
                    "max": number_value(*threshold),
                }));
            }
        }
    }
    let state = if failed.is_empty() { "passed" } else { "failed" };
    (
        state.to_string(),
        json!({
            "shape": expected_shape,
            "columns": expected.columns,
            "units": expected.units,
            "unit_mode": match unit_mode { UnitMode::Exact => "exact", UnitMode::Dimension => "dimension" },
            "normalization": match normalization {
                StatisticalNormalization::Population => "population",
                StatisticalNormalization::Sample => "sample",
            },
            "nonfinite_policy": match policy {
                NonFinitePolicy::Reject => "reject",
                NonFinitePolicy::Match => "match",
                NonFinitePolicy::IgnoreNan => "ignore_nan",
            },
            "compared": errors.len(),
            "nonfinite_count": nonfinite_count,
            "ignored_nan": ignored_nan,
            "metrics": metrics
                .iter()
                .map(|(name, value)| json!({"metric": name, "value": number_value(*value)}))
                .collect::<Vec<_>>(),
            "thresholds": thresholds
                .iter()
                .map(|(metric, threshold)| json!({"metric": metric, "max": number_value(*threshold)}))
                .collect::<Vec<_>>(),
            "failed_metrics": failed,
            "mismatch_count": failed.len(),
        }),
    )
}

fn invariant_rules(predicate: &Value) -> std::result::Result<(Value, Vec<Value>), String> {
    let configuration = param(predicate, "configuration")
        .or_else(|| {
            if predicate.get("rules").is_some() {
                Some(predicate)
            } else {
                None
            }
        })
        .ok_or_else(|| "invariant requires a configuration object".to_string())?;
    let rules = configuration
        .get("rules")
        .and_then(Value::as_array)
        .ok_or_else(|| "invariant configuration requires a rules array".to_string())?;
    if rules.is_empty() {
        return Err("invariant configuration rules must not be empty".to_string());
    }
    if rules.len() > 100 {
        return Err("invariant configuration has more than 100 rules".to_string());
    }
    Ok((configuration.clone(), rules.clone()))
}

fn find_column<'a>(
    table: &'a NumericTable,
    name: &str,
) -> std::result::Result<usize, String> {
    table
        .columns
        .iter()
        .position(|column| column == name)
        .ok_or_else(|| {
            format!(
                "column {name:?} is not present; available columns: {}",
                table.columns.join(", ")
            )
        })
}

fn selected_indices(
    table: &NumericTable,
    rule: &Value,
) -> std::result::Result<Vec<usize>, String> {
    match rule.get("column").and_then(Value::as_str) {
        Some(column) => Ok(vec![find_column(table, column)?]),
        None => Ok((0..table.columns.len()).collect()),
    }
}

fn values_of(table: &NumericTable, column_index: usize) -> Vec<f64> {
    table.rows.iter().map(|row| row[column_index]).collect()
}

fn evaluate_one_invariant(
    table: &NumericTable,
    column_index: usize,
    rule: &Value,
) -> std::result::Result<(bool, Value), String> {
    let kind = rule
        .get("kind")
        .and_then(Value::as_str)
        .ok_or_else(|| "invariant rule requires kind".to_string())?;
    let values = values_of(table, column_index);
    let column = &table.columns[column_index];
    match kind {
        "finite" => {
            let violations: Vec<usize> = values
                .iter()
                .enumerate()
                .filter(|(_, value)| !value.is_finite())
                .map(|(index, _)| index)
                .take(MAX_MISMATCHES)
                .collect();
            Ok((
                violations.is_empty(),
                json!({"column": column, "rule": kind, "violations": violations}),
            ))
        }
        "nonnegative" => {
            let violations: Vec<Value> = values
                .iter()
                .enumerate()
                .filter(|(_, value)| !value.is_finite() || **value < 0.0)
                .map(|(index, value)| json!({"row": index, "value": number_value(*value)}))
                .take(MAX_MISMATCHES)
                .collect();
            Ok((
                violations.is_empty(),
                json!({"column": column, "rule": kind, "violations": violations}),
            ))
        }
        "range" => {
            let minimum = rule
                .get("min")
                .map(|value| {
                    value
                        .as_str()
                        .ok_or_else(|| "range min must be a decimal string".to_string())
                        .and_then(|text| parse_decimal(text, "range min"))
                })
                .transpose()?;
            let maximum = rule
                .get("max")
                .map(|value| {
                    value
                        .as_str()
                        .ok_or_else(|| "range max must be a decimal string".to_string())
                        .and_then(|text| parse_decimal(text, "range max"))
                })
                .transpose()?;
            if minimum.is_none() && maximum.is_none() {
                return Err("range rule requires min and/or max".to_string());
            }
            if let (Some(minimum), Some(maximum)) = (minimum, maximum) {
                if minimum > maximum {
                    return Err("range min must not exceed max".to_string());
                }
            }
            let violations: Vec<Value> = values
                .iter()
                .enumerate()
                .filter(|(_, value)| {
                    !value.is_finite()
                        || minimum.map(|minimum| **value < minimum).unwrap_or(false)
                        || maximum.map(|maximum| **value > maximum).unwrap_or(false)
                })
                .map(|(index, value)| json!({"row": index, "value": number_value(*value)}))
                .take(MAX_MISMATCHES)
                .collect();
            Ok((
                violations.is_empty(),
                json!({
                    "column": column,
                    "rule": kind,
                    "min": minimum.map(number_value),
                    "max": maximum.map(number_value),
                    "violations": violations,
                }),
            ))
        }
        "monotonic" => {
            let direction = rule
                .get("direction")
                .and_then(Value::as_str)
                .ok_or_else(|| "monotonic rule requires direction".to_string())?;
            let violations: Vec<Value> = values
                .windows(2)
                .enumerate()
                .filter_map(|(index, pair)| {
                    let (left, right) = (pair[0], pair[1]);
                    let broken = if !left.is_finite() || !right.is_finite() {
                        true
                    } else {
                        match direction {
                            "nondecreasing" => right < left,
                            "nonincreasing" => right > left,
                            "strictly_increasing" => right <= left,
                            "strictly_decreasing" => right >= left,
                            _ => true,
                        }
                    };
                    if broken {
                        Some(json!({
                            "row": index,
                            "left": number_value(left),
                            "right": number_value(right),
                        }))
                    } else {
                        None
                    }
                })
                .take(MAX_MISMATCHES)
                .collect();
            if !matches!(
                direction,
                "nondecreasing" | "nonincreasing" | "strictly_increasing" | "strictly_decreasing"
            ) {
                return Err(format!("unknown monotonic direction {direction:?}"));
            }
            Ok((
                violations.is_empty(),
                json!({
                    "column": column,
                    "rule": kind,
                    "direction": direction,
                    "violations": violations,
                }),
            ))
        }
        "constant" => {
            let atol = optional_decimal(rule, "atol", 0.0)?;
            let rtol = optional_decimal(rule, "rtol", 0.0)?;
            let matches = match values.first() {
                None => false,
                Some(first) => values.iter().all(|value| {
                    value.is_finite()
                        && first.is_finite()
                        && (*value - *first).abs()
                            <= atol + rtol * value.abs().max(first.abs())
                }),
            };
            Ok((
                matches,
                json!({
                    "column": column,
                    "rule": kind,
                    "reference": values.first().map(|value| number_value(*value)),
                    "atol": number_value(atol),
                    "rtol": number_value(rtol),
                }),
            ))
        }
        "sum" => {
            let expected_text = rule
                .get("expected")
                .and_then(Value::as_str)
                .ok_or_else(|| "sum rule requires expected as a decimal string".to_string())?;
            let expected = parse_decimal(expected_text, "sum expected")?;
            let atol = optional_decimal(rule, "atol", 0.0)?;
            let rtol = optional_decimal(rule, "rtol", 0.0)?;
            let total: f64 = values.iter().sum();
            let passed = total.is_finite()
                && (total - expected).abs() <= atol + rtol * total.abs().max(expected.abs());
            Ok((
                passed,
                json!({
                    "column": column,
                    "rule": kind,
                    "expected": number_value(expected),
                    "actual": number_value(total),
                    "atol": number_value(atol),
                    "rtol": number_value(rtol),
                }),
            ))
        }
        other => Err(format!("unregistered invariant rule {other:?}")),
    }
}

pub(crate) fn evaluate_invariant(
    store: &mut Store,
    predicate: &Value,
    root: &Path,
    checker: Option<&str>,
    exit_code: Option<i32>,
) -> Outcome {
    let declared_checker = checker
        .map(str::to_string)
        .or_else(|| optional_string(predicate, "checker_id"));
    if let Some(declared) = &declared_checker {
        if declared != NUMERIC_INVARIANTS_CHECKER {
            return (
                "unsupported".to_string(),
                json!({
                    "reason": format!(
                        "checker {declared} is not registered for invariant predicates"
                    ),
                    "checker_id": declared,
                }),
                vec![],
            );
        }
    }
    if let Some(failure) = command_exit_failure(predicate, exit_code) {
        return failure;
    }
    let version = match optional_string(predicate, "version") {
        Some(version) => version,
        None => {
            return (
                "error".to_string(),
                json!({"reason": "invariant requires a checker version"}),
                vec![],
            )
        }
    };
    if version != "1" {
        return (
            "unsupported".to_string(),
            json!({
                "reason": format!("unsupported invariant checker version {version:?}"),
                "version": version,
            }),
            vec![],
        );
    }
    let (configuration, rules) = match invariant_rules(predicate) {
        Ok(value) => value,
        Err(error) => return ("error".to_string(), json!({"reason": error}), vec![]),
    };
    let artifact_reference = match configuration
        .get("artifact")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        Some(value) => value.to_string(),
        None => {
            return (
                "error".to_string(),
                json!({"reason": "invariant configuration requires artifact"}),
                vec![],
            )
        }
    };
    let artifact = match resolve_artifact(store, &artifact_reference, root) {
        Ok(artifact) => artifact,
        Err(error) => return ("error".to_string(), json!({"reason": error}), vec![]),
    };
    let table = match parse_table(&artifact.bytes, "invariant artifact") {
        Ok(table) => table,
        Err(error) => return ("error".to_string(), json!({"reason": error}), vec![]),
    };
    let columns = match configuration.get("columns") {
        Some(value) => match value.as_array() {
            Some(items) => {
                let mut requested = Vec::new();
                for item in items {
                    match item.as_str() {
                        Some(name) if !name.is_empty() => requested.push(name.to_string()),
                        _ => {
                            return (
                                "error".to_string(),
                                json!({"reason": "invariant columns must be non-empty strings"}),
                                vec![],
                            )
                        }
                    }
                }
                match project(&table, &requested) {
                    Ok(table) => table,
                    Err(error) => {
                        return ("error".to_string(), json!({"reason": error}), vec![])
                    }
                }
            }
            None => {
                return (
                    "error".to_string(),
                    json!({"reason": "invariant columns must be an array"}),
                    vec![],
                )
            }
        },
        None => table,
    };
    if let Some(shape) = configuration.get("shape") {
        let shape_array = shape.as_array().map(|items| {
            items
                .iter()
                .filter_map(Value::as_u64)
                .map(|value| value as usize)
                .collect::<Vec<_>>()
        });
        if shape_array.as_deref() != Some(&[columns.rows.len(), columns.columns.len()]) {
            return (
                "failed".to_string(),
                json!({
                    "reason": "shape mismatch",
                    "expected_shape": shape,
                    "actual_shape": [columns.rows.len(), columns.columns.len()],
                }),
                vec![],
            );
        }
    }
    if let Some(units) = configuration.get("units") {
        let expected_units = match parse_units(Some(units), &columns.columns, "invariant") {
            Ok(units) => units,
            Err(error) => return ("error".to_string(), json!({"reason": error}), vec![]),
        };
        let mut mismatches = Vec::new();
        for (index, expected) in expected_units.iter().enumerate() {
            if expected.is_some() && expected != &columns.units[index] {
                mismatches.push(json!({
                    "column": columns.columns[index],
                    "expected_unit": expected,
                    "actual_unit": columns.units[index],
                }));
            }
        }
        if !mismatches.is_empty() {
            return (
                "failed".to_string(),
                json!({"reason": "unit mismatch", "unit_mismatches": mismatches}),
                vec![],
            );
        }
    }
    let mut results = Vec::new();
    let mut unsupported = false;
    let mut failed = false;
    for (index, rule) in rules.iter().enumerate() {
        let rule_id = rule
            .get("id")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| format!("rule.{index}"));
        let indices = match selected_indices(&columns, rule) {
            Ok(indices) => indices,
            Err(error) => {
                results.push(json!({"rule_id": rule_id, "state": "error", "reason": error}));
                failed = true;
                continue;
            }
        };
        for column_index in indices {
            match evaluate_one_invariant(&columns, column_index, rule) {
                Ok((true, detail)) => {
                    results.push(json!({"rule_id": rule_id, "state": "passed", "detail": detail}))
                }
                Ok((false, detail)) => {
                    failed = true;
                    results.push(json!({"rule_id": rule_id, "state": "failed", "detail": detail}));
                }
                Err(error) => {
                    if error.starts_with("unregistered invariant rule") {
                        unsupported = true;
                        results.push(json!({
                            "rule_id": rule_id,
                            "state": "unsupported",
                            "reason": error,
                        }));
                    } else {
                        failed = true;
                        results.push(json!({"rule_id": rule_id, "state": "error", "reason": error}));
                    }
                }
            }
        }
    }
    let state = if unsupported {
        "unsupported"
    } else if failed {
        "failed"
    } else {
        "passed"
    };
    let artifacts = vec![artifact_entry("invariant", &artifact)];
    (
        state.to_string(),
        json!({
            "kind": "invariant",
            "checker_id": NUMERIC_INVARIANTS_CHECKER,
            "version": version,
            "artifact": artifact_reference,
            "columns": columns.columns,
            "shape": [columns.rows.len(), columns.columns.len()],
            "rules_total": rules.len(),
            "rule_results": results,
            "artifacts": artifacts,
        }),
        artifacts,
    )
}

#[derive(Debug)]
struct MeasurementSet {
    samples: Vec<f64>,
    metric: String,
    unit: String,
    environment_digest: String,
    ignored_entries: usize,
}

fn parse_measurement_set(
    bytes: &[u8],
    expected_metric: &str,
    expected_unit: &str,
    expected_environment: &str,
) -> std::result::Result<MeasurementSet, String> {
    let value: Value = serde_json::from_slice(bytes)
        .map_err(|err| format!("measurements artifact is not valid JSON: {err}"))?;
    let object = value
        .as_object()
        .ok_or_else(|| "measurements artifact must be a JSON object".to_string())?;
    if let Some(samples) = object.get("samples") {
        let metric = object
            .get("metric")
            .and_then(Value::as_str)
            .ok_or_else(|| "measurements artifact requires metric".to_string())?;
        let unit = object
            .get("unit")
            .and_then(Value::as_str)
            .ok_or_else(|| "measurements artifact requires unit".to_string())?;
        let environment_digest = object
            .get("environment_digest")
            .and_then(Value::as_str)
            .ok_or_else(|| "measurements artifact requires environment_digest".to_string())?;
        if metric != expected_metric {
            return Err(format!(
                "measurements cover metric {metric:?}, not {expected_metric:?}"
            ));
        }
        if unit != expected_unit {
            return Err(format!(
                "measurements use unit {unit:?}, not {expected_unit:?}"
            ));
        }
        if environment_digest != expected_environment {
            return Err(format!(
                "measurements environment {environment_digest} does not match the declared environment {expected_environment}"
            ));
        }
        let mut parsed = Vec::new();
        let items = samples
            .as_array()
            .ok_or_else(|| "measurements samples must be an array".to_string())?;
        if items.len() > MAX_SAMPLES {
            return Err(format!(
                "measurements contain more than {MAX_SAMPLES} samples"
            ));
        }
        for (index, item) in items.iter().enumerate() {
            let sample = parse_number_like(item, &format!("sample {index}"))?;
            if !sample.is_finite() || sample < 0.0 {
                return Err(format!(
                    "performance sample {index} must be finite and non-negative"
                ));
            }
            parsed.push(sample);
        }
        return Ok(MeasurementSet {
            samples: parsed,
            metric: metric.to_string(),
            unit: unit.to_string(),
            environment_digest: environment_digest.to_string(),
            ignored_entries: 0,
        });
    }
    let entries = object
        .get("measurements")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            "measurements artifact requires samples or a measurements array".to_string()
        })?;
    let mut samples = Vec::new();
    let mut ignored_entries = 0usize;
    let mut seen_metric = None;
    let mut seen_unit = None;
    let mut seen_environment = None;
    for (index, entry) in entries.iter().enumerate() {
        let entry = entry.as_object().ok_or_else(|| {
            format!("measurements entry {index} must be an object")
        })?;
        let metric = entry
            .get("metric")
            .and_then(Value::as_str)
            .unwrap_or(expected_metric);
        if metric != expected_metric {
            ignored_entries += 1;
            continue;
        }
        let unit = entry
            .get("unit")
            .and_then(Value::as_str)
            .unwrap_or(expected_unit);
        let environment = entry
            .get("environment_digest")
            .and_then(Value::as_str)
            .unwrap_or(expected_environment);
        if unit != expected_unit {
            return Err(format!(
                "measurements entry {index} uses unit {unit:?}, not {expected_unit:?}"
            ));
        }
        if environment != expected_environment {
            return Err(format!(
                "measurements entry {index} environment {environment} does not match {expected_environment}"
            ));
        }
        seen_metric.get_or_insert(metric.to_string());
        seen_unit.get_or_insert(unit.to_string());
        seen_environment.get_or_insert(environment.to_string());
        let items = entry
            .get("samples")
            .and_then(Value::as_array)
            .ok_or_else(|| format!("measurements entry {index} requires samples"))?;
        if samples.len() + items.len() > MAX_SAMPLES {
            return Err(format!(
                "measurements contain more than {MAX_SAMPLES} samples"
            ));
        }
        for (sample_index, item) in items.iter().enumerate() {
            let sample =
                parse_number_like(item, &format!("entry {index} sample {sample_index}"))?;
            if !sample.is_finite() || sample < 0.0 {
                return Err(format!(
                    "measurement sample {sample_index} in entry {index} must be finite and non-negative"
                ));
            }
            samples.push(sample);
        }
    }
    if samples.is_empty() {
        return Err(format!(
            "measurements contain no samples for metric {expected_metric:?}"
        ));
    }
    Ok(MeasurementSet {
        samples,
        metric: seen_metric.unwrap_or_else(|| expected_metric.to_string()),
        unit: seen_unit.unwrap_or_else(|| expected_unit.to_string()),
        environment_digest: seen_environment
            .unwrap_or_else(|| expected_environment.to_string()),
        ignored_entries,
    })
}

fn performance_statistic(samples: &[f64], statistic: &str) -> Option<f64> {
    if samples.is_empty() {
        return None;
    }
    let mut sorted = samples.to_vec();
    sorted.sort_by(|left, right| left.total_cmp(right));
    let length = sorted.len();
    match statistic {
        "min" => Some(sorted[0]),
        "max" => Some(sorted[length - 1]),
        "mean" => Some(sorted.iter().sum::<f64>() / length as f64),
        "median" => {
            if length % 2 == 1 {
                Some(sorted[length / 2])
            } else {
                Some((sorted[length / 2 - 1] + sorted[length / 2]) / 2.0)
            }
        }
        "p95" => {
            let rank = ((95.0 * length as f64) / 100.0).ceil() as usize;
            Some(sorted[rank.saturating_sub(1).min(length - 1)])
        }
        "p99" => {
            let rank = ((99.0 * length as f64) / 100.0).ceil() as usize;
            Some(sorted[rank.saturating_sub(1).min(length - 1)])
        }
        "sum" => Some(sorted.iter().sum()),
        "count" => Some(length as f64),
        _ => None,
    }
}

fn statistic_snapshot(samples: &[f64]) -> Value {
    json!({
        "count": samples.len(),
        "min": performance_statistic(samples, "min").map(number_value),
        "max": performance_statistic(samples, "max").map(number_value),
        "mean": performance_statistic(samples, "mean").map(number_value),
        "median": performance_statistic(samples, "median").map(number_value),
        "p95": performance_statistic(samples, "p95").map(number_value),
        "p99": performance_statistic(samples, "p99").map(number_value),
    })
}

fn reported_samples(samples: &[f64]) -> (Value, bool) {
    let truncated = samples.len() > MAX_REPORTED_SAMPLES;
    let reported = if truncated {
        &samples[..MAX_REPORTED_SAMPLES]
    } else {
        samples
    };
    (
        json!(reported.iter().map(|value| number_value(*value)).collect::<Vec<_>>()),
        truncated,
    )
}

pub(crate) fn evaluate_performance_budget(
    store: &mut Store,
    predicate: &Value,
    root: &Path,
    checker: Option<&str>,
    exit_code: Option<i32>,
) -> Outcome {
    if let Some(checker) = checker {
        if checker != PERFORMANCE_BUDGET_CHECKER {
            return (
                "unsupported".to_string(),
                json!({
                    "reason": format!(
                        "checker {checker} is not registered for performance_budget"
                    ),
                    "checker_id": checker,
                }),
                vec![],
            );
        }
    }
    if let Some(failure) = command_exit_failure(predicate, exit_code) {
        return failure;
    }
    let metric = match required_string(predicate, "metric") {
        Ok(value) => value,
        Err(error) => return ("error".to_string(), json!({"reason": error}), vec![]),
    };
    let unit = match required_string(predicate, "unit") {
        Ok(value) => value,
        Err(error) => return ("error".to_string(), json!({"reason": error}), vec![]),
    };
    let statistic = match required_string(predicate, "statistic") {
        Ok(value) => value,
        Err(error) => return ("error".to_string(), json!({"reason": error}), vec![]),
    };
    if performance_statistic(&[0.0], &statistic).is_none() {
        return (
            "error".to_string(),
            json!({
                "reason": format!("unknown performance statistic {statistic:?}"),
                "supported": ["min", "max", "mean", "median", "p95", "p99", "sum", "count"],
            }),
            vec![],
        );
    }
    let threshold = match decimal_string(predicate, "threshold") {
        Ok(value) => value,
        Err(error) => return ("error".to_string(), json!({"reason": error}), vec![]),
    };
    let comparison = match required_string(predicate, "comparison") {
        Ok(value) => value,
        Err(error) => return ("error".to_string(), json!({"reason": error}), vec![]),
    };
    let normalized_comparison = match comparison.as_str() {
        "max" | "<=" => "max",
        "min" | ">=" => "min",
        "baseline_max_ratio" | "max_ratio" => "baseline_max_ratio",
        other => {
            return (
                "error".to_string(),
                json!({
                    "reason": format!("unknown performance comparison {other:?}"),
                    "supported": ["max", "min", "baseline_max_ratio"],
                }),
                vec![],
            )
        }
    };
    if matches!(normalized_comparison, "baseline_max_ratio") && threshold <= 0.0 {
        return (
            "error".to_string(),
            json!({"reason": "baseline_max_ratio requires a positive threshold"}),
            vec![],
        );
    }
    let min_repetitions = match param(predicate, "min_repetitions").and_then(Value::as_u64) {
        Some(value) if (1..=MAX_SAMPLES as u64).contains(&value) => value as usize,
        Some(_) => {
            return (
                "error".to_string(),
                json!({"reason": format!("min_repetitions must be 1..={MAX_SAMPLES}")}),
                vec![],
            )
        }
        None => {
            return (
                "error".to_string(),
                json!({"reason": "performance_budget requires min_repetitions"}),
                vec![],
            )
        }
    };
    let environment_digest = match required_string(predicate, "environment_digest") {
        Ok(value) => value,
        Err(error) => return ("error".to_string(), json!({"reason": error}), vec![]),
    };
    if let Err(error) = validate::require_digest("environment_digest", &environment_digest) {
        return ("error".to_string(), json!({"reason": error.to_string()}), vec![]);
    }
    let measurements_reference = match optional_string(predicate, "measurements_artifact")
        .or_else(|| optional_string(predicate, "actual_artifact"))
    {
        Some(value) => value,
        None => {
            return (
                "error".to_string(),
                json!({"reason": "performance_budget requires measurements_artifact"}),
                vec![],
            )
        }
    };
    let baseline_reference = optional_string(predicate, "baseline_artifact");
    if normalized_comparison == "baseline_max_ratio" && baseline_reference.is_none() {
        return (
            "inconclusive".to_string(),
            json!({
                "reason": "baseline_max_ratio requires baseline_artifact; a missing baseline cannot prove no regression",
                "metric": metric,
                "unit": unit,
                "comparison": normalized_comparison,
            }),
            vec![],
        );
    }
    let measurements_artifact = match resolve_artifact(store, &measurements_reference, root) {
        Ok(artifact) => artifact,
        Err(error) => return ("error".to_string(), json!({"reason": error}), vec![]),
    };
    let measurement = match parse_measurement_set(
        &measurements_artifact.bytes,
        &metric,
        &unit,
        &environment_digest,
    ) {
        Ok(measurement) => measurement,
        Err(error) => {
            return (
                "inconclusive".to_string(),
                json!({
                    "reason": error,
                    "metric": metric,
                    "unit": unit,
                    "environment_digest": environment_digest,
                }),
                vec![artifact_entry("measurements", &measurements_artifact)],
            )
        }
    };
    let mut artifacts = vec![artifact_entry("measurements", &measurements_artifact)];
    let candidate_statistic = performance_statistic(&measurement.samples, &statistic)
        .expect("statistic was validated above");
    let mut baseline_summary = Value::Null;
    let mut baseline_statistic = None;
    if let Some(baseline_reference) = &baseline_reference {
        let baseline_artifact = match resolve_artifact(store, baseline_reference, root) {
            Ok(artifact) => artifact,
            Err(error) => return ("error".to_string(), json!({"reason": error}), vec![]),
        };
        artifacts.push(artifact_entry("baseline", &baseline_artifact));
        let baseline = match parse_measurement_set(
            &baseline_artifact.bytes,
            &metric,
            &unit,
            &environment_digest,
        ) {
            Ok(baseline) => baseline,
            Err(error) => {
                return (
                    "inconclusive".to_string(),
                    json!({
                        "reason": format!("baseline is not comparable: {error}"),
                        "metric": metric,
                        "unit": unit,
                        "environment_digest": environment_digest,
                    }),
                    artifacts,
                )
            }
        };
        baseline_statistic = performance_statistic(&baseline.samples, &statistic);
        baseline_summary = json!({
            "samples": reported_samples(&baseline.samples).0,
            "samples_truncated": reported_samples(&baseline.samples).1,
            "statistics": statistic_snapshot(&baseline.samples),
            "metric": baseline.metric,
            "unit": baseline.unit,
            "environment_digest": baseline.environment_digest,
        });
    }
    let (samples_json, samples_truncated) = reported_samples(&measurement.samples);
    let coverage_satisfied = measurement.samples.len() >= min_repetitions;
    let mut detail = json!({
        "kind": "performance_budget",
        "metric": measurement.metric,
        "unit": measurement.unit,
        "environment_digest": measurement.environment_digest,
        "statistic": statistic,
        "statistic_value": number_value(candidate_statistic),
        "threshold": number_value(threshold),
        "comparison": normalized_comparison,
        "min_repetitions": min_repetitions,
        "samples": samples_json,
        "samples_truncated": samples_truncated,
        "statistics": statistic_snapshot(&measurement.samples),
        "coverage": {
            "samples": measurement.samples.len(),
            "min_repetitions": min_repetitions,
            "satisfied": coverage_satisfied,
        },
        "ignored_entries": measurement.ignored_entries,
        "baseline": baseline_summary,
        "artifacts": artifacts,
    });
    if !coverage_satisfied {
        detail["reason"] = json!(
            "the measurement has fewer repetitions than the frozen contract requires"
        );
        return ("inconclusive".to_string(), detail, artifacts);
    }
    let passed = match normalized_comparison {
        "max" => candidate_statistic <= threshold,
        "min" => candidate_statistic >= threshold,
        "baseline_max_ratio" => {
            let baseline_value = baseline_statistic.expect("checked above");
            if baseline_value == 0.0 {
                detail["reason"] = json!(
                    "baseline statistic is zero, so a regression ratio is undefined"
                );
                return ("inconclusive".to_string(), detail, artifacts);
            }
            let ratio = candidate_statistic / baseline_value;
            detail["ratio"] = number_value(ratio);
            detail["delta"] = number_value(candidate_statistic - baseline_value);
            ratio <= threshold
        }
        _ => unreachable!(),
    };
    if let (Some(baseline_value), true) = (baseline_statistic, normalized_comparison != "baseline_max_ratio") {
        detail["ratio"] = number_value(if baseline_value == 0.0 {
            0.0
        } else {
            candidate_statistic / baseline_value
        });
        detail["delta"] = number_value(candidate_statistic - baseline_value);
    }
    (
        if passed { "passed" } else { "failed" }.to_string(),
        detail,
        artifacts,
    )
}

#[cfg(test)]
mod numeric_mode_tests {
    use super::*;

    fn fixture() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open_in_memory(dir.path()).unwrap();
        (dir, store)
    }

    fn artifact(store: &mut Store, bytes: &[u8], media_type: &str) -> String {
        store.put_blob(bytes, media_type).unwrap()
    }

    fn run(store: &mut Store, root: &Path, predicate: Value) -> Outcome {
        evaluate_numeric_tolerance(
            store,
            &predicate,
            root,
            Some(NUMERIC_TOLERANCE_CHECKER),
            Some(0),
        )
    }

    #[test]
    fn ulp_mode_detects_one_bit_and_respects_the_declared_budget() {
        let (dir, mut store) = fixture();
        let expected = artifact(&mut store, b"value\n1.0\n2.0\n", "text/csv");
        let actual = artifact(
            &mut store,
            b"value\n1.0000000000000002\n2.0\n",
            "text/csv",
        );
        let base = json!({
            "expected_artifact": expected,
            "actual_artifact": actual,
            "nonfinite_policy": "reject",
            "mode": "ulp",
        });
        let mut local = base.clone();
        local["max_ulps"] = json!("0");
        let (state, detail, _) = run(&mut store, dir.path(), local);
        assert_eq!(state, "failed", "{detail}");
        assert_eq!(detail["max_ulp_distance"], "1");
        assert_eq!(detail["mismatch_count"], 1);

        let mut tolerant = base.clone();
        tolerant["max_ulps"] = json!("1");
        let (state, detail, _) = run(&mut store, dir.path(), tolerant);
        assert_eq!(state, "passed", "{detail}");
        assert_eq!(detail["max_ulp_distance"], "1");

        let mut missing = base;
        missing.as_object_mut().unwrap().remove("max_ulps");
        let (state, detail, _) = run(&mut store, dir.path(), missing);
        assert_eq!(state, "error");
        assert!(detail["reason"].as_str().unwrap().contains("max_ulps"));
    }

    #[test]
    fn statistical_mode_gates_the_error_distribution() {
        let (dir, mut store) = fixture();
        let expected = artifact(&mut store, b"value\n1\n2\n3\n", "text/csv");
        let actual = artifact(&mut store, b"value\n1.25\n2.25\n3.25\n", "text/csv");
        let base = json!({
            "expected_artifact": expected,
            "actual_artifact": actual,
            "nonfinite_policy": "reject",
            "mode": "statistical",
            "statistics": {
                "mean_abs_error_max": "0.25",
                "p95_abs_error_max": "0.25",
                "max_abs_error_max": "0.25",
            },
        });
        let (state, detail, _) = run(&mut store, dir.path(), base.clone());
        assert_eq!(state, "passed", "{detail}");
        assert_eq!(detail["compared"], 3);

        let mut strict = base.clone();
        strict["statistics"]["max_abs_error_max"] = json!("0.24");
        let (state, detail, _) = run(&mut store, dir.path(), strict);
        assert_eq!(state, "failed", "{detail}");
        assert_eq!(detail["failed_metrics"][0]["metric"], "max_abs_error");

        let mut unknown = base.clone();
        unknown["statistics"]["median_abs_error_max"] = json!("0.1");
        let (state, detail, _) = run(&mut store, dir.path(), unknown);
        assert_eq!(state, "error");
        assert!(detail["reason"].as_str().unwrap().contains("unknown statistical"));
    }

    #[test]
    fn dimension_mode_accepts_equivalent_units_and_refuses_unknown_ones() {
        let (dir, mut store) = fixture();
        let expected = artifact(
            &mut store,
            br#"{"data":[[1.0],[2.0]],"columns":["v"],"units":{"v":"V"}}"#,
            "application/json",
        );
        let equivalent = artifact(
            &mut store,
            br#"{"data":[[1.0],[2.0]],"columns":["v"],"units":{"v":"kg*m^2/(A*s^3)"}}"#,
            "application/json",
        );
        let different_scale = artifact(
            &mut store,
            br#"{"data":[[1.0],[2.0]],"columns":["v"],"units":{"v":"mV"}}"#,
            "application/json",
        );
        let unknown = artifact(
            &mut store,
            br#"{"data":[[1.0],[2.0]],"columns":["v"],"units":{"v":"furlong"}}"#,
            "application/json",
        );
        let predicate = |actual: &str| {
            json!({
                "expected_artifact": expected,
                "actual_artifact": actual,
                "nonfinite_policy": "reject",
                "mode": "absolute_relative",
                "atol": "0",
                "rtol": "0",
                "unit_mode": "dimension",
            })
        };
        let (state, detail, _) = run(&mut store, dir.path(), predicate(&equivalent));
        assert_eq!(state, "passed", "{detail}");
        assert_eq!(detail["unit_mode"], "dimension");

        let (state, detail, _) = run(&mut store, dir.path(), predicate(&different_scale));
        assert_eq!(state, "failed", "{detail}");
        assert!(detail["reason"].as_str().unwrap().contains("dimension or scale"));

        let (state, detail, _) = run(&mut store, dir.path(), predicate(&unknown));
        assert_eq!(state, "inconclusive", "{detail}");
        assert!(detail["unit_inconclusive"][0]["reason"]
            .as_str()
            .unwrap()
            .contains("unknown unit"));
    }

    #[test]
    fn matlab_semantics_are_refused_not_approximated() {
        let (dir, mut store) = fixture();
        let expected = artifact(&mut store, b"value\n1\n", "text/csv");
        let actual = artifact(&mut store, b"value\n1\n", "text/csv");
        let predicate = json!({
            "expected_artifact": expected,
            "actual_artifact": actual,
            "nonfinite_policy": "reject",
            "semantics": "matlab",
        });
        let (state, detail, _) = run(&mut store, dir.path(), predicate);
        assert_eq!(state, "unsupported", "{detail}");
        assert!(detail["unsupported_semantics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item.as_str().unwrap().contains("NaN")));
    }

    #[test]
    fn unknown_mode_is_a_contract_error() {
        let (dir, mut store) = fixture();
        let expected = artifact(&mut store, b"value\n1\n", "text/csv");
        let actual = artifact(&mut store, b"value\n1\n", "text/csv");
        let predicate = json!({
            "expected_artifact": expected,
            "actual_artifact": actual,
            "nonfinite_policy": "reject",
            "mode": "bitwise",
        });
        let (state, detail, _) = run(&mut store, dir.path(), predicate);
        assert_eq!(state, "error", "{detail}");
        assert!(detail["reason"].as_str().unwrap().contains("unknown numeric comparison mode"));
    }
}

#[cfg(test)]
mod matlab_comparison_tests {
    use super::*;

    fn fixture() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open_in_memory(dir.path()).unwrap();
        (dir, store)
    }

    fn run(store: &mut Store, root: &Path, predicate: Value) -> Outcome {
        evaluate_numeric_tolerance(
            store,
            &predicate,
            root,
            Some(NUMERIC_TOLERANCE_CHECKER),
            Some(0),
        )
    }

    #[test]
    fn ignore_nan_skips_nan_pairs_but_infinity_must_match() {
        let (dir, mut store) = fixture();
        let expected = store
            .put_blob(b"value\n1\nNaN\nInf\n", "text/csv")
            .unwrap();
        let mismatched = store
            .put_blob(b"value\n1\nNaN\n-Inf\n", "text/csv")
            .unwrap();
        let matching = store.put_blob(b"value\n1\nNaN\nInf\n", "text/csv").unwrap();
        let predicate = |actual: &str| {
            json!({
                "expected_artifact": expected,
                "actual_artifact": actual,
                "atol": "0",
                "rtol": "0",
                "nonfinite_policy": "ignore_nan",
            })
        };
        let (state, detail, _) = run(&mut store, dir.path(), predicate(&mismatched));
        assert_eq!(state, "failed", "{detail}");
        assert_eq!(detail["ignored_nan"], 1);
        assert_eq!(detail["mismatch_count"], 1);
        let (state, detail, _) = run(&mut store, dir.path(), predicate(&matching));
        assert_eq!(state, "passed", "{detail}");
        assert_eq!(detail["ignored_nan"], 1);
    }

    #[test]
    fn sample_and_population_variance_are_distinct() {
        let (dir, mut store) = fixture();
        let expected = store.put_blob(b"value\n1\n2\n3\n", "text/csv").unwrap();
        let actual = store
            .put_blob(b"value\n1.25\n2.5\n3.75\n", "text/csv")
            .unwrap();
        let base = json!({
            "expected_artifact": expected,
            "actual_artifact": actual,
            "mode": "statistical",
            "nonfinite_policy": "reject",
            "statistics": {"variance_max": "0.05"},
        });
        let mut population = base.clone();
        population["normalization"] = json!("population");
        let (state, detail, _) = run(&mut store, dir.path(), population);
        assert_eq!(state, "passed", "{detail}");
        let mut sample = base;
        sample["normalization"] = json!("sample");
        let (state, detail, _) = run(&mut store, dir.path(), sample);
        assert_eq!(state, "failed", "{detail}");
        assert_eq!(detail["failed_metrics"][0]["metric"], "variance");
    }

    #[test]
    fn matlab_eps_tolerance_uses_spacing_at_the_magnitude() {
        let (dir, mut store) = fixture();
        let expected_value = 1.0_f64;
        let actual_value = 1.0_f64 + 2.0 * f64::EPSILON;
        let expected = store
            .put_blob(format!("value\n{expected_value}\n").as_bytes(), "text/csv")
            .unwrap();
        let actual = store
            .put_blob(format!("value\n{actual_value}\n").as_bytes(), "text/csv")
            .unwrap();
        let predicate = |multiplier: &str| {
            json!({
                "expected_artifact": expected,
                "actual_artifact": actual,
                "tolerance_model": "matlab_eps",
                "eps_multiplier": multiplier,
                "nonfinite_policy": "reject",
            })
        };
        let (state, detail, _) = run(&mut store, dir.path(), predicate("1"));
        assert_eq!(state, "failed", "{detail}");
        let (state, detail, _) = run(&mut store, dir.path(), predicate("2"));
        assert_eq!(state, "passed", "{detail}");
        assert_eq!(detail["tolerance_model"], "matlab_eps");
    }

    #[test]
    fn declared_units_are_available_to_dimension_mode() {
        let (dir, mut store) = fixture();
        let expected = store
            .put_blob(
                br#"{"data":[[1.0]],"columns":["v"],"units":{"v":"DN"}}"#,
                "application/json",
            )
            .unwrap();
        let actual = store
            .put_blob(
                br#"{"data":[[1.0]],"columns":["v"],"units":{"v":"1"}}"#,
                "application/json",
            )
            .unwrap();
        let predicate = |definitions: Option<Value>| {
            let mut value = json!({
                "expected_artifact": expected,
                "actual_artifact": actual,
                "atol": "0",
                "rtol": "0",
                "nonfinite_policy": "reject",
                "unit_mode": "dimension",
            });
            if let Some(definitions) = definitions {
                value["unit_definitions"] = definitions;
            }
            value
        };
        let (state, detail, _) = run(
            &mut store,
            dir.path(),
            predicate(Some(json!({"DN": "1"}))),
        );
        assert_eq!(state, "passed", "{detail}");
        assert_eq!(detail["custom_units"][0], "DN");
        let (state, detail, _) = run(&mut store, dir.path(), predicate(None));
        assert_eq!(state, "inconclusive", "{detail}");
        let (state, detail, _) = run(
            &mut store,
            dir.path(),
            predicate(Some(json!({"m": "1e-3*m"}))),
        );
        assert_eq!(state, "error", "{detail}");
        assert!(detail["reason"].as_str().unwrap().contains("shadow"));
    }
}
