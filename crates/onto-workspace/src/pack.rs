//! The pack format: a directory of human-editable YAML files.
//!
//! ```yaml
//! predicates:
//!   - id: places
//!     label: places
//!     cardinality: many
//! types:
//!   - id: BusinessConcept
//!     label: Business concept
//! entities:
//!   - id: concept:customer
//!     type: BusinessConcept
//!     label: Customer
//!     definition: A party that buys from us.
//!     aliases: [client]
//!     properties: { grain: one row per person }
//!     relations:
//!       - { predicate: places, target: concept:order, qualifiers: { cardinality: one_to_many } }
//! claims:   # raw escape hatch, also used for accepted proposals
//!   - subject: concept:order
//!     predicate: grain
//!     object: { literal: one row per order }
//! ```
//!
//! An optional `pack.toml` names the pack and sets its default authority.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use onto_core::digest::hex_sha256;
use onto_core::{Authority, Cardinality, Claim, Layer, Object, Page, Provenance, Record, Source, SourceError, SourceInfo, Validity, vocab};
use serde::{Deserialize, Serialize};

pub const MANIFEST: &str = "pack.toml";

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackManifest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    /// Authority for claims that do not set one.
    #[serde(default)]
    pub authority: Option<Authority>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackFile {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub predicates: Vec<PredicateDef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub types: Vec<TypeDef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub entities: Vec<EntityDef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub claims: Vec<ClaimDef>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PredicateDef {
    pub id: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub definition: Option<String>,
    #[serde(default)]
    pub cardinality: Option<Cardinality>,
    /// Entity type of the object, or `literal`.
    #[serde(default)]
    pub range: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TypeDef {
    pub id: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub definition: Option<String>,
    #[serde(default)]
    pub subtype_of: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum OneOrMany {
    One(String),
    Many(Vec<String>),
}

impl OneOrMany {
    fn into_vec(self) -> Vec<String> {
        match self {
            OneOrMany::One(s) => vec![s],
            OneOrMany::Many(v) => v,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EntityDef {
    pub id: String,
    #[serde(rename = "type", default)]
    pub types: Option<OneOrMany>,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub definition: Option<String>,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub properties: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    pub relations: Vec<RelationDef>,
    #[serde(default)]
    pub authority: Option<Authority>,
    #[serde(default)]
    pub rationale: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelationDef {
    pub predicate: String,
    pub target: String,
    #[serde(default)]
    pub qualifiers: BTreeMap<String, serde_json::Value>,
    #[serde(default, skip_serializing_if = "Validity::is_open")]
    pub valid: Validity,
    #[serde(default)]
    pub rationale: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClaimDef {
    pub subject: String,
    pub predicate: String,
    pub object: Object,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub qualifiers: BTreeMap<String, serde_json::Value>,
    #[serde(default, skip_serializing_if = "Validity::is_open")]
    pub valid: Validity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authority: Option<Authority>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rationale: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attributed_to: Option<String>,
    /// Digest of the proposal this claim was accepted from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposal: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accepted_at: Option<String>,
}

/// A pack directory exposed as a [`Source`].
pub struct PackSource {
    pub id: String,
    pub layer: Layer,
    pub kind: String,
    pub dir: PathBuf,
    /// Overrides the content hash as the reported version (e.g. a git commit).
    pub version: Option<String>,
}

impl PackSource {
    pub fn new(id: impl Into<String>, layer: Layer, dir: impl Into<PathBuf>) -> Self {
        Self { id: id.into(), layer, kind: "pack".into(), dir: dir.into(), version: None }
    }

    fn invalid(&self, message: impl Into<String>) -> SourceError {
        SourceError::Invalid { source_id: self.id.clone(), message: message.into() }
    }

    fn default_authority(&self) -> Authority {
        match self.layer {
            Layer::Central => Authority::Declared,
            // Committed repository files went through code review.
            Layer::Repository => Authority::Reviewed,
            Layer::Local => Authority::Inferred,
        }
    }

    /// Parse the pack into records and a content hash over every file read.
    pub fn load(&self) -> Result<(Vec<Record>, String), SourceError> {
        if !self.dir.is_dir() {
            return Err(SourceError::Unavailable { source_id: self.id.clone(), message: format!("pack directory {} does not exist", self.dir.display()) });
        }
        let mut files = Vec::new();
        collect_yaml(&self.dir, &mut files).map_err(|e| self.invalid(e.to_string()))?;
        files.sort();

        let mut hash_input = Vec::new();
        let manifest_path = self.dir.join(MANIFEST);
        let manifest: PackManifest = if manifest_path.is_file() {
            let text = fs::read_to_string(&manifest_path).map_err(|e| self.invalid(e.to_string()))?;
            hash_input.extend_from_slice(text.as_bytes());
            toml::from_str(&text).map_err(|e| self.invalid(format!("{MANIFEST}: {e}")))?
        } else {
            PackManifest::default()
        };
        let default_authority = manifest.authority.unwrap_or_else(|| self.default_authority());

        let mut records = Vec::new();
        for path in files {
            let rel = path.strip_prefix(&self.dir).unwrap_or(&path).to_string_lossy().replace('\\', "/");
            let text = fs::read_to_string(&path).map_err(|e| self.invalid(format!("{rel}: {e}")))?;
            hash_input.extend_from_slice(rel.as_bytes());
            hash_input.extend_from_slice(text.as_bytes());
            let file: PackFile = serde_norway::from_str(&text).map_err(|e| self.invalid(format!("{rel}: {e}")))?;
            self.expand(&rel, file, default_authority, &mut records)?;
        }
        Ok((records, format!("sha256:{}", hex_sha256(&hash_input))))
    }

    fn expand(&self, file: &str, pack: PackFile, default_authority: Authority, out: &mut Vec<Record>) -> Result<(), SourceError> {
        let record = |claim: Claim, anchor: &str, authority: Option<Authority>, rationale: Option<String>| -> Result<Record, SourceError> {
            if claim.subject.trim().is_empty() || claim.predicate.trim().is_empty() {
                return Err(self.invalid(format!("{file}: claim with empty subject or predicate")));
            }
            let mut provenance = Provenance::new(&self.id, self.layer, authority.unwrap_or(default_authority));
            provenance.locator = Some(format!("{file}#{anchor}"));
            provenance.rationale = rationale;
            Ok(Record { claim, provenance })
        };

        for p in pack.predicates {
            let mut claims = vec![Claim::relation(&p.id, vocab::TYPE, vocab::PREDICATE_TYPE)];
            claims.extend(p.label.map(|v| Claim::literal(&p.id, vocab::LABEL, v)));
            claims.extend(p.definition.map(|v| Claim::literal(&p.id, vocab::DEFINITION, v)));
            claims.extend(p.cardinality.map(|c| {
                Claim::literal(
                    &p.id,
                    vocab::CARDINALITY,
                    match c {
                        Cardinality::One => "one",
                        Cardinality::Many => "many",
                    },
                )
            }));
            claims.extend(p.range.map(|v| Claim::literal(&p.id, vocab::RANGE, v)));
            for claim in claims {
                out.push(record(claim, &p.id, None, None)?);
            }
        }

        for t in pack.types {
            let mut claims = vec![Claim::relation(&t.id, vocab::TYPE, vocab::TYPE_TYPE)];
            claims.extend(t.label.map(|v| Claim::literal(&t.id, vocab::LABEL, v)));
            claims.extend(t.definition.map(|v| Claim::literal(&t.id, vocab::DEFINITION, v)));
            claims.extend(t.subtype_of.into_iter().map(|parent| Claim::relation(&t.id, vocab::SUBTYPE_OF, parent)));
            for claim in claims {
                out.push(record(claim, &t.id, None, None)?);
            }
        }

        for e in pack.entities {
            let id = &e.id;
            let mut claims: Vec<Claim> =
                e.types.map(OneOrMany::into_vec).unwrap_or_default().into_iter().map(|t| Claim::relation(id, vocab::TYPE, t)).collect();
            claims.extend(e.label.map(|v| Claim::literal(id, vocab::LABEL, v)));
            claims.extend(e.definition.map(|v| Claim::literal(id, vocab::DEFINITION, v)));
            claims.extend(e.aliases.into_iter().map(|v| Claim::literal(id, vocab::ALIAS, v)));
            claims.extend(e.properties.into_iter().map(|(k, v)| Claim::literal(id, k, v)));
            for claim in claims {
                out.push(record(claim, id, e.authority, e.rationale.clone())?);
            }
            for r in e.relations {
                let mut claim = Claim::relation(id, r.predicate, r.target);
                claim.qualifiers = r.qualifiers;
                claim.valid = r.valid;
                out.push(record(claim, id, e.authority, r.rationale.or_else(|| e.rationale.clone()))?);
            }
        }

        for c in pack.claims {
            let anchor = c.proposal.as_ref().map(|d| format!("proposal:{d}")).unwrap_or_else(|| c.subject.clone());
            let claim = Claim { subject: c.subject, predicate: c.predicate, object: c.object, qualifiers: c.qualifiers, valid: c.valid };
            let mut r = record(claim, &anchor, c.authority, c.rationale)?;
            r.provenance.attributed_to = c.attributed_to;
            out.push(r);
        }
        Ok(())
    }
}

fn collect_yaml(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name.starts_with('.') {
            continue;
        }
        if path.is_dir() {
            collect_yaml(&path, out)?;
        } else if name.ends_with(".yaml") || name.ends_with(".yml") {
            out.push(path);
        }
    }
    Ok(())
}

impl Source for PackSource {
    fn info(&self) -> SourceInfo {
        SourceInfo { id: self.id.clone(), kind: self.kind.clone(), layer: self.layer }
    }

    fn fetch(&self, _cursor: Option<&str>) -> Result<Page, SourceError> {
        let (records, hash) = self.load()?;
        Ok(Page { records, next_cursor: None, version: Some(self.version.clone().unwrap_or(hash)) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_entities_relations_and_schema() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(MANIFEST), "name = \"t\"\nauthority = \"declared\"\n").unwrap();
        fs::create_dir(dir.path().join("sub")).unwrap();
        fs::write(
            dir.path().join("sub/concepts.yaml"),
            r#"
predicates:
  - { id: places, cardinality: many }
entities:
  - id: concept:customer
    type: BusinessConcept
    label: Customer
    aliases: [client]
    properties: { grain: one row per person }
    relations:
      - { predicate: places, target: concept:order, qualifiers: { cardinality: one_to_many } }
claims:
  - { subject: concept:order, predicate: grain, object: { literal: one row per order }, authority: reviewed }
"#,
        )
        .unwrap();
        fs::write(dir.path().join(".hidden.yaml"), "not: [valid").unwrap();

        let source = PackSource::new("core", Layer::Central, dir.path());
        let (records, hash) = source.load().unwrap();
        assert!(hash.starts_with("sha256:"));
        let find = |s: &str, p: &str| records.iter().find(|r| r.claim.subject == s && r.claim.predicate == p).unwrap();
        assert_eq!(find("places", vocab::CARDINALITY).claim.object.as_text(), Some("many"));
        assert_eq!(find("concept:customer", vocab::ALIAS).claim.object.as_text(), Some("client"));
        let rel = find("concept:customer", "places");
        assert_eq!(rel.claim.qualifiers["cardinality"], "one_to_many");
        assert_eq!(rel.provenance.authority, Authority::Declared);
        assert_eq!(rel.provenance.locator.as_deref(), Some("sub/concepts.yaml#concept:customer"));
        assert_eq!(find("concept:order", "grain").provenance.authority, Authority::Reviewed);
    }

    #[test]
    fn rejects_unknown_fields_and_missing_dirs() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.yaml"), "entities:\n  - { id: x, colour: red }\n").unwrap();
        let err = PackSource::new("p", Layer::Repository, dir.path()).load().unwrap_err();
        assert!(err.to_string().contains("a.yaml"), "{err}");
        assert!(matches!(PackSource::new("p", Layer::Repository, dir.path().join("nope")).load(), Err(SourceError::Unavailable { .. })));
    }
}
