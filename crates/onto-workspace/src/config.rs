use onto_core::{Authority, Layer, ResolutionMode};
use serde::{Deserialize, Serialize};

pub const CONFIG_FILE: &str = "onto.toml";

/// `onto.toml`.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub resolution: ResolutionConfig,
    #[serde(default)]
    pub proposals: ProposalConfig,
    #[serde(default, rename = "source")]
    pub sources: Vec<SourceConfig>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolutionConfig {
    #[serde(default)]
    pub mode: ResolutionMode,
    /// Source ids, highest priority first.
    #[serde(default)]
    pub priority: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProposalConfig {
    /// Pack source that receives accepted proposals. Defaults to the first
    /// repository-layer pack source.
    #[serde(default)]
    pub target: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SourceConfig {
    pub id: String,
    pub layer: Layer,
    #[serde(flatten)]
    pub kind: SourceKind,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SourceKind {
    /// A pack directory, relative to the workspace root.
    Pack { path: String },
    /// A pack inside a git repository. `onto.lock` pins the commit.
    Git {
        url: String,
        #[serde(default = "default_rev")]
        rev: String,
        #[serde(default = "default_path")]
        path: String,
    },
    /// Any provider: a command that prints JSON-lines records on stdout.
    Command {
        command: Vec<String>,
        #[serde(default = "default_command_authority")]
        authority: Authority,
    },
}

impl SourceKind {
    pub fn name(&self) -> &'static str {
        match self {
            SourceKind::Pack { .. } => "pack",
            SourceKind::Git { .. } => "git",
            SourceKind::Command { .. } => "command",
        }
    }
}

fn default_rev() -> String {
    "main".into()
}

fn default_path() -> String {
    ".".into()
}

fn default_command_authority() -> Authority {
    Authority::Observed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_all_source_kinds() {
        let config: Config = toml::from_str(
            r#"
            [resolution]
            mode = "preserve"
            priority = ["repo", "core"]

            [[source]]
            id = "core"
            layer = "central"
            kind = "git"
            url = "https://example.com/ontology.git"
            path = "packs/core"

            [[source]]
            id = "repo"
            layer = "repository"
            kind = "pack"
            path = ".ontology"

            [[source]]
            id = "catalog"
            layer = "repository"
            kind = "command"
            command = ["python", "export.py"]
            "#,
        )
        .unwrap();
        assert_eq!(config.resolution.mode, ResolutionMode::Preserve);
        assert!(matches!(&config.sources[0].kind, SourceKind::Git { rev, .. } if rev == "main"));
        assert!(matches!(config.sources[2].kind, SourceKind::Command { authority: Authority::Observed, .. }));
    }
}
