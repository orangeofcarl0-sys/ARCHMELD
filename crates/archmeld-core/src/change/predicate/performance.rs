// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! Performance-budget evaluation over measurement sets.

use super::*;

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
