// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! Structured errors that map onto `contracts/schemas/error.schema.json`.

use serde_json::{json, Value};

pub type Result<T> = std::result::Result<T, CoreError>;

#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    /// Schema-valid but semantically rejected input, or malformed input.
    #[error("invalid input: {0}")]
    Invalid(String),
    /// Same key with different payload, stale generation, sequence conflict.
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("not found: {0}")]
    NotFound(String),
    /// Capability or encoding that this build does not support.
    #[error("unsupported: {0}")]
    Unsupported(String),
    #[error("storage: {0}")]
    Storage(String),
    /// Deterministic fault injection for crash/rollback tests.
    #[error("injected fault: {0}")]
    Injected(String),
}

impl CoreError {
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::Invalid(message.into())
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self::Conflict(message.into())
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::NotFound(message.into())
    }

    pub fn unsupported(message: impl Into<String>) -> Self {
        Self::Unsupported(message.into())
    }

    /// Contract error code, see S19 §7.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Invalid(_) => "invalid_request",
            Self::Conflict(_) => "scope_conflict",
            Self::NotFound(_) => "not_found",
            Self::Unsupported(_) => "unsupported",
            Self::Storage(_) | Self::Injected(_) => "internal_error",
        }
    }

    pub fn retryable(&self) -> bool {
        matches!(
            self,
            Self::Storage(_) | Self::Injected(_) | Self::Conflict(_)
        )
    }

    /// Build the wire `Error` object. `request_id` must be supplied by the
    /// caller; core never invents one.
    pub fn to_contract_error(&self, request_id: &str) -> Value {
        json!({
            "code": self.code(),
            "message": self.to_string(),
            "request_id": request_id,
            "retryable": self.retryable(),
            "details": {},
            "evidence_refs": [],
        })
    }
}

impl From<rusqlite::Error> for CoreError {
    fn from(value: rusqlite::Error) -> Self {
        Self::Storage(value.to_string())
    }
}

impl From<std::io::Error> for CoreError {
    fn from(value: std::io::Error) -> Self {
        Self::Storage(value.to_string())
    }
}

impl From<serde_json::Error> for CoreError {
    fn from(value: serde_json::Error) -> Self {
        Self::Invalid(format!("json: {value}"))
    }
}
