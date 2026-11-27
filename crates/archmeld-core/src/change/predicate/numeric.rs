// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! Numeric tolerance, statistical and ULP comparison engines.

use super::*;

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
