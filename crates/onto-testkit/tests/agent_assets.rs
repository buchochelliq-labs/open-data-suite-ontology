//! Skills and agent files must describe the tools the server actually exposes.

use std::collections::BTreeSet;

use onto_testkit::fixture::repo_root;
use onto_testkit::{Fixture, Session};

fn mentioned_tools(text: &str) -> BTreeSet<String> {
    text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|w| w.starts_with("ontology_") && w.len() > "ontology_".len())
        .map(str::to_string)
        .collect()
}

#[tokio::test]
async fn skills_and_agents_match_the_server() {
    let fixture = Fixture::new("shop").unwrap();
    let session = Session::start(&fixture).await.unwrap();
    let tools: BTreeSet<String> = session.tools().await.unwrap().into_iter().map(|t| t.name.to_string()).collect();
    let instructions = session.instructions().expect("server instructions");
    session.close().await.unwrap();

    let skill = std::fs::read_to_string(repo_root().join("skills/ontology-grounding/SKILL.md")).unwrap();
    let agent = std::fs::read_to_string(repo_root().join("agents/ontology-modelling.agent.md")).unwrap();
    assert!(skill.starts_with("---\nname: ontology-grounding\n"), "skill front matter");

    for (name, text) in [("SKILL.md", &skill), ("agent", &agent), ("server instructions", &instructions)] {
        let unknown: Vec<_> = mentioned_tools(text).difference(&tools).cloned().collect();
        assert!(unknown.is_empty(), "{name} mentions tools the server lacks: {unknown:?}");
    }
    let undocumented: Vec<_> = tools.difference(&mentioned_tools(&skill)).cloned().collect();
    assert!(undocumented.is_empty(), "SKILL.md does not cover: {undocumented:?}");
}

/// OpenDataSuite's MCP tools at the pinned release (ODS ADR-0010, v0.0.2).
const ODS_TOOLS: &[&str] = &[
    "ods_project_summary",
    "ods_search",
    "ods_get_node",
    "ods_lineage",
    "ods_impact",
    "ods_erd",
    "ods_test_gaps",
    "ods_list_opaque",
    "ods_state_policies",
    "ods_compare_observed",
    "ods_find_data",
    "ods_describe_entity",
    "ods_plan_query",
];

#[test]
fn ods_tools_named_in_the_skill_exist_in_ods() {
    let skill = std::fs::read_to_string(repo_root().join("skills/ontology-grounding/SKILL.md")).unwrap();
    let named: BTreeSet<&str> = skill.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).filter(|w| w.starts_with("ods_")).collect();
    assert!(!named.is_empty());
    for tool in named {
        assert!(ODS_TOOLS.contains(&tool), "SKILL.md names `{tool}`, which ODS v0.0.2 does not serve");
    }
}

#[tokio::test]
async fn only_propose_is_not_read_only() {
    let fixture = Fixture::new("shop").unwrap();
    let session = Session::start(&fixture).await.unwrap();
    for tool in session.tools().await.unwrap() {
        let read_only = tool.annotations.as_ref().and_then(|a| a.read_only_hint).unwrap_or(false);
        assert_eq!(read_only, tool.name != "ontology_propose", "{}", tool.name);
        assert!(tool.output_schema.is_some(), "{} should declare structured output", tool.name);
    }
    session.close().await.unwrap();
}
