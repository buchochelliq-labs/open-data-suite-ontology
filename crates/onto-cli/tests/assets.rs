//! The crate embeds copies of the repository's skills and agent (`assets/`), so the
//! package published to crates.io carries them. Each copy must match its source.

use std::path::Path;

#[test]
fn embedded_assets_match_their_sources() {
    let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let repo = crate_dir.join("../..");
    if !repo.join("skills").is_dir() {
        // Built from the published package: there is no repository to compare with.
        return;
    }
    for (copy, source) in [
        ("assets/skills/ontology-grounding/SKILL.md", "skills/ontology-grounding/SKILL.md"),
        ("assets/skills/steno/SKILL.md", "skills/steno/SKILL.md"),
        ("assets/agents/ontology-modelling.agent.md", ".github/agents/ontology-modelling.agent.md"),
    ] {
        let copy_text = std::fs::read_to_string(crate_dir.join(copy)).unwrap();
        let source_text = std::fs::read_to_string(repo.join(source)).unwrap();
        assert!(copy_text == source_text, "crates/onto-cli/{copy} differs from {source}: copy the source over it (cp {source} crates/onto-cli/{copy})");
    }
}
