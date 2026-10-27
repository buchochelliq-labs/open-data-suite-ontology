//! OpenDataSuite (ODS) as an ontology source.
//!
//! ODS is the engine that understands a dbt project: it reads every supported
//! artifact format and derives keys and relationships from constraints, data
//! tests and the joins in compiled SQL, each with its evidence. The ontology
//! complements it rather than re-deriving any of that: this source runs
//! `ods erd generate --json`, reads the result with ODS's own `ods-erd` types,
//! and turns it into claims, so concepts and agreed facts can be attached to
//! the assets ODS describes.
//!
//! Identity is shared: an asset's subject is its ODS node id verbatim (the dbt
//! `unique_id`, e.g. `model.jaffle_ods.orders`), and a column is
//! `<node id>#<column>`. Lineage and impact stay in ODS (`ods mcp`); they are
//! deliberately not copied here.
//!
//! ODS's evidence becomes authority, never upgraded:
//!
//! | ODS basis | Authority |
//! |---|---|
//! | `declared` (a constraint) | `declared` |
//! | `tested` (a data test) | `observed` |
//! | `joined` (the project's SQL joins on it) | `observed` |
//! | `inferred` (naming only) | `inferred` |
//!
//! The basis itself is kept as a qualifier so nothing is lost.

use std::path::PathBuf;
use std::process::Command;

use ods_erd::{Basis, Erd};
use onto_core::digest::hex_sha256;
use onto_core::{Authority, Claim, Layer, Page, Provenance, Record, Source, SourceError, SourceInfo, vocab};
use serde::Deserialize;
use serde_json::{Value, json};

/// Envelope major version this source understands (ODS ADR-0003).
pub const ENVELOPE_MAJOR: u32 = 1;

pub const DATA_ASSET: &str = "DataAsset";
pub const ATTRIBUTE: &str = "Attribute";

/// Predicates used for ODS facts; declared in the core pack.
pub mod predicates {
    pub const ASSET_KIND: &str = "asset_kind";
    pub const RELATION: &str = "relation";
    pub const HAS_COLUMN: &str = "has_column";
    pub const DATA_TYPE: &str = "data_type";
    pub const NOT_NULL: &str = "not_null";
    pub const PRIMARY_KEY: &str = "primary_key";
    pub const UNIQUE_KEY: &str = "unique_key";
    pub const REFERENCES: &str = "references";
}

/// The id of a column of an asset.
pub fn column_id(asset: &str, column: &str) -> String {
    format!("{asset}#{column}")
}

/// Map ODS's evidence to authority. Unknown future bases are treated as the
/// weakest evidence, never as stronger.
pub fn authority(basis: Basis) -> Authority {
    match basis {
        Basis::Declared => Authority::Declared,
        Basis::Tested | Basis::Joined => Authority::Observed,
        Basis::Inferred => Authority::Inferred,
        _ => Authority::Inferred,
    }
}

fn basis_text(basis: Basis) -> Value {
    serde_json::to_value(basis).unwrap_or(Value::Null)
}

#[derive(Debug, Deserialize)]
struct EnvelopeVersion {
    major: u32,
}

#[derive(Debug, Deserialize)]
struct Diagnostic {
    #[serde(default)]
    level: String,
    #[serde(default)]
    code: String,
    #[serde(default)]
    message: String,
    #[serde(default)]
    hint: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Envelope {
    schema_version: EnvelopeVersion,
    #[serde(default)]
    ods_version: Option<String>,
    result: Option<ErdResult>,
    #[serde(default)]
    diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Deserialize)]
struct ErdResult {
    erd: Erd,
}

/// A parsed `ods erd generate --json` response.
#[derive(Debug)]
pub struct OdsErd {
    pub ods_version: Option<String>,
    pub erd: Erd,
}

/// Parse an ODS JSON envelope. A failed command (null `result`) returns its
/// error diagnostics as the message.
pub fn parse_envelope(stdout: &str) -> Result<OdsErd, String> {
    let envelope: Envelope = serde_json::from_str(stdout).map_err(|e| format!("not an ODS JSON envelope: {e}"))?;
    if envelope.schema_version.major != ENVELOPE_MAJOR {
        return Err(format!("ODS envelope version {} is not supported (expected {ENVELOPE_MAJOR}.x)", envelope.schema_version.major));
    }
    let Some(result) = envelope.result else {
        let messages: Vec<String> = envelope
            .diagnostics
            .iter()
            .filter(|d| d.level == "error")
            .map(|d| match &d.hint {
                Some(hint) => format!("{} {} ({hint})", d.code, d.message),
                None => format!("{} {}", d.code, d.message),
            })
            .collect();
        return Err(if messages.is_empty() { "ODS returned no result".into() } else { messages.join("; ") });
    };
    if result.erd.schema_version != ods_erd::SCHEMA_VERSION {
        return Err(format!("ODS ERD schema {} is not supported (expected {})", result.erd.schema_version, ods_erd::SCHEMA_VERSION));
    }
    Ok(OdsErd { ods_version: envelope.ods_version, erd: result.erd })
}

/// Turn an ERD into records for `source_id`.
pub fn records(source_id: &str, layer: Layer, parsed: &OdsErd) -> Vec<Record> {
    let version = parsed.ods_version.as_ref().map(|v| format!("ods {v}"));
    let mut out = Vec::new();
    let mut push = |claim: Claim, authority: Authority, evidence: Option<&[String]>| {
        let mut provenance = Provenance::new(source_id, layer, authority);
        provenance.locator = Some(format!("ods:erd.generate#{}", claim.subject));
        provenance.source_version = version.clone();
        if let Some(evidence) = evidence.filter(|e| !e.is_empty()) {
            provenance.rationale = Some(evidence.join("; "));
        }
        out.push(Record { claim, provenance });
    };

    for entity in &parsed.erd.entities {
        let id = &entity.id;
        push(Claim::relation(id, vocab::TYPE, DATA_ASSET), Authority::Observed, None);
        push(Claim::literal(id, vocab::LABEL, entity.name.clone()), Authority::Observed, None);
        push(Claim::literal(id, predicates::ASSET_KIND, serde_json::to_value(entity.kind).unwrap_or(Value::Null)), Authority::Observed, None);
        if let Some(description) = &entity.description {
            push(Claim::literal(id, vocab::DEFINITION, description.clone()), Authority::Declared, None);
        }
        if let Some(relation) = &entity.relation {
            push(Claim::literal(id, predicates::RELATION, relation.clone()), Authority::Observed, None);
        }
        for column in &entity.columns {
            let column_id = column_id(id, &column.name);
            push(Claim::relation(id, predicates::HAS_COLUMN, &column_id), Authority::Observed, None);
            push(Claim::relation(&column_id, vocab::TYPE, ATTRIBUTE), Authority::Observed, None);
            push(Claim::literal(&column_id, vocab::LABEL, column.name.clone()), Authority::Observed, None);
            if let Some(data_type) = &column.data_type {
                push(Claim::literal(&column_id, predicates::DATA_TYPE, data_type.clone()), Authority::Observed, None);
            }
            if let Some(description) = &column.description {
                push(Claim::literal(&column_id, vocab::DEFINITION, description.clone()), Authority::Declared, None);
            }
        }
        let mut key_claim = |predicate: &str, key: &ods_erd::Key| {
            let mut claim = Claim::literal(id, predicate, json!(key.columns));
            claim.qualifiers.insert("basis".into(), basis_text(key.basis));
            push(claim, authority(key.basis), Some(&key.evidence));
        };
        if let Some(key) = &entity.primary_key {
            key_claim(predicates::PRIMARY_KEY, key);
        }
        for key in &entity.unique_keys {
            key_claim(predicates::UNIQUE_KEY, key);
        }
    }

    for r in &parsed.erd.relationships {
        let mut claim = Claim::relation(&r.from, predicates::REFERENCES, &r.to);
        claim.qualifiers.insert("from_columns".into(), json!(r.from_columns));
        claim.qualifiers.insert("to_columns".into(), json!(r.to_columns));
        claim.qualifiers.insert("cardinality".into(), serde_json::to_value(r.cardinality).unwrap_or(Value::Null));
        claim.qualifiers.insert("optional".into(), json!(r.optional));
        claim.qualifiers.insert("basis".into(), basis_text(r.basis));
        push(claim, authority(r.basis), Some(&r.evidence));
    }
    out
}

/// Runs the `ods` CLI against a dbt project.
pub struct OdsSource {
    pub id: String,
    pub layer: Layer,
    /// The `ods` executable and any leading arguments.
    pub command: Vec<String>,
    /// dbt project directory (working directory for `ods`).
    pub project_dir: PathBuf,
    /// dbt target directory; ODS's own default when `None`.
    pub target_dir: Option<PathBuf>,
    /// Also ask ODS for naming-convention keys (recorded as `inferred`).
    pub infer: bool,
}

impl OdsSource {
    fn unavailable(&self, message: impl Into<String>) -> SourceError {
        SourceError::Unavailable { source_id: self.id.clone(), message: message.into() }
    }
}

impl Source for OdsSource {
    fn info(&self) -> SourceInfo {
        SourceInfo { id: self.id.clone(), kind: "ods".into(), layer: self.layer }
    }

    fn fetch(&self, _cursor: Option<&str>) -> Result<Page, SourceError> {
        let (program, leading) = self.command.split_first().ok_or_else(|| self.unavailable("empty ods command"))?;
        let mut command = Command::new(program);
        command.args(leading).args(["erd", "generate", "--all", "--format", "json", "--json"]).current_dir(&self.project_dir);
        if let Some(target) = &self.target_dir {
            command.arg("--target-dir").arg(target);
        }
        if self.infer {
            command.arg("--infer");
        }
        let output =
            command.output().map_err(|e| self.unavailable(format!("cannot run `{program}` (install OpenDataSuite: `pip install opendatasuite`): {e}")))?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        // ODS always writes an envelope in JSON mode, including on failure.
        let parsed = parse_envelope(&stdout).map_err(|message| {
            if output.status.success() { SourceError::Invalid { source_id: self.id.clone(), message } } else { self.unavailable(message) }
        })?;
        Ok(Page { records: records(&self.id, self.layer, &parsed), next_cursor: None, version: Some(format!("sha256:{}", hex_sha256(stdout.as_bytes()))) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ENVELOPE: &str = include_str!("../tests/data/erd-envelope.json");

    fn find<'a>(records: &'a [Record], subject: &str, predicate: &str) -> Vec<&'a Record> {
        records.iter().filter(|r| r.claim.subject == subject && r.claim.predicate == predicate).collect()
    }

    #[test]
    fn translates_assets_columns_keys_and_relationships() {
        let parsed = parse_envelope(ENVELOPE).unwrap();
        assert_eq!(parsed.ods_version.as_deref(), Some("0.0.2"));
        let records = records("dbt", Layer::Repository, &parsed);
        assert!(records.iter().all(|r| r.provenance.source_id == "dbt" && r.provenance.source_version.as_deref() == Some("ods 0.0.2")));

        let orders = "model.jaffle_ods.orders";
        assert_eq!(find(&records, orders, vocab::TYPE)[0].claim.object.entity(), Some(DATA_ASSET));
        assert!(find(&records, orders, predicates::HAS_COLUMN).iter().any(|r| r.claim.object.entity() == Some("model.jaffle_ods.orders#customer_id")));

        let rank_pk = &find(&records, "model.jaffle_ods.customer_order_rank", predicates::PRIMARY_KEY)[0];
        assert_eq!(rank_pk.claim.object, onto_core::Object::Literal(json!(["customer_id", "order_seq"])));
        assert_eq!(rank_pk.provenance.authority, Authority::Declared);
        assert!(rank_pk.provenance.rationale.as_deref().unwrap().contains("constraint primary_key"));

        let tested =
            find(&records, orders, predicates::REFERENCES).into_iter().find(|r| r.claim.object.entity() == Some("model.jaffle_ods.customers")).unwrap();
        assert_eq!(tested.provenance.authority, Authority::Observed, "a tested relationship is observed, not declared");
        assert_eq!(tested.claim.qualifiers["basis"], "tested");
        assert_eq!(tested.claim.qualifiers["cardinality"], "many_to_one");
        assert_eq!(tested.claim.qualifiers["from_columns"], json!(["customer_id"]));
    }

    #[test]
    fn evidence_is_never_upgraded() {
        assert_eq!(authority(Basis::Declared), Authority::Declared);
        assert_eq!(authority(Basis::Tested), Authority::Observed);
        assert_eq!(authority(Basis::Joined), Authority::Observed);
        assert_eq!(authority(Basis::Inferred), Authority::Inferred);
    }

    #[test]
    fn failures_and_unknown_versions_are_reported() {
        let failed = r#"{"schema_version":{"major":1,"minor":0},"command":"erd","ods_version":"0.0.2","result":null,
            "diagnostics":[{"level":"error","code":"ODS-E0201","message":"no manifest.json","hint":"run `dbt parse`"}]}"#;
        let err = parse_envelope(failed).unwrap_err();
        assert!(err.contains("ODS-E0201") && err.contains("dbt parse"), "{err}");

        let future = ENVELOPE.replacen("\"major\": 1", "\"major\": 2", 1);
        assert!(parse_envelope(&future).unwrap_err().contains("not supported"));
        assert!(parse_envelope("not json").is_err());
    }
}
