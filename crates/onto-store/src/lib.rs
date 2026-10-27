//! Local store for synced records.
//!
//! One SQLite file holds every source's current generation, a full-text index
//! over entity text and pending proposals. Following dbt-skills ADR 0001 this is
//! the baseline retrieval engine; graph walks are bounded and report truncation
//! instead of silently returning a partial answer as complete.

use std::collections::{BTreeMap, BTreeSet, HashSet, VecDeque};
use std::path::Path;

use onto_core::{Authority, Claim, Layer, Provenance, Record, vocab};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    Invalid(String),
}

pub type Result<T> = std::result::Result<T, StoreError>;

/// Sync state for one source.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SourceState {
    pub source_id: String,
    pub kind: String,
    pub layer: Layer,
    pub version: Option<String>,
    pub synced_at: String,
    pub record_count: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SearchHit {
    pub id: String,
    pub score: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Edge {
    pub subject: String,
    pub predicate: String,
    pub object: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub qualifiers: BTreeMap<String, serde_json::Value>,
    pub sources: Vec<Provenance>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Completeness {
    /// Every edge within the requested depth was returned.
    CompleteWithinDepth,
    /// The edge budget was exhausted; more edges exist.
    Truncated,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Neighborhood {
    pub root: String,
    pub edges: Vec<Edge>,
    pub completeness: Completeness,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProposalStatus {
    Pending,
    Accepted,
    Rejected,
}

/// A stored proposal. The payload is opaque to the store.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StoredProposal {
    pub digest: String,
    pub status: ProposalStatus,
    pub created_at: String,
    pub payload: serde_json::Value,
}

pub struct Store {
    conn: Connection,
}

/// Bump when a derived table changes shape; see [`Store::init`].
const SCHEMA_VERSION: i64 = 1;

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS sources (
    source_id TEXT PRIMARY KEY,
    kind TEXT NOT NULL,
    layer TEXT NOT NULL,
    version TEXT,
    synced_at TEXT NOT NULL,
    record_count INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS records (
    id INTEGER PRIMARY KEY,
    source_id TEXT NOT NULL REFERENCES sources(source_id) ON DELETE CASCADE,
    claim_id TEXT NOT NULL,
    subject TEXT NOT NULL,
    predicate TEXT NOT NULL,
    object_entity TEXT,
    claim_json TEXT NOT NULL,
    provenance_json TEXT NOT NULL,
    UNIQUE (source_id, claim_id)
);
CREATE INDEX IF NOT EXISTS records_subject ON records(subject);
CREATE INDEX IF NOT EXISTS records_object ON records(object_entity) WHERE object_entity IS NOT NULL;
CREATE INDEX IF NOT EXISTS records_predicate ON records(predicate);
CREATE VIRTUAL TABLE IF NOT EXISTS entity_text USING fts5(entity_id UNINDEXED, id_text, body, tokenize = "porter unicode61");
CREATE TABLE IF NOT EXISTS proposals (
    digest TEXT PRIMARY KEY,
    status TEXT NOT NULL,
    created_at TEXT NOT NULL,
    payload_json TEXT NOT NULL
);
"#;

fn enum_text<T: Serialize>(value: &T) -> Result<String> {
    match serde_json::to_value(value)? {
        serde_json::Value::String(s) => Ok(s),
        other => Err(StoreError::Invalid(format!("expected string enum, got {other}"))),
    }
}

fn enum_parse<T: for<'de> Deserialize<'de>>(text: String) -> Result<T> {
    Ok(serde_json::from_value(serde_json::Value::String(text))?)
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        Self::init(Connection::open(path)?)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(mut conn: Connection) -> Result<Self> {
        conn.execute_batch("PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL;")?;
        let version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        let tx = conn.transaction()?;
        if version < SCHEMA_VERSION {
            // Derived indexes are rebuilt from records; records and proposals are kept.
            tx.execute_batch("DROP TABLE IF EXISTS entity_text;")?;
        }
        tx.execute_batch(SCHEMA)?;
        if version < SCHEMA_VERSION {
            rebuild_text_index(&tx)?;
            tx.execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION};"))?;
        }
        tx.commit()?;
        Ok(Self { conn })
    }

    /// Atomically replace one source's records with a complete new generation.
    pub fn replace_source(&mut self, state: &SourceState, records: &[Record]) -> Result<()> {
        for record in records {
            if record.provenance.source_id != state.source_id || record.provenance.layer != state.layer {
                return Err(StoreError::Invalid(format!(
                    "record attributed to `{}`/{:?} cannot be stored for source `{}`/{:?}",
                    record.provenance.source_id, record.provenance.layer, state.source_id, state.layer
                )));
            }
        }
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM sources WHERE source_id = ?1", [&state.source_id])?;
        tx.execute(
            "INSERT INTO sources (source_id, kind, layer, version, synced_at, record_count) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![state.source_id, state.kind, enum_text(&state.layer)?, state.version, state.synced_at, records.len() as i64],
        )?;
        {
            let mut insert = tx.prepare(
                "INSERT OR IGNORE INTO records (source_id, claim_id, subject, predicate, object_entity, claim_json, provenance_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )?;
            for r in records {
                insert.execute(params![
                    state.source_id,
                    r.claim.id(),
                    r.claim.subject,
                    r.claim.predicate,
                    r.claim.object.entity(),
                    serde_json::to_string(&r.claim)?,
                    serde_json::to_string(&r.provenance)?,
                ])?;
            }
        }
        rebuild_text_index(&tx)?;
        tx.commit()?;
        Ok(())
    }

    pub fn remove_source(&mut self, source_id: &str) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM sources WHERE source_id = ?1", [source_id])?;
        rebuild_text_index(&tx)?;
        tx.commit()?;
        Ok(())
    }

    pub fn sources(&self) -> Result<Vec<SourceState>> {
        let mut stmt = self.conn.prepare("SELECT source_id, kind, layer, version, synced_at, record_count FROM sources ORDER BY source_id")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?, row.get(3)?, row.get(4)?, row.get::<_, i64>(5)?))
        })?;
        rows.map(|row| {
            let (source_id, kind, layer, version, synced_at, count) = row?;
            Ok(SourceState { source_id, kind, layer: enum_parse(layer)?, version, synced_at, record_count: count as usize })
        })
        .collect()
    }

    fn query_records(&self, sql: &str, param: &dyn rusqlite::ToSql) -> Result<Vec<Record>> {
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map([param], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))?;
        rows.map(|row| {
            let (claim, provenance) = row?;
            Ok(Record { claim: serde_json::from_str(&claim)?, provenance: serde_json::from_str(&provenance)? })
        })
        .collect()
    }

    /// Every record whose subject is `id`.
    pub fn records_about(&self, id: &str) -> Result<Vec<Record>> {
        self.query_records("SELECT claim_json, provenance_json FROM records WHERE subject = ?1 ORDER BY id", &id)
    }

    /// Records using `predicate`, e.g. to build a schema.
    pub fn records_with_predicate(&self, predicate: &str) -> Result<Vec<Record>> {
        self.query_records("SELECT claim_json, provenance_json FROM records WHERE predicate = ?1 ORDER BY id", &predicate)
    }

    /// Every subject with at least one record, in order.
    pub fn subjects(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare("SELECT DISTINCT subject FROM records ORDER BY subject")?;
        let rows = stmt.query_map([], |row| row.get(0))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn entity_exists(&self, id: &str) -> Result<bool> {
        Ok(self.conn.query_row("SELECT EXISTS (SELECT 1 FROM records WHERE subject = ?1)", [id], |row| row.get(0))?)
    }

    /// Ranked lexical search over entity ids, labels, aliases and definitions.
    /// All terms must match; if nothing does, any term may match.
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>> {
        let terms: Vec<String> = query.split(|c: char| !c.is_alphanumeric()).filter(|t| !t.is_empty()).map(|t| format!("\"{}\"*", t.to_lowercase())).collect();
        if terms.is_empty() {
            return Ok(Vec::new());
        }
        let mut hits = self.fts(&terms.join(" AND "), limit)?;
        if hits.is_empty() && terms.len() > 1 {
            hits = self.fts(&terms.join(" OR "), limit)?;
        }
        Ok(hits)
    }

    fn fts(&self, expression: &str, limit: usize) -> Result<Vec<SearchHit>> {
        // Columns (`<asset id>#<column>`) rank after models and concepts: a model
        // has many columns, which would otherwise crowd the entities themselves out.
        let mut stmt = self.conn.prepare(
            "SELECT entity_id, bm25(entity_text, 4.0, 1.0) AS score FROM entity_text WHERE entity_text MATCH ?1 \
             ORDER BY instr(entity_id, '#') > 0, score, entity_id LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![expression, limit as i64], |row| Ok(SearchHit { id: row.get(0)?, score: -row.get::<_, f64>(1)? }))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Breadth-first walk over entity-valued claims in both directions.
    /// Stops once `max_edges` distinct edges are collected and reports truncation.
    pub fn neighbors(&self, root: &str, depth: usize, max_edges: usize) -> Result<Neighborhood> {
        // Qualifiers are part of an edge's identity: two foreign keys to the same
        // table on different columns are two relationships, not one.
        let mut edges: BTreeMap<(String, String, String, String), Edge> = BTreeMap::new();
        let mut seen: HashSet<String> = HashSet::from([root.to_string()]);
        let mut queue = VecDeque::from([(root.to_string(), 0usize)]);
        let mut completeness = Completeness::CompleteWithinDepth;

        'walk: while let Some((node, level)) = queue.pop_front() {
            if level >= depth {
                continue;
            }
            let records = self.query_records(
                "SELECT claim_json, provenance_json FROM records WHERE object_entity IS NOT NULL AND (subject = ?1 OR object_entity = ?1) ORDER BY id",
                &node,
            )?;
            for record in records {
                if record.provenance.authority == Authority::Proposed {
                    continue;
                }
                let Claim { subject, predicate, object, qualifiers, .. } = record.claim;
                let target = object.entity().expect("filtered to entity objects").to_string();
                let key = (subject.clone(), predicate.clone(), target.clone(), serde_json::to_string(&qualifiers)?);
                if !edges.contains_key(&key) && edges.len() >= max_edges {
                    completeness = Completeness::Truncated;
                    break 'walk;
                }
                let other = if subject == node { target.clone() } else { subject.clone() };
                edges
                    .entry(key)
                    .or_insert_with(|| Edge { subject, predicate, object: target, qualifiers, sources: Vec::new() })
                    .sources
                    .push(record.provenance);
                if seen.insert(other.clone()) {
                    queue.push_back((other, level + 1));
                }
            }
        }
        Ok(Neighborhood { root: root.to_string(), edges: edges.into_values().collect(), completeness })
    }

    /// Every relationship of `node`, in both directions, without a limit, so a
    /// caller can filter before it counts. Edges are keyed by subject,
    /// predicate, object and qualifiers, with every source that asserted them.
    pub fn edges_of(&self, node: &str) -> Result<Vec<Edge>> {
        let records = self.query_records(
            "SELECT claim_json, provenance_json FROM records WHERE object_entity IS NOT NULL AND (subject = ?1 OR object_entity = ?1) ORDER BY id",
            &node,
        )?;
        let mut edges: BTreeMap<(String, String, String, String), Edge> = BTreeMap::new();
        for record in records.into_iter().filter(|r| r.provenance.authority != Authority::Proposed) {
            let Claim { subject, predicate, object, qualifiers, .. } = record.claim;
            let target = object.entity().expect("filtered to entity objects").to_string();
            let key = (subject.clone(), predicate.clone(), target.clone(), serde_json::to_string(&qualifiers)?);
            edges.entry(key).or_insert_with(|| Edge { subject, predicate, object: target, qualifiers, sources: Vec::new() }).sources.push(record.provenance);
        }
        Ok(edges.into_values().collect())
    }

    pub fn put_proposal(&self, proposal: &StoredProposal) -> Result<()> {
        self.conn.execute(
            "INSERT INTO proposals (digest, status, created_at, payload_json) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(digest) DO NOTHING",
            params![proposal.digest, enum_text(&proposal.status)?, proposal.created_at, serde_json::to_string(&proposal.payload)?],
        )?;
        Ok(())
    }

    pub fn proposal(&self, digest: &str) -> Result<Option<StoredProposal>> {
        self.conn
            .query_row("SELECT digest, status, created_at, payload_json FROM proposals WHERE digest = ?1", [digest], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?, row.get::<_, String>(3)?))
            })
            .optional()?
            .map(|(digest, status, created_at, payload)| {
                Ok(StoredProposal { digest, status: enum_parse(status)?, created_at, payload: serde_json::from_str(&payload)? })
            })
            .transpose()
    }

    pub fn proposals(&self, status: Option<ProposalStatus>) -> Result<Vec<StoredProposal>> {
        let mut stmt = self.conn.prepare("SELECT digest FROM proposals ORDER BY created_at, digest")?;
        let digests: Vec<String> = stmt.query_map([], |row| row.get(0))?.collect::<std::result::Result<_, _>>()?;
        let mut out = Vec::new();
        for digest in digests {
            if let Some(p) = self.proposal(&digest)?
                && status.is_none_or(|s| s == p.status)
            {
                out.push(p);
            }
        }
        Ok(out)
    }

    pub fn set_proposal_status(&self, digest: &str, status: ProposalStatus) -> Result<()> {
        let changed = self.conn.execute("UPDATE proposals SET status = ?2 WHERE digest = ?1", params![digest, enum_text(&status)?])?;
        if changed == 0 {
            return Err(StoreError::Invalid(format!("unknown proposal {digest}")));
        }
        Ok(())
    }
}

/// Rebuild the search index from scratch. Fine at project scale; an
/// incremental index is a later optimisation.
fn rebuild_text_index(tx: &Transaction<'_>) -> Result<()> {
    tx.execute("DELETE FROM entity_text", [])?;
    let mut texts: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    {
        let mut stmt = tx.prepare("SELECT DISTINCT subject FROM records")?;
        for subject in stmt.query_map([], |row| row.get::<_, String>(0))? {
            texts.entry(subject?).or_default();
        }
        let placeholders = vocab::SEARCHABLE.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
        let sql = format!("SELECT subject, claim_json, provenance_json FROM records WHERE predicate IN ({placeholders})");
        let mut stmt = tx.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(vocab::SEARCHABLE.iter()), |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?))
        })?;
        for row in rows {
            let (subject, claim, provenance) = row?;
            let claim: Claim = serde_json::from_str(&claim)?;
            let provenance: Provenance = serde_json::from_str(&provenance)?;
            if provenance.authority == Authority::Proposed {
                continue;
            }
            if let Some(text) = claim.object.as_text() {
                texts.entry(subject).or_default().insert(text.to_string());
            }
        }
    }
    let mut insert = tx.prepare("INSERT INTO entity_text (entity_id, id_text, body) VALUES (?1, ?2, ?3)")?;
    for (id, body) in texts {
        insert.execute(params![id, id, body.into_iter().collect::<Vec<_>>().join("\n")])?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(id: &str) -> SourceState {
        SourceState { source_id: id.into(), kind: "test".into(), layer: Layer::Central, version: Some("v1".into()), synced_at: "now".into(), record_count: 0 }
    }

    fn rec(claim: Claim, source: &str) -> Record {
        Record { claim, provenance: Provenance::new(source, Layer::Central, Authority::Declared) }
    }

    fn sample() -> Store {
        let mut store = Store::open_in_memory().unwrap();
        let records = vec![
            rec(Claim::literal("concept:customer", vocab::LABEL, "Customer"), "core"),
            rec(Claim::literal("concept:customer", vocab::ALIAS, "account holder"), "core"),
            rec(Claim::literal("concept:order_line", vocab::LABEL, "Order line"), "core"),
            rec(Claim::relation("concept:customer", "places", "concept:order"), "core"),
            rec(Claim::relation("concept:order", "contains", "concept:order_line"), "core"),
            rec(Claim::relation("concept:order_line", "references", "concept:product"), "core"),
            rec(Claim::relation("concept:order", "contains", "concept:order"), "core"),
        ];
        store.replace_source(&state("core"), &records).unwrap();
        store
    }

    #[test]
    fn search_matches_ids_labels_and_aliases() {
        let store = sample();
        assert_eq!(store.search("customer", 10).unwrap()[0].id, "concept:customer");
        assert_eq!(store.search("account holder", 10).unwrap()[0].id, "concept:customer");
        assert_eq!(store.search("order_line", 10).unwrap()[0].id, "concept:order_line");
        // Porter stemming matches plurals.
        assert_eq!(store.search("customers", 10).unwrap()[0].id, "concept:customer");
        assert!(store.search("   ", 10).unwrap().is_empty());
        // No result has both terms, so it falls back to either.
        assert!(!store.search("customer zebra", 10).unwrap().is_empty());
    }

    #[test]
    fn search_ranks_models_and_concepts_before_columns() {
        let mut store = Store::open_in_memory().unwrap();
        let mut records = vec![rec(Claim::literal("model.shop.order_events", vocab::LABEL, "order_events"), "dbt")];
        for column in ["order_id", "order_date", "order_status", "order_total"] {
            records.push(rec(Claim::literal(format!("model.shop.orders#{column}"), vocab::LABEL, column), "dbt"));
        }
        store.replace_source(&state("dbt"), &records).unwrap();
        let hits = store.search("order", 3).unwrap();
        assert_eq!(hits[0].id, "model.shop.order_events", "{hits:?}");
        // Columns are still found when nothing else matches.
        assert_eq!(store.search("order_total", 3).unwrap()[0].id, "model.shop.orders#order_total");
    }

    #[test]
    fn replace_is_per_source_and_rejects_impersonation() {
        let mut store = sample();
        store.replace_source(&state("core"), &[rec(Claim::literal("concept:x", vocab::LABEL, "X"), "core")]).unwrap();
        assert!(!store.entity_exists("concept:customer").unwrap());
        assert!(store.replace_source(&state("core"), &[rec(Claim::literal("a", vocab::LABEL, "A"), "other")]).is_err());
    }

    #[test]
    fn neighbors_are_bounded_and_report_truncation() {
        let store = sample();
        let one = store.neighbors("concept:order", 1, 100).unwrap();
        assert_eq!(one.completeness, Completeness::CompleteWithinDepth);
        assert_eq!(one.edges.len(), 3, "customer->order, order->order_line, self loop");

        let two = store.neighbors("concept:customer", 3, 100).unwrap();
        assert!(two.edges.iter().any(|e| e.object == "concept:product"));

        let capped = store.neighbors("concept:customer", 3, 2).unwrap();
        assert_eq!(capped.completeness, Completeness::Truncated);
        assert_eq!(capped.edges.len(), 2);
    }

    #[test]
    fn upgrading_an_old_store_rebuilds_the_search_index() {
        let dir = std::env::temp_dir().join(format!("onto-store-upgrade-{}", std::process::id()));
        let _ = std::fs::remove_file(&dir);
        {
            let conn = Connection::open(&dir).unwrap();
            conn.execute_batch(&SCHEMA.replace(r#", tokenize = "porter unicode61""#, "")).unwrap();
        }
        let mut store = Store::open(&dir).unwrap();
        store.replace_source(&state("core"), &[rec(Claim::literal("concept:customer", vocab::LABEL, "Customer"), "core")]).unwrap();
        drop(store);
        let store = Store::open(&dir).unwrap();
        assert_eq!(store.search("customers", 5).unwrap().len(), 1);
        let _ = std::fs::remove_file(&dir);
    }

    #[test]
    fn edges_with_different_qualifiers_stay_distinct() {
        let mut store = Store::open_in_memory().unwrap();
        let fk = |column: &str| {
            let mut claim = Claim::relation("orders", "references", "customers");
            claim.qualifiers.insert("from_columns".into(), serde_json::json!([column]));
            claim
        };
        store.replace_source(&state("core"), &[rec(fk("billing_customer_id"), "core"), rec(fk("shipping_customer_id"), "core")]).unwrap();
        let around = store.neighbors("orders", 1, 10).unwrap();
        assert_eq!(around.edges.len(), 2, "{:?}", around.edges);
        assert!(around.edges.iter().all(|e| e.sources.len() == 1));
    }

    #[test]
    fn proposals_round_trip() {
        let store = sample();
        let p = StoredProposal { digest: "abc".into(), status: ProposalStatus::Pending, created_at: "t".into(), payload: serde_json::json!({"x": 1}) };
        store.put_proposal(&p).unwrap();
        store.set_proposal_status("abc", ProposalStatus::Accepted).unwrap();
        assert_eq!(store.proposal("abc").unwrap().unwrap().status, ProposalStatus::Accepted);
        assert!(store.proposals(Some(ProposalStatus::Pending)).unwrap().is_empty());
        assert!(store.set_proposal_status("nope", ProposalStatus::Rejected).is_err());
    }
}
