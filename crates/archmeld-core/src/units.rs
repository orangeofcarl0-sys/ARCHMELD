// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! Explicit dimensional analysis for numeric comparisons (B029).
//!
//! This is a deliberately bounded unit algebra: a fixed SI/derived symbol table
//! plus caller-declared multiplicative units, products, quotients, integer
//! powers and decimal scale constants. It is not a general unit-conversion
//! engine and it refuses affine or logarithmic units instead of guessing.

use crate::error::{CoreError, Result};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq)]
pub struct Dimension {
    exponents: BTreeMap<&'static str, i32>,
    scale: f64,
}

impl Dimension {
    pub fn is_dimensionless(&self) -> bool {
        self.exponents.is_empty()
    }

    pub fn scale(&self) -> f64 {
        self.scale
    }

    pub fn describe(&self) -> String {
        if self.exponents.is_empty() {
            return if (self.scale - 1.0).abs() <= 1e-12 {
                "1".to_string()
            } else {
                format!("{}*1", self.scale)
            };
        }
        let mut text = String::new();
        for (symbol, exponent) in &self.exponents {
            if !text.is_empty() {
                text.push('*');
            }
            text.push_str(symbol);
            if *exponent != 1 {
                text.push('^');
                text.push_str(&exponent.to_string());
            }
        }
        if (self.scale - 1.0).abs() > 1e-12 {
            text = format!("{}*({text})", self.scale);
        }
        text
    }

    pub fn equivalent(&self, other: &Dimension) -> bool {
        self.exponents == other.exponents
            && (self.scale - other.scale).abs() <= 1e-12 * self.scale.abs().max(other.scale.abs()).max(1.0)
    }
}

/// Caller-declared multiplicative units. Definitions are expressions over the
/// built-in table or other declared units; cycles and unknown symbols are
/// refused eagerly.
#[derive(Debug, Clone, Default)]
pub struct Registry {
    definitions: BTreeMap<String, String>,
}

impl Registry {
    pub fn new(definitions: &BTreeMap<String, String>) -> Result<Self> {
        if definitions.len() > 64 {
            return Err(CoreError::invalid(
                "unit_definitions has more than 64 entries",
            ));
        }
        let mut registry = Self {
            definitions: definitions.clone(),
        };
        for name in definitions.keys() {
            if name.is_empty() || !name.chars().all(is_symbol_char) {
                return Err(CoreError::invalid(format!(
                    "unit definition name {name:?} must be a symbol"
                )));
            }
            if builtin_dimension(name).is_some() {
                return Err(CoreError::invalid(format!(
                    "unit definition {name:?} would shadow a built-in unit"
                )));
            }
            let mut stack = Vec::new();
            registry.resolve(name, &mut stack)?;
        }
        Ok(registry)
    }

    pub fn names(&self) -> Vec<String> {
        self.definitions.keys().cloned().collect()
    }

    fn resolve(&self, symbol: &str, stack: &mut Vec<String>) -> Result<Dimension> {
        if let Some(definition) = self.definitions.get(symbol) {
            if stack.iter().any(|item| item == symbol) {
                return Err(CoreError::invalid(format!(
                    "unit definition cycle at {symbol:?}"
                )));
            }
            if stack.len() >= 16 {
                return Err(CoreError::invalid("unit definition nesting is too deep"));
            }
            stack.push(symbol.to_string());
            let result = parse_expression(definition, self, stack);
            stack.pop();
            return result;
        }
        builtin_dimension(symbol).ok_or_else(|| {
            CoreError::unsupported(format!(
                "unknown unit symbol {symbol:?}; declare it in unit_definitions or use the built-in SI table"
            ))
        })?
    }
}

pub fn parse(expression: &str) -> Result<Dimension> {
    let registry = Registry::default();
    parse_with(expression, &registry)
}

pub fn parse_with(expression: &str, registry: &Registry) -> Result<Dimension> {
    let text = expression.trim();
    if text.is_empty() {
        return Err(CoreError::invalid("unit expression must not be empty"));
    }
    let mut stack = Vec::new();
    parse_expression(text, registry, &mut stack)
}

fn parse_expression(text: &str, registry: &Registry, stack: &mut Vec<String>) -> Result<Dimension> {
    let mut parser = Parser {
        chars: text.char_indices().peekable(),
        source: text,
        registry,
        stack,
    };
    let dimension = parser.expression(0)?;
    parser.skip_space();
    if parser.chars.peek().is_some() {
        return Err(CoreError::invalid(format!(
            "unit expression {text:?} has trailing input"
        )));
    }
    if !dimension.scale.is_finite() || dimension.scale <= 0.0 {
        return Err(CoreError::invalid(format!(
            "unit expression {text:?} has an invalid scale"
        )));
    }
    Ok(dimension)
}

pub fn equivalent(left: &str, right: &str) -> Result<bool> {
    Ok(parse(left)?.equivalent(&parse(right)?))
}

struct Parser<'a> {
    chars: std::iter::Peekable<std::str::CharIndices<'a>>,
    source: &'a str,
    registry: &'a Registry,
    stack: &'a mut Vec<String>,
}

impl Parser<'_> {
    fn skip_space(&mut self) {
        while matches!(self.chars.peek(), Some((_, ch)) if ch.is_whitespace()) {
            self.chars.next();
        }
    }

    fn peek(&mut self) -> Option<char> {
        self.skip_space();
        self.chars.peek().map(|(_, ch)| *ch)
    }

    fn expression(&mut self, depth: usize) -> Result<Dimension> {
        if depth > 32 {
            return Err(CoreError::invalid("unit expression nesting is too deep"));
        }
        let mut dimension = self.term(depth)?;
        loop {
            match self.peek() {
                Some('*') => {
                    self.chars.next();
                    dimension = dimension.combine(&self.term(depth)?, 1)?;
                }
                Some('/') => {
                    self.chars.next();
                    dimension = dimension.combine(&self.term(depth)?, -1)?;
                }
                _ => return Ok(dimension),
            }
        }
    }

    fn term(&mut self, depth: usize) -> Result<Dimension> {
        let mut dimension = self.atom(depth)?;
        if self.peek() == Some('^') {
            self.chars.next();
            let exponent = self.integer()?;
            if exponent.abs() > 32 {
                return Err(CoreError::invalid("unit exponent magnitude is too large"));
            }
            dimension = dimension.power(exponent)?;
        }
        Ok(dimension)
    }

    fn atom(&mut self, depth: usize) -> Result<Dimension> {
        match self.peek() {
            Some('(') => {
                self.chars.next();
                let dimension = self.expression(depth + 1)?;
                if self.peek() != Some(')') {
                    return Err(CoreError::invalid(format!(
                        "unit expression {:?} has an unmatched '('",
                        self.source
                    )));
                }
                self.chars.next();
                Ok(dimension)
            }
            Some(ch) if ch.is_ascii_digit() || ch == '.' => {
                let number = self.number()?;
                if number <= 0.0 || !number.is_finite() {
                    return Err(CoreError::invalid(
                        "unit scale constants must be positive and finite",
                    ));
                }
                Ok(Dimension {
                    exponents: BTreeMap::new(),
                    scale: number,
                })
            }
            Some(ch) if is_symbol_char(ch) => {
                let symbol = self.symbol()?;
                self.registry.resolve(&symbol, self.stack)
            }
            other => Err(CoreError::invalid(format!(
                "unit expression {:?} has an unexpected token {other:?}",
                self.source
            ))),
        }
    }

    fn integer(&mut self) -> Result<i32> {
        self.skip_space();
        let mut text = String::new();
        if matches!(self.chars.peek(), Some((_, '+' | '-'))) {
            if let Some((_, ch)) = self.chars.next() {
                text.push(ch);
            }
        }
        while let Some((_, ch)) = self.chars.peek().copied() {
            if ch.is_ascii_digit() {
                text.push(ch);
                self.chars.next();
            } else {
                break;
            }
        }
        text.parse::<i32>()
            .map_err(|_| CoreError::invalid(format!("unit exponent {text:?} is not an integer")))
    }

    fn number(&mut self) -> Result<f64> {
        let mut text = String::new();
        while let Some((_, ch)) = self.chars.peek().copied() {
            if ch.is_ascii_digit() || matches!(ch, '.' | 'e' | 'E' | '+' | '-') {
                if matches!(ch, '+' | '-') && !text.ends_with('e') && !text.ends_with('E') {
                    break;
                }
                text.push(ch);
                self.chars.next();
            } else {
                break;
            }
        }
        text.parse::<f64>()
            .map_err(|_| CoreError::invalid(format!("unit scale {text:?} is not a number")))
    }

    fn symbol(&mut self) -> Result<String> {
        let mut symbol = String::new();
        while let Some((_, ch)) = self.chars.peek().copied() {
            if is_symbol_char(ch) {
                symbol.push(ch);
                self.chars.next();
            } else {
                break;
            }
        }
        if symbol.is_empty() {
            return Err(CoreError::invalid("unit expression is missing a symbol"));
        }
        Ok(symbol)
    }
}

impl Dimension {
    fn combine(&self, other: &Dimension, sign: i32) -> Result<Dimension> {
        let mut exponents = self.exponents.clone();
        for (symbol, exponent) in &other.exponents {
            let entry = exponents.entry(symbol).or_insert(0);
            *entry += exponent * sign;
            if *entry == 0 {
                exponents.remove(symbol);
            }
        }
        let scale = if sign > 0 {
            self.scale * other.scale
        } else {
            self.scale / other.scale
        };
        if !scale.is_finite() || scale <= 0.0 {
            return Err(CoreError::invalid("unit expression has an invalid scale"));
        }
        Ok(Dimension { exponents, scale })
    }

    fn power(&self, exponent: i32) -> Result<Dimension> {
        let mut exponents = BTreeMap::new();
        for (symbol, value) in &self.exponents {
            let powered = value
                .checked_mul(exponent)
                .ok_or_else(|| CoreError::invalid("unit exponent overflow"))?;
            if powered != 0 {
                exponents.insert(*symbol, powered);
            }
        }
        let scale = self.scale.powi(exponent);
        if !scale.is_finite() || scale <= 0.0 {
            return Err(CoreError::invalid("unit exponent produced an invalid scale"));
        }
        Ok(Dimension { exponents, scale })
    }
}

fn is_symbol_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '_' | 'µ' | 'μ' | 'Ω' | '%')
}

fn base(pairs: &[(&'static str, i32)], scale: f64) -> Dimension {
    let mut exponents = BTreeMap::new();
    for (symbol, exponent) in pairs {
        if *exponent != 0 {
            exponents.insert(*symbol, *exponent);
        }
    }
    Dimension { exponents, scale }
}

fn builtin_dimension(symbol: &str) -> Option<Result<Dimension>> {
    let scale = |pairs: &[(&'static str, i32)], factor: f64| Some(Ok(base(pairs, factor)));
    let derived = |pairs: &[(&'static str, i32)], factor: f64| Some(Ok(base(pairs, factor)));
    match symbol {
        "1" | "dimensionless" | "rad" | "sr" => scale(&[], 1.0),
        "%" => scale(&[], 0.01),
        "ppm" => scale(&[], 1e-6),
        "m" => scale(&[("m", 1)], 1.0),
        "kg" => scale(&[("kg", 1)], 1.0),
        "s" => scale(&[("s", 1)], 1.0),
        "A" => scale(&[("A", 1)], 1.0),
        "K" => scale(&[("K", 1)], 1.0),
        "mol" => scale(&[("mol", 1)], 1.0),
        "cd" => scale(&[("cd", 1)], 1.0),
        "Hz" => derived(&[("s", -1)], 1.0),
        "N" => derived(&[("kg", 1), ("m", 1), ("s", -2)], 1.0),
        "J" => derived(&[("kg", 1), ("m", 2), ("s", -2)], 1.0),
        "W" => derived(&[("kg", 1), ("m", 2), ("s", -3)], 1.0),
        "Pa" => derived(&[("kg", 1), ("m", -1), ("s", -2)], 1.0),
        "C" => derived(&[("A", 1), ("s", 1)], 1.0),
        "V" => derived(&[("kg", 1), ("m", 2), ("A", -1), ("s", -3)], 1.0),
        "ohm" | "Ω" => derived(&[("kg", 1), ("m", 2), ("A", -2), ("s", -3)], 1.0),
        "S" => derived(&[("kg", -1), ("m", -2), ("A", 2), ("s", 3)], 1.0),
        "F" => derived(&[("kg", -1), ("m", -2), ("A", 2), ("s", 4)], 1.0),
        "H" => derived(&[("kg", 1), ("m", 2), ("A", -2), ("s", -2)], 1.0),
        "T" => derived(&[("kg", 1), ("A", -1), ("s", -2)], 1.0),
        "Wb" => derived(&[("kg", 1), ("m", 2), ("A", -1), ("s", -2)], 1.0),
        "lm" => derived(&[("cd", 1)], 1.0),
        "lx" => derived(&[("cd", 1), ("m", -2)], 1.0),
        "Bq" => derived(&[("s", -1)], 1.0),
        "Gy" | "Sv" => derived(&[("m", 2), ("s", -2)], 1.0),
        "kat" => derived(&[("mol", 1), ("s", -1)], 1.0),
        "km" => scale(&[("m", 1)], 1e3),
        "cm" => scale(&[("m", 1)], 1e-2),
        "mm" => scale(&[("m", 1)], 1e-3),
        "um" | "µm" | "μm" => scale(&[("m", 1)], 1e-6),
        "nm" => scale(&[("m", 1)], 1e-9),
        "ms" => scale(&[("s", 1)], 1e-3),
        "us" | "µs" | "μs" => scale(&[("s", 1)], 1e-6),
        "ns" => scale(&[("s", 1)], 1e-9),
        "mA" => scale(&[("A", 1)], 1e-3),
        "uA" | "µA" | "μA" => scale(&[("A", 1)], 1e-6),
        "nA" => scale(&[("A", 1)], 1e-9),
        "mV" => derived(&[("kg", 1), ("m", 2), ("A", -1), ("s", -3)], 1e-3),
        "kV" => derived(&[("kg", 1), ("m", 2), ("A", -1), ("s", -3)], 1e3),
        "mW" => derived(&[("kg", 1), ("m", 2), ("s", -3)], 1e-3),
        "uW" | "µW" | "μW" => derived(&[("kg", 1), ("m", 2), ("s", -3)], 1e-6),
        "nW" => derived(&[("kg", 1), ("m", 2), ("s", -3)], 1e-9),
        "kHz" => derived(&[("s", -1)], 1e3),
        "MHz" => derived(&[("s", -1)], 1e6),
        "GHz" => derived(&[("s", -1)], 1e9),
        "degC" | "°C" | "dB" | "dBm" | "log10" => Some(Err(CoreError::unsupported(format!(
            "unit {symbol:?} is affine or logarithmic; it has no purely multiplicative dimension"
        )))),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry(pairs: &[(&str, &str)]) -> Registry {
        let definitions: BTreeMap<String, String> = pairs
            .iter()
            .map(|(name, expression)| (name.to_string(), expression.to_string()))
            .collect();
        Registry::new(&definitions).unwrap()
    }

    #[test]
    fn derived_si_units_are_equivalent_to_base_expressions() {
        assert!(equivalent("V", "kg*m^2/(A*s^3)").unwrap());
        assert!(equivalent("N", "kg*m/s^2").unwrap());
        assert!(equivalent("J", "N*m").unwrap());
        assert!(equivalent("W", "J/s").unwrap());
        assert!(equivalent("Hz", "1/s").unwrap());
        assert!(equivalent("rad", "1").unwrap());
        assert!(equivalent("mm", "1e-3*m").unwrap());
        assert!(equivalent("um", "1e-6*m").unwrap());
    }

    #[test]
    fn scale_and_dimension_both_participate() {
        assert!(!equivalent("mm", "m").unwrap());
        assert!(!equivalent("V", "mV").unwrap());
        assert!(!equivalent("V", "A").unwrap());
        assert!(equivalent("m/s", "m*s^-1").unwrap());
    }

    #[test]
    fn affine_and_unknown_units_are_refused() {
        assert!(parse("degC").unwrap_err().to_string().contains("affine"));
        assert!(parse("dB").unwrap_err().to_string().contains("affine"));
        assert!(parse("furlong").unwrap_err().to_string().contains("unknown"));
        assert!(parse("m/").unwrap_err().to_string().contains("unexpected"));
        assert!(parse("(m").unwrap_err().to_string().contains("unmatched"));
        assert!(parse("m^1.5").unwrap_err().to_string().contains("trailing input"));
    }

    #[test]
    fn declared_units_extend_the_table_without_shadowing_builtins() {
        let registry = registry(&[
            ("DN", "1"),
            ("mdeg", "1e-3*rad"),
            ("pixel_pitch", "um"),
        ]);
        let dn = parse_with("DN", &registry).unwrap();
        assert!(dn.is_dimensionless());
        let mdeg = parse_with("mdeg", &registry).unwrap();
        assert!(mdeg.is_dimensionless());
        assert!((mdeg.scale() - 1e-3).abs() < 1e-15);
        let pitch = parse_with("pixel_pitch^2", &registry).unwrap();
        assert!((pitch.scale() - 1e-12).abs() < 1e-20);
        assert!(
            Registry::new(&BTreeMap::from([("m".to_string(), "1e-3*m".to_string())]))
                .unwrap_err()
                .to_string()
                .contains("shadow")
        );
    }

    #[test]
    fn declared_unit_cycles_and_unknown_bases_are_refused() {
        let cycle: BTreeMap<String, String> = BTreeMap::from([
            ("alpha".to_string(), "beta".to_string()),
            ("beta".to_string(), "alpha".to_string()),
        ]);
        assert!(Registry::new(&cycle).unwrap_err().to_string().contains("cycle"));
        let unknown: BTreeMap<String, String> =
            BTreeMap::from([("widget".to_string(), "furlong".to_string())]);
        assert!(Registry::new(&unknown)
            .unwrap_err()
            .to_string()
            .contains("unknown"));
    }
}
