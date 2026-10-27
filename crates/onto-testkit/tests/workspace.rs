//! Sync behaviour: lockfile pinning, CI mode, stale-on-error and the command adapter.

use std::process::Command;

use onto_testkit::Fixture;
use onto_workspace::{SyncOptions, SyncOutcome};

fn commit_central(fixture: &Fixture, file: &str, body: &str) {
    std::fs::write(fixture.central.join("packs/core").join(file), body).unwrap();
    for args in [vec!["add", "."], vec!["commit", "-q", "-m", "change"]] {
        let ok =
            Command::new("git").arg("-C").arg(&fixture.central).args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"]).args(args).status().unwrap();
        assert!(ok.success());
    }
}

fn label(fixture: &Fixture, id: &str) -> Option<String> {
    let workspace = fixture.workspace().unwrap();
    let store = workspace.open_store().unwrap();
    let records = store.records_about(id).unwrap();
    workspace.resolver(None).resolve_entity(id, &records, &workspace.schema(&store).unwrap()).text(onto_core::vocab::LABEL).map(str::to_string)
}

#[test]
fn locked_sync_ignores_new_central_commits_until_unlocked() {
    let fixture = Fixture::new("shop").unwrap();
    let lock_before = std::fs::read_to_string(fixture.root.join("onto.lock")).unwrap();
    commit_central(&fixture, "extra.yaml", "entities:\n  - { id: concept:invoice, label: Invoice }\n");

    let workspace = fixture.workspace().unwrap();
    let mut store = workspace.open_store().unwrap();
    workspace.sync(&mut store, &SyncOptions { locked: true, only: None }).unwrap();
    assert_eq!(label(&fixture, "concept:invoice"), None, "locked sync must stay on the pinned commit");
    assert_eq!(std::fs::read_to_string(fixture.root.join("onto.lock")).unwrap(), lock_before, "locked sync never rewrites the lock");

    workspace.sync(&mut store, &SyncOptions::default()).unwrap();
    assert_eq!(label(&fixture, "concept:invoice").as_deref(), Some("Invoice"));
    assert_ne!(std::fs::read_to_string(fixture.root.join("onto.lock")).unwrap(), lock_before);
}

#[test]
fn locked_sync_requires_a_lock_entry() {
    let fixture = Fixture::new("shop").unwrap();
    std::fs::remove_file(fixture.root.join("onto.lock")).unwrap();
    let workspace = fixture.workspace().unwrap();
    let mut store = workspace.open_store().unwrap();
    let reports = workspace.sync(&mut store, &SyncOptions { locked: true, only: None }).unwrap();
    assert!(matches!(&reports[0].outcome, SyncOutcome::Failed { error } if error.contains("onto.lock")));
}

#[test]
fn a_broken_source_keeps_its_previous_generation() {
    let fixture = Fixture::new("shop").unwrap();
    std::fs::write(fixture.root.join(".ontology/broken.yaml"), "entities: [ { id: x, colour: red } ]\n").unwrap();
    let workspace = fixture.workspace().unwrap();
    let mut store = workspace.open_store().unwrap();
    let reports = workspace.sync(&mut store, &SyncOptions::default()).unwrap();
    let repo = reports.iter().find(|r| r.source_id == "repo").unwrap();
    assert!(matches!(&repo.outcome, SyncOutcome::Failed { error } if error.contains("broken.yaml")));
    assert_eq!(label(&fixture, "concept:return").as_deref(), Some("Return"), "previous repo records are still served");
}

#[test]
fn command_sources_cannot_choose_their_identity() {
    let fixture = Fixture::new("shop").unwrap();
    let line = r#"{"claim":{"subject":"table:orders","predicate":"onto:label","object":{"literal":"orders"}},"locator":"catalog://main.shop.orders"}"#;
    let config = std::fs::read_to_string(fixture.root.join("onto.toml")).unwrap()
        + &format!("\n[[source]]\nid = \"catalog\"\nlayer = \"repository\"\nkind = \"command\"\ncommand = [\"sh\", \"-c\", {:?}]\n", format!("echo '{line}'"));
    std::fs::write(fixture.root.join("onto.toml"), config).unwrap();

    let workspace = fixture.workspace().unwrap();
    let mut store = workspace.open_store().unwrap();
    workspace.sync(&mut store, &SyncOptions::default()).unwrap();
    let records = store.records_about("table:orders").unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].provenance.source_id, "catalog");
    assert_eq!(records[0].provenance.authority, onto_core::Authority::Observed);
    assert_eq!(records[0].provenance.locator.as_deref(), Some("catalog://main.shop.orders"));
}

#[test]
fn a_source_disabled_locally_keeps_the_shared_lock_and_config() {
    let fixture = Fixture::new("shop").unwrap();
    let config_before = std::fs::read_to_string(fixture.root.join("onto.toml")).unwrap();
    let lock_before = std::fs::read_to_string(fixture.root.join("onto.lock")).unwrap();
    std::fs::write(fixture.root.join(".onto/local.toml"), "[[source]]\nid = \"central\"\nenabled = false\n").unwrap();

    let workspace = fixture.workspace().unwrap();
    assert_eq!(workspace.disabled, vec!["central".to_string()]);
    assert_eq!(workspace.config.resolution.priority, vec!["repo".to_string()]);
    let mut store = workspace.open_store().unwrap();
    workspace.sync(&mut store, &SyncOptions::default()).unwrap();

    assert!(store.sources().unwrap().iter().all(|s| s.source_id != "central"), "disabled facts are dropped from this machine");
    assert_eq!(std::fs::read_to_string(fixture.root.join("onto.lock")).unwrap(), lock_before, "the shared pin is untouched");
    assert_eq!(std::fs::read_to_string(fixture.root.join("onto.toml")).unwrap(), config_before);
}

#[test]
fn local_overrides_are_checked() {
    let fixture = Fixture::new("shop").unwrap();
    std::fs::write(fixture.root.join(".onto/local.toml"), "[[source]]\nid = \"nope\"\nenabled = false\n").unwrap();
    assert!(fixture.workspace().is_err());
    std::fs::write(fixture.root.join(".onto/local.toml"), "[[source]]\nid = \"repo\"\ncommand = [\"x\"]\n").unwrap();
    assert!(fixture.workspace().err().unwrap().to_string().contains("does not apply"));
}
