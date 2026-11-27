// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! Declarative invariant rules over numeric tables.

use super::*;

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
