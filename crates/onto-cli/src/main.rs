use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use onto_workspace::proposals;
use onto_workspace::{SyncOptions, SyncOutcome, Workspace, now_rfc3339};

#[derive(Parser)]
#[command(name = "onto", version, about = "Layered, provenance-preserving ontology for agents")]
struct Cli {
    /// Workspace root containing onto.toml.
    #[arg(long, global = true, default_value = ".")]
    root: PathBuf,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create onto.toml and an empty .ontology/ pack.
    Init {
        /// Git URL of a central pack repository.
        #[arg(long)]
        central: Option<String>,
        /// Pack directory inside the central repository.
        #[arg(long, default_value = "packs/core")]
        central_path: String,
    },
    /// Fetch every configured source into the local store.
    Sync {
        /// Use the commits in onto.lock and fail if a git source is unlocked (for CI).
        #[arg(long)]
        locked: bool,
        /// Only sync these source ids.
        #[arg(long = "source")]
        only: Vec<String>,
    },
    /// Show synced sources and pending proposals.
    Status,
    /// Search entities.
    Search {
        query: String,
        #[arg(long, default_value_t = 10)]
        limit: usize,
    },
    /// Show the resolved view of an entity, with provenance and conflicts.
    Show {
        id: String,
        #[arg(long)]
        as_of: Option<String>,
    },
    /// Show relationships around an entity.
    Neighbors {
        id: String,
        #[arg(long, default_value_t = 1)]
        depth: usize,
        #[arg(long, default_value_t = 50)]
        max_edges: usize,
    },
    /// Print a compact Markdown digest of the team's agreed context for agents,
    /// or write it to a file (for example a skill reference or AGENTS.md).
    Context {
        /// Longest digest in characters (about four per token).
        #[arg(long, default_value_t = 16_000)]
        max_chars: usize,
        /// Write here instead of standard output.
        #[arg(long)]
        out: Option<std::path::PathBuf>,
    },
    /// Review proposals made by agents.
    Proposals {
        #[command(subcommand)]
        action: ProposalAction,
    },
    /// Allow a source to run the command onto.toml gives it, after you review it.
    Trust {
        /// Source id from onto.toml.
        source: String,
    },
    /// Serve the MCP server over stdio.
    Mcp,
}

#[derive(Subcommand)]
enum ProposalAction {
    /// List pending proposals.
    List,
    /// Accept a proposal: append it to the repository pack for you to commit.
    Accept {
        digest: String,
        /// Reviewer name recorded with the claim (defaults to git user.name).
        #[arg(long)]
        by: Option<String>,
    },
    /// Reject a proposal.
    Reject { digest: String },
}

fn reviewer(by: Option<String>) -> Result<String> {
    if let Some(by) = by {
        return Ok(by);
    }
    let output = std::process::Command::new("git").args(["config", "user.name"]).output();
    match output {
        Ok(o) if o.status.success() && !o.stdout.trim_ascii().is_empty() => Ok(String::from_utf8_lossy(o.stdout.trim_ascii()).into_owned()),
        _ => bail!("cannot determine reviewer; pass --by <name>"),
    }
}

fn init(root: &std::path::Path, central: Option<String>, central_path: String) -> Result<()> {
    let config = root.join("onto.toml");
    if config.exists() {
        bail!("{} already exists", config.display());
    }
    let mut text = String::from("[resolution]\nmode = \"priority\"\n");
    if let Some(url) = central {
        text.push_str("priority = [\"repo\", \"central\"]\n\n[[source]]\nid = \"central\"\nlayer = \"central\"\nkind = \"git\"\n");
        // JSON string escapes are valid TOML basic strings.
        text.push_str(&format!("url = {}\npath = {}\n", serde_json::to_string(&url)?, serde_json::to_string(&central_path)?));
    } else {
        text.push_str("priority = [\"repo\"]\n");
    }
    text.push_str("\n[[source]]\nid = \"repo\"\nlayer = \"repository\"\nkind = \"pack\"\npath = \".ontology\"\n");
    std::fs::create_dir_all(root.join(".ontology"))?;
    std::fs::write(&config, text)?;
    let ignored = onto_workspace::local::ignore_state_dir(root)?;
    println!("Created {} and .ontology/.{} Commit them, then run `onto sync`.", config.display(), if ignored { " Added .onto/ to .gitignore." } else { "" });
    println!("Each person runs `onto sync` on their own machine; synced facts stay in .onto/ and are never committed.");
    Ok(())
}

fn print_json(value: &impl serde::Serialize) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

async fn run(cli: Cli) -> Result<ExitCode> {
    if let Command::Init { central, central_path } = cli.command {
        init(&cli.root, central, central_path)?;
        return Ok(ExitCode::SUCCESS);
    }
    let workspace = Workspace::load(&cli.root).context("loading workspace")?;
    if let Command::Mcp = cli.command {
        onto_mcp::serve_stdio(workspace).await?;
        return Ok(ExitCode::SUCCESS);
    }
    if onto_workspace::local::state_dir_ignored(&workspace.root) == Some(false) {
        eprintln!("warning: .onto/ is not git-ignored. It holds facts synced with your own access; add `.onto/` to .gitignore so they are never committed.");
    }
    let mut store = workspace.open_store()?;
    match cli.command {
        Command::Sync { locked, only } => {
            let reports = workspace.sync(&mut store, &SyncOptions { locked, only: (!only.is_empty()).then_some(only) })?;
            let mut failed = false;
            for report in reports {
                match report.outcome {
                    SyncOutcome::Synced { version, records } => {
                        println!("synced  {:<16} {records:>6} records  {}", report.source_id, version.unwrap_or_default())
                    }
                    SyncOutcome::Failed { error } => {
                        failed = true;
                        eprintln!("FAILED  {:<16} {error} (previous generation kept)", report.source_id);
                    }
                }
            }
            return Ok(if failed { ExitCode::FAILURE } else { ExitCode::SUCCESS });
        }
        Command::Status => {
            for s in store.sources()? {
                println!(
                    "{:<16} {:<10} {:<8} {:>6} records  synced {}  {}",
                    s.source_id,
                    format!("{:?}", s.layer).to_lowercase(),
                    s.kind,
                    s.record_count,
                    s.synced_at,
                    s.version.unwrap_or_default()
                );
            }
            for id in &workspace.disabled {
                println!("{id:<16} disabled on this machine (.onto/local.toml)");
            }
            println!("pending proposals: {}", proposals::pending(&store)?.len());
            println!("External facts reflect this machine's last sync; teammates sync independently.");
        }
        Command::Search { query, limit } => {
            for hit in store.search(&query, limit)? {
                println!("{:<40} {:.2}", hit.id, hit.score);
            }
        }
        Command::Show { id, as_of } => {
            let mut records = store.records_about(&id)?;
            if records.is_empty() {
                bail!("no records about `{id}`");
            }
            records.extend(proposals::pending_records(&store, &id)?);
            let view = workspace.resolver(Some(as_of.unwrap_or_else(now_rfc3339))).resolve_entity(&id, &records, &workspace.schema(&store)?);
            print_json(&view)?;
        }
        Command::Context { max_chars, out } => {
            let digest = onto_workspace::digest::render(&workspace, &store, &onto_workspace::digest::DigestOptions { max_chars })?;
            match out {
                Some(path) => {
                    std::fs::write(&path, &digest)?;
                    println!("Wrote {} characters of context to {}", digest.len(), path.display());
                }
                None => print!("{digest}"),
            }
        }
        Command::Neighbors { id, depth, max_edges } => print_json(&store.neighbors(&id, depth, max_edges)?)?,
        Command::Trust { source } => match workspace.trust(&source)? {
            Some(command) => println!("Trusted `{source}` to run {command:?} on this machine. A change to this command in onto.toml needs trusting again."),
            None => println!("`{source}` runs no command from onto.toml; nothing to trust."),
        },
        Command::Proposals { action } => match action {
            ProposalAction::List => {
                for (digest, request) in proposals::pending(&store)? {
                    println!(
                        "{digest}\n  {} {} {}\n  rationale: {}",
                        request.subject,
                        request.predicate,
                        serde_json::to_string(&request.object)?,
                        request.rationale
                    );
                }
            }
            ProposalAction::Accept { digest, by } => {
                let path = proposals::accept(&workspace, &mut store, &digest, &reviewer(by)?)?;
                println!("Accepted. Review and commit {path}.");
            }
            ProposalAction::Reject { digest } => {
                proposals::reject(&store, &digest)?;
                println!("Rejected {digest}.");
            }
        },
        Command::Init { .. } | Command::Mcp => unreachable!("handled above"),
    }
    Ok(ExitCode::SUCCESS)
}

#[tokio::main]
async fn main() -> ExitCode {
    match run(Cli::parse()).await {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}
