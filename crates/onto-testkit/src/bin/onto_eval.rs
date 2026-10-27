//! Run the evaluation tasks against a model and write a report.
//!
//! ```sh
//! ONTO_LLM_BASE_URL=https://api.example.com/v1 ONTO_LLM_MODEL=… ONTO_LLM_API_KEY=… \
//!   cargo run -p onto-testkit --features llm --bin onto-eval -- --runs 3
//! ```
//! The judge uses `ONTO_JUDGE_BASE_URL` / `ONTO_JUDGE_MODEL` / `ONTO_JUDGE_API_KEY`,
//! falling back to the agent's model. `ods` must be installed (or `ONTO_ODS_BIN`).

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::Parser;
use onto_testkit::agent::ModelConfig;
use onto_testkit::eval::{Condition, Task, errors, report, run_task};
use onto_testkit::fixture::repo_root;

#[derive(Parser)]
#[command(about = "Evaluate whether the ontology improves an agent's data-model design")]
struct Args {
    /// Directory of task files.
    #[arg(long)]
    evals: Option<PathBuf>,
    /// Repetitions per task and condition.
    #[arg(long, default_value_t = 3)]
    runs: usize,
    /// Conditions to compare.
    #[arg(long, value_delimiter = ',', default_values_t = ["ods_only".to_string(), "ontology_only".to_string(), "both".to_string()])]
    conditions: Vec<String>,
    /// Only tasks whose name contains this.
    #[arg(long)]
    task: Option<String>,
    /// Output directory for report.md, results.json and transcripts.
    #[arg(long)]
    out: Option<PathBuf>,
}

fn judge_config(agent: &ModelConfig) -> ModelConfig {
    let var = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
    ModelConfig {
        base_url: var("ONTO_JUDGE_BASE_URL").unwrap_or_else(|| agent.base_url.clone()),
        model: var("ONTO_JUDGE_MODEL").unwrap_or_else(|| agent.model.clone()),
        api_key: var("ONTO_JUDGE_API_KEY").or_else(|| agent.api_key.clone()),
        ..agent.clone()
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let Some(agent) = ModelConfig::from_env() else { bail!("set ONTO_LLM_BASE_URL (and ONTO_LLM_MODEL, ONTO_LLM_API_KEY)") };
    let judge = judge_config(&agent);
    let conditions: Vec<Condition> = args
        .conditions
        .iter()
        .map(|c| serde_json::from_value(serde_json::Value::String(c.clone())).with_context(|| format!("unknown condition `{c}`")))
        .collect::<Result<_>>()?;
    let tasks = Task::load_dir(&args.evals.unwrap_or_else(|| repo_root().join("evals")))?;
    let out = args.out.unwrap_or_else(|| repo_root().join("target/eval"));
    std::fs::create_dir_all(out.join("transcripts"))?;

    let mut results = Vec::new();
    for task in tasks.iter().filter(|t| args.task.as_ref().is_none_or(|f| t.name.contains(f.as_str()))) {
        for &condition in conditions.iter().filter(|c| task.applies_to(**c)) {
            for run in 1..=args.runs {
                eprintln!("{} / {} / run {run}", task.name, condition.name());
                let result = run_task(task, condition, run, &agent, Some(&judge)).await?;
                let file = out.join("transcripts").join(format!("{}-{}-{run}.jsonl", task.name, condition.name()));
                std::fs::write(file, result.transcript.to_jsonl())?;
                results.push(result);
            }
        }
    }
    let markdown = report(&results);
    std::fs::write(out.join("report.md"), &markdown)?;
    std::fs::write(out.join("results.json"), serde_json::to_string_pretty(&results)?)?;
    println!("{markdown}");
    let errors = errors(&results);
    if !errors.is_empty() {
        bail!("{} error(s) from the model provider; the report is not a valid evaluation. See {}", errors.len(), out.join("report.md").display());
    }
    let violations: usize = results.iter().map(|r| r.violations.len()).sum();
    if violations > 0 {
        bail!("{violations} invariant violation(s); see {}", out.join("results.json").display());
    }
    Ok(())
}
