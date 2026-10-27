//! Run the evaluation tasks against a model and write a report.
//!
//! ```sh
//! ONTO_LLM_BASE_URL=https://api.example.com/v1 ONTO_LLM_MODEL=… ONTO_LLM_API_KEY=… \
//!   cargo run -p onto-testkit --features llm --bin onto-eval -- --runs 3
//! ```
//! The judge uses `ONTO_JUDGE_BASE_URL` / `ONTO_JUDGE_MODEL` / `ONTO_JUDGE_API_KEY`,
//! falling back to the agent's model. With `--human simulated`, a model plays the
//! person from each task's persona: `ONTO_HUMAN_BASE_URL` / `ONTO_HUMAN_MODEL` /
//! `ONTO_HUMAN_API_KEY`, also falling back to the agent's model. `ods` must be
//! installed (or `ONTO_ODS_BIN`).

use std::path::PathBuf;

use anyhow::{Result, bail};
use clap::Parser;
use onto_testkit::agent::{ModelConfig, RequestBudget};
use onto_testkit::claude_code::ClaudeCodeConfig;
use onto_testkit::eval::{Agent, Cast, Condition, RunResult, Task, errors, render_chat, report, run_task};
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
    /// Conditions to compare: ods_only, ontology_only or both, each optionally
    /// with +digest (the context digest in the system prompt).
    #[arg(long, value_delimiter = ',', default_values_t = ["ods_only".to_string(), "ontology_only".to_string(), "both".to_string()])]
    conditions: Vec<String>,
    /// Who plays the person: the tasks' scripted replies, or a model given
    /// each task's persona (tasks without a persona stay scripted).
    #[arg(long, value_enum, default_value_t = HumanMode::Scripted)]
    human: HumanMode,
    /// Only tasks whose name contains this.
    #[arg(long)]
    task: Option<String>,
    /// Output directory for report.md, results.json and transcripts.
    #[arg(long)]
    out: Option<PathBuf>,
    /// Do not call the judge model; judged criteria are skipped. Saves requests.
    #[arg(long)]
    no_judge: bool,
    /// Stop after this many model requests in total (agent and judge).
    #[arg(long)]
    max_requests: Option<usize>,
}

#[derive(Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
enum HumanMode {
    Scripted,
    Simulated,
}

/// A model for `role` (`JUDGE` or `HUMAN`) from `ONTO_<role>_*`, defaulting to the agent's.
fn role_config(agent: &ModelConfig, role: &str) -> ModelConfig {
    let var = |name: &str| std::env::var(format!("ONTO_{role}_{name}")).ok().filter(|v| !v.is_empty());
    ModelConfig {
        base_url: var("BASE_URL").unwrap_or_else(|| agent.base_url.clone()),
        model: var("MODEL").unwrap_or_else(|| agent.model.clone()),
        api_key: var("API_KEY").or_else(|| agent.api_key.clone()),
        ..agent.clone()
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let mut budget = None;
    let simulated = args.human == HumanMode::Simulated;
    let (agent, judge, person) = if std::env::var("ONTO_AGENT").is_ok_and(|a| a == "claude-code") {
        let config = ClaudeCodeConfig::from_env();
        eprintln!("agent: Claude Code ({}), onto at {}", config.model.as_deref().unwrap_or("default model"), config.onto_bin.display());
        let person = ClaudeCodeConfig { model: std::env::var("ONTO_HUMAN_MODEL").ok().filter(|m| !m.is_empty()).or(config.model.clone()), ..config.clone() };
        (Agent::ClaudeCode(config.clone()), (!args.no_judge).then_some(Agent::ClaudeCode(config)), simulated.then_some(Agent::ClaudeCode(person)))
    } else {
        let Some(mut config) = ModelConfig::from_env() else {
            bail!("set ONTO_LLM_BASE_URL (and ONTO_LLM_MODEL, ONTO_LLM_API_KEY), or ONTO_AGENT=claude-code")
        };
        if let Some(max) = args.max_requests {
            config.budget = Some(RequestBudget::new(max));
        }
        budget = config.budget.clone();
        let judge = (!args.no_judge).then(|| Agent::Api(role_config(&config, "JUDGE")));
        let person = simulated.then(|| Agent::Api(role_config(&config, "HUMAN")));
        (Agent::Api(config), judge, person)
    };
    if let Some(person) = &person {
        let name = match person {
            Agent::Api(c) => c.model.clone(),
            Agent::ClaudeCode(c) => format!("Claude Code ({})", c.model.as_deref().unwrap_or("default model")),
        };
        eprintln!("person: simulated by {name} from each task's persona");
    }
    let cast = Cast { agent: &agent, judge: judge.as_ref(), person: person.as_ref() };
    let conditions: Vec<Condition> = args.conditions.iter().map(|c| c.parse::<Condition>().map_err(anyhow::Error::msg)).collect::<Result<_>>()?;
    let tasks = Task::load_dir(&args.evals.unwrap_or_else(|| repo_root().join("evals")))?;
    let out = args.out.unwrap_or_else(|| repo_root().join("target/eval"));
    std::fs::create_dir_all(out.join("transcripts"))?;

    let plan: Vec<(&Task, Condition, usize)> = tasks
        .iter()
        .filter(|t| args.task.as_ref().is_none_or(|f| t.name.contains(f.as_str())))
        .flat_map(|t| conditions.iter().filter(|c| t.applies_to(**c)).flat_map(move |&c| (1..=args.runs).map(move |run| (t, c, run))))
        .collect();
    eprintln!("{} planned runs", plan.len());

    let mut results = Vec::new();
    let mut stopped: Option<String> = None;
    for (task, condition, run) in plan {
        if let Some(reason) = &stopped {
            results.push(RunResult::not_run(task, condition, run, reason.clone()));
            continue;
        }
        eprintln!("\n=== {} / {} / run {run} ===", task.name, condition.name());
        let result = run_task(task, condition, run, cast).await?;
        let stem = out.join("transcripts").join(format!("{}-{}-{run}", task.name, condition.name()));
        std::fs::write(stem.with_extension("jsonl"), result.transcript.to_jsonl())?;
        std::fs::write(stem.with_extension("md"), render_chat(&format!("{} / {} / run {run}", task.name, condition.name()), &result))?;
        if result.budget_exhausted {
            stopped = Some("not run: request budget exhausted earlier in this run".into());
        }
        results.push(result);
    }
    if let Some(budget) = &budget {
        eprintln!("model requests used: {}", budget.used());
    }
    let markdown = report(&results);
    std::fs::write(out.join("report.md"), &markdown)?;
    std::fs::write(out.join("results.json"), serde_json::to_string_pretty(&results)?)?;
    println!("{markdown}");
    if let Some(reason) = &stopped {
        bail!("stopped early ({reason}); completed runs are in {}", out.join("report.md").display());
    }
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
