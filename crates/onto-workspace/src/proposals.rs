//! Agents propose; humans accept.
//!
//! [`prepare`] validates a proposed claim, shows what it would change and stores
//! it as pending in machine-local state. It never touches repository files.
//! [`accept`] is the only path that writes, and it is invoked by a person
//! (`onto proposals accept <digest>`), never by an agent-facing tool.

use std::collections::BTreeMap;

use onto_core::digest::canonical_digest;
use onto_core::{Authority, Claim, Layer, Object, Property, Provenance, Record, Validity};
use onto_store::{ProposalStatus, Store, StoredProposal};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::pack::{ClaimDef, PackFile};
use crate::workspace::{Result, SyncOptions, Workspace, WorkspaceError, now_rfc3339};

pub const ACCEPTED_FILE: &str = "accepted.yaml";
const MAX_TEXT: usize = 4_000;

/// What an agent asks to record.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProposalRequest {
    /// Entity the claim is about, e.g. `concept:customer`.
    pub subject: String,
    /// Predicate, e.g. `onto:definition`, `grain` or a relation name.
    pub predicate: String,
    /// `{"literal": value}` or `{"entity": "id"}`.
    pub object: Object,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub qualifiers: BTreeMap<String, serde_json::Value>,
    #[serde(default, skip_serializing_if = "Validity::is_open")]
    pub valid: Validity,
    /// Why this should be recorded, including the evidence or the human answer it is based on.
    pub rationale: String,
}

impl ProposalRequest {
    pub fn claim(&self) -> Claim {
        Claim {
            subject: self.subject.clone(),
            predicate: self.predicate.clone(),
            object: self.object.clone(),
            qualifiers: self.qualifiers.clone(),
            valid: self.valid.clone(),
        }
    }
}

/// A pending proposal as shown to the agent and the reviewer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Proposal {
    pub digest: String,
    pub status: ProposalStatus,
    pub request: ProposalRequest,
    pub target_source: String,
    /// Repository file the claim is appended to on acceptance.
    pub target_file: String,
    /// The YAML that acceptance would append.
    pub patch: String,
    /// Current resolved state of this predicate, so the reviewer sees what changes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current: Option<Property>,
    /// What happens next. Preparing a proposal is not approval.
    pub next_step: String,
}

fn invalid(message: impl Into<String>) -> WorkspaceError {
    WorkspaceError::Config(message.into())
}

fn validate(request: &ProposalRequest) -> Result<()> {
    if request.subject.trim().is_empty() || request.predicate.trim().is_empty() {
        return Err(invalid("subject and predicate are required"));
    }
    if request.rationale.trim().is_empty() {
        return Err(invalid("a rationale is required"));
    }
    let size = serde_json::to_string(request).map(|s| s.len()).unwrap_or(usize::MAX);
    if size > MAX_TEXT {
        return Err(invalid(format!("proposal is {size} bytes; the limit is {MAX_TEXT}")));
    }
    Ok(())
}

fn claim_def(request: &ProposalRequest, digest: Option<&str>, accepted_by: Option<&str>) -> ClaimDef {
    ClaimDef {
        subject: request.subject.clone(),
        predicate: request.predicate.clone(),
        object: request.object.clone(),
        qualifiers: request.qualifiers.clone(),
        valid: request.valid.clone(),
        authority: Some(Authority::Reviewed),
        rationale: Some(request.rationale.clone()),
        attributed_to: accepted_by.map(str::to_string),
        proposal: digest.map(str::to_string),
        accepted_at: None,
    }
}

/// Validate and store a pending proposal. Idempotent for identical requests.
pub fn prepare(workspace: &Workspace, store: &Store, request: ProposalRequest, as_of: Option<String>) -> Result<Proposal> {
    validate(&request)?;
    let (target, dir) = workspace.proposal_target()?;
    let digest = canonical_digest(&(&request, &target.id));
    let patch = serde_norway::to_string(&PackFile { claims: vec![claim_def(&request, Some(&digest), None)], ..Default::default() })
        .map_err(|e| invalid(e.to_string()))?;

    let records = store.records_about(&request.subject)?;
    let schema = workspace.schema(store)?;
    let current = workspace.resolver(as_of.clone()).resolve_entity(&request.subject, &records, &schema).property(&request.predicate).cloned();

    let stored = store.proposal(&digest)?;
    let status = stored.as_ref().map(|p| p.status).unwrap_or(ProposalStatus::Pending);
    if stored.is_none() {
        store.put_proposal(&StoredProposal {
            digest: digest.clone(),
            status,
            created_at: now_rfc3339(),
            payload: serde_json::to_value(&request).map_err(|e| invalid(e.to_string()))?,
        })?;
    }

    let target_file = dir.join(ACCEPTED_FILE);
    Ok(Proposal {
        digest: digest.clone(),
        status,
        target_source: target.id.clone(),
        target_file: target_file.strip_prefix(&workspace.root).unwrap_or(&target_file).to_string_lossy().replace('\\', "/"),
        patch,
        current,
        next_step: format!("Pending human review. A person must run `onto proposals accept {digest}`; do not describe this as saved."),
        request,
    })
}

/// Pending proposals with their requests.
pub fn pending(store: &Store) -> Result<Vec<(String, ProposalRequest)>> {
    store
        .proposals(Some(ProposalStatus::Pending))?
        .into_iter()
        .map(|p| Ok((p.digest, serde_json::from_value(p.payload).map_err(|e| invalid(e.to_string()))?)))
        .collect()
}

/// Pending proposals about `subject` as `Proposed` records, so resolution can
/// show them next to (never as) values.
pub fn pending_records(store: &Store, subject: &str) -> Result<Vec<Record>> {
    Ok(pending(store)?
        .into_iter()
        .filter(|(_, request)| request.subject == subject)
        .map(|(digest, request)| {
            let mut provenance = Provenance::new("proposal", Layer::Local, Authority::Proposed);
            provenance.locator = Some(format!("proposal:{digest}"));
            provenance.rationale = Some(request.rationale.clone());
            Record { claim: request.claim(), provenance }
        })
        .collect())
}

/// Human acceptance: append the claim to the target pack and resync it.
pub fn accept(workspace: &Workspace, store: &mut Store, digest: &str, accepted_by: &str) -> Result<String> {
    let stored = store.proposal(digest)?.ok_or_else(|| invalid(format!("unknown proposal {digest}")))?;
    if stored.status != ProposalStatus::Pending {
        return Err(invalid(format!("proposal {digest} is {:?}, not pending", stored.status)));
    }
    if accepted_by.trim().is_empty() {
        return Err(invalid("acceptance must name the reviewer"));
    }
    let request: ProposalRequest = serde_json::from_value(stored.payload).map_err(|e| invalid(e.to_string()))?;
    let (target, dir) = workspace.proposal_target()?;
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(ACCEPTED_FILE);
    let mut file: PackFile = if path.exists() {
        serde_norway::from_str(&std::fs::read_to_string(&path)?).map_err(|e| invalid(format!("{}: {e}", path.display())))?
    } else {
        PackFile::default()
    };
    let mut def = claim_def(&request, Some(digest), Some(accepted_by));
    def.accepted_at = Some(now_rfc3339());
    file.claims.push(def);
    let body = serde_norway::to_string(&file).map_err(|e| invalid(e.to_string()))?;
    std::fs::write(&path, format!("# Accepted proposals. Written by `onto proposals accept`; review in version control.\n{body}"))?;

    store.set_proposal_status(digest, ProposalStatus::Accepted)?;
    let target_id = target.id.clone();
    workspace.sync(store, &SyncOptions { locked: false, only: Some(vec![target_id]) })?;
    Ok(path.to_string_lossy().into_owned())
}

pub fn reject(store: &Store, digest: &str) -> Result<()> {
    let stored = store.proposal(digest)?.ok_or_else(|| invalid(format!("unknown proposal {digest}")))?;
    if stored.status != ProposalStatus::Pending {
        return Err(invalid(format!("proposal {digest} is {:?}, not pending", stored.status)));
    }
    Ok(store.set_proposal_status(digest, ProposalStatus::Rejected)?)
}
