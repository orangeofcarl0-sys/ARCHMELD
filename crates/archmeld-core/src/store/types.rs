// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! Serde views of the versioned contract objects used by the evidence import
//! path. Every struct denies unknown fields; schema validity alone is not
//! semantic validity (S19 §3).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CoverageState {
    Complete,
    Partial,
    Unsupported,
    Failed,
    Cancelled,
    Unknown,
}

impl CoverageState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::Partial => "partial",
            Self::Unsupported => "unsupported",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Coverage {
    pub state: CoverageState,
    pub scope: String,
    pub expected_count: Option<u64>,
    pub observed_count: u64,
    pub omitted_count: u64,
    pub unresolved_count: u64,
    pub limitations: Vec<String>,
}

impl Coverage {
    /// S19 §3.5: `complete` requires a known denominator, everything observed,
    /// nothing omitted and nothing unresolved. Completeness is only relative to
    /// the declared scope.
    pub fn is_well_formed_complete(&self) -> bool {
        self.state == CoverageState::Complete
            && self.expected_count == Some(self.observed_count)
            && self.omitted_count == 0
            && self.unresolved_count == 0
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self).expect("coverage serializes")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Precision {
    Bytes,
    LineOnly,
    Candidate,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Anchor {
    pub path: String,
    pub blob_digest: String,
    pub precision: Precision,
    pub byte_start: Option<u64>,
    pub byte_end: Option<u64>,
    #[serde(default)]
    pub line_start: Option<u64>,
    #[serde(default)]
    pub line_end: Option<u64>,
    pub encoding: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BackendRef {
    pub adapter_id: String,
    pub run_id: String,
    pub backend_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FileRole {
    Source,
    Config,
    Generated,
    Protocol,
    Test,
    Other,
}

impl FileRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Source => "source",
            Self::Config => "config",
            Self::Generated => "generated",
            Self::Protocol => "protocol",
            Self::Test => "test",
            Self::Other => "other",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FileOrigin {
    Tracked,
    Untracked,
    Generated,
    External,
}

impl FileOrigin {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tracked => "tracked",
            Self::Untracked => "untracked",
            Self::Generated => "generated",
            Self::External => "external",
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FileEntry {
    pub path: String,
    pub blob_digest: String,
    pub size_bytes: u64,
    pub role: FileRole,
    pub executable: bool,
    pub origin: FileOrigin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityKind {
    File,
    Module,
    Symbol,
    Type,
    Callsite,
    Component,
    Process,
    Thread,
    Task,
    Queue,
    Resource,
    Boundary,
    External,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Entity {
    pub id: String,
    pub project_id: String,
    pub snapshot_id: String,
    pub config_digest: String,
    pub kind: EntityKind,
    pub language: String,
    pub qualified_name: String,
    pub anchors: Vec<Anchor>,
    pub external: bool,
    pub backend_refs: Vec<BackendRef>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EvidenceKind {
    Syntax,
    Semantic,
    Runtime,
    Human,
    Model,
}

impl EvidenceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Syntax => "syntax",
            Self::Semantic => "semantic",
            Self::Runtime => "runtime",
            Self::Human => "human",
            Self::Model => "model",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Basis {
    Syntax,
    Semantic,
    Runtime,
    Human,
    Model,
}

impl Basis {
    pub fn evidence_kind(self) -> EvidenceKind {
        match self {
            Self::Syntax => EvidenceKind::Syntax,
            Self::Semantic => EvidenceKind::Semantic,
            Self::Runtime => EvidenceKind::Runtime,
            Self::Human => EvidenceKind::Human,
            Self::Model => EvidenceKind::Model,
        }
    }

    pub fn as_str(self) -> &'static str {
        self.evidence_kind().as_str()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Freshness {
    Current,
    Historical,
    Stale,
    Unbound,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Producer {
    pub name: String,
    pub version: String,
    pub source_ref: String,
    pub run_id: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Evidence {
    pub id: String,
    pub project_id: String,
    pub snapshot_id: String,
    pub config_digest: String,
    pub kind: EvidenceKind,
    pub producer: Producer,
    pub artifact_digest: String,
    pub locator: String,
    pub source_anchor: Option<Anchor>,
    pub scenario_id: Option<String>,
    pub runtime_run_id: Option<String>,
    pub coverage: Coverage,
    pub freshness: Freshness,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationKind {
    Contains,
    Declares,
    Imports,
    References,
    StaticCallCandidate,
    ResolvedCall,
    ObservedCall,
    Implements,
    Owns,
    Reads,
    Writes,
    Publishes,
    Consumes,
    Awaits,
    Schedules,
    GuardedBy,
    GeneratedFrom,
    CommunicatesWith,
}

impl RelationKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Contains => "contains",
            Self::Declares => "declares",
            Self::Imports => "imports",
            Self::References => "references",
            Self::StaticCallCandidate => "static_call_candidate",
            Self::ResolvedCall => "resolved_call",
            Self::ObservedCall => "observed_call",
            Self::Implements => "implements",
            Self::Owns => "owns",
            Self::Reads => "reads",
            Self::Writes => "writes",
            Self::Publishes => "publishes",
            Self::Consumes => "consumes",
            Self::Awaits => "awaits",
            Self::Schedules => "schedules",
            Self::GuardedBy => "guarded_by",
            Self::GeneratedFrom => "generated_from",
            Self::CommunicatesWith => "communicates_with",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Resolution {
    Resolved,
    Candidate,
    Observed,
    Declared,
    Unresolved,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Relation {
    pub id: String,
    pub project_id: String,
    pub snapshot_id: String,
    pub config_digest: String,
    pub source_id: String,
    pub target_id: String,
    pub kind: RelationKind,
    pub basis: Basis,
    pub evidence_ids: Vec<String>,
    pub conditions: Vec<String>,
    pub resolution: Resolution,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Diagnostic {
    pub code: String,
    pub severity: Severity,
    pub message: String,
    pub entity_id: Option<String>,
    pub path: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceBatch {
    pub schema_version: String,
    pub project_id: String,
    pub snapshot_id: String,
    pub config_digest: String,
    pub run_id: String,
    pub adapter_id: String,
    pub adapter_version: String,
    /// `DecimalInteger`: a decimal string, never a JSON number.
    pub sequence: String,
    #[serde(rename = "final")]
    pub is_final: bool,
    pub coverage: Coverage,
    pub entities: Vec<Entity>,
    pub evidence: Vec<Evidence>,
    pub relations: Vec<Relation>,
    pub diagnostics: Vec<Diagnostic>,
}
