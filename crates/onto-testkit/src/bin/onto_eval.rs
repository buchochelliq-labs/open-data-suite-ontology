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

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Result, bail};
use clap::Parser;
use onto_testkit::Transcript;
use onto_testkit::agent::{ModelConfig, RequestBudget};
use onto_testkit::calibration;
use onto_testkit::claude_code::ClaudeCodeConfig;
use onto_testkit::eval::{Agent, Cast, Condition, RunResult, Task, errors, judge_conversation, render, render_chat, report, run_task};
use onto_testkit::fixture::repo_root;
use onto_testkit::human::PersonHistory;

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
    /// with +digest (the context digest in the system prompt). The default is
    /// the recommended set.
    #[arg(long, value_delimiter = ',', default_values_t = Condition::RECOMMENDED.map(|c| c.name()))]
    conditions: Vec<String>,
    /// Who plays the person: the tasks' scripted replies, or a model given
    /// each task's persona (tasks without a persona stay scripted).
    #[arg(long, value_enum, default_value_t = HumanMode::Scripted)]
    human: HumanMode,
    /// What the simulated person sees on each reply: its opening message and
    /// this many latest messages, the rest summarised as "(n earlier
    /// messages)"; or `full` for the whole conversation.
    #[arg(long, default_value_t = PersonHistory::default())]
    person_window: PersonHistory,
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
    /// Run every selected task on this overlay (`evals/overlays/<name>/`)
    /// instead of its own, or on the bare fixture with `none`: the same tasks
    /// with and without, say, a wiki source, to measure what it adds.
    #[arg(long)]
    overlay: Option<String>,
    /// Re-render the report of an earlier run from its results.json, with no
    /// model calls: to apply a newer report (e.g. its statistics) to old runs.
    #[arg(long)]
    report: Option<PathBuf>,
    /// Compare two earlier runs of the same tasks (results.json each, e.g. with
    /// and without a wiki): per condition, the paired difference first − second.
    #[arg(long, num_args = 2, value_names = ["FIRST", "SECOND"])]
    compare: Option<Vec<PathBuf>>,
    /// Check the judge against hand-labelled answers (#57): Cohen's κ between
    /// the labels and the judge's recorded verdicts, with no model calls. The
    /// directory defaults to `evals/judge-calibration`.
    #[arg(long, num_args = 0..=1, value_name = "DIR")]
    calibrate: Option<Option<PathBuf>>,
    /// With --calibrate: ask the judge model again instead of using its
    /// recorded verdicts.
    #[arg(long, requires = "calibrate")]
    live: bool,
    /// With --calibrate --live: save the new verdicts as the recorded ones.
    #[arg(long, requires = "live")]
    record: bool,
    /// Label a calibration set by hand: each unlabelled item's conversation
    /// and question, without the judge's verdict. Saves after every answer.
    #[arg(long, num_args = 0..=1, value_name = "DIR")]
    label: Option<Option<PathBuf>>,
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

/// The judge model alone, for calibration: as an eval run would configure it.
fn judge_from_env() -> Result<Agent> {
    if std::env::var("ONTO_AGENT").is_ok_and(|a| a == "claude-code") {
        let config = ClaudeCodeConfig::from_env();
        let model = std::env::var("ONTO_JUDGE_MODEL").ok().filter(|m| !m.is_empty()).or(config.model.clone());
        return Ok(Agent::ClaudeCode(ClaudeCodeConfig { model, ..config }));
    }
    let Some(config) = ModelConfig::from_env() else {
        bail!("set ONTO_LLM_BASE_URL (and ONTO_LLM_MODEL, ONTO_LLM_API_KEY, or ONTO_JUDGE_*), or ONTO_AGENT=claude-code")
    };
    Ok(Agent::Api(role_config(&config, "JUDGE")))
}

fn calibration_dir(dir: &Option<PathBuf>) -> PathBuf {
    dir.clone().unwrap_or_else(|| repo_root().join("evals/judge-calibration"))
}

async fn calibrate(dir: &Option<PathBuf>, live: bool, record: bool) -> Result<()> {
    let set = calibration::Set::load(&calibration_dir(dir))?;
    let (verdicts, source) = if live {
        let judge = judge_from_env()?;
        let mut by_conversation: BTreeMap<&str, Vec<(&str, &str)>> = BTreeMap::new();
        for item in &set.items {
            by_conversation.entry(&item.conversation).or_default().push((&item.criterion, &item.question));
        }
        let mut verdicts = Vec::new();
        for (conversation, questions) in by_conversation {
            eprintln!("judging {conversation} ({} questions)", questions.len());
            let transcript = Transcript::from_jsonl(&std::fs::read_to_string(set.conversation_path(conversation))?)?;
            verdicts.extend(judge_conversation(&judge, conversation, &questions, &transcript).await?);
        }
        if record {
            set.save_recorded(&verdicts)?;
        }
        (verdicts, "asked the judge model again")
    } else {
        (set.recorded.clone(), "recorded (recorded.json)")
    };
    let agreement = calibration::agreement(&set.items, &verdicts);
    print!("{}", agreement.report(source));
    agreement.check(calibration::MIN_KAPPA, calibration::MIN_COMPARED).map_err(anyhow::Error::msg)
}

fn label(dir: &Option<PathBuf>) -> Result<()> {
    use std::io::{BufRead, Write};
    let mut set = calibration::Set::load(&calibration_dir(dir))?;
    let todo: Vec<usize> = (0..set.items.len()).filter(|&i| set.items[i].human.is_none()).collect();
    eprintln!("{} of {} items to label. The judge's verdicts are not shown.", todo.len(), set.items.len());
    let stdin = std::io::stdin();
    let mut lines = stdin.lock().lines();
    let mut shown = None;
    for (n, i) in todo.iter().enumerate() {
        let item = set.items[*i].clone();
        if shown.as_deref() != Some(item.conversation.as_str()) {
            let transcript = Transcript::from_jsonl(&std::fs::read_to_string(set.conversation_path(&item.conversation))?)?;
            println!("\n================ {} ================\n{}", item.conversation, render(&transcript));
            shown = Some(item.conversation.clone());
        }
        let answer = loop {
            print!("\n[{}/{}] {}: {}\n(p)ass, (f)ail, (u)nsure, (s)kip, (q)uit > ", n + 1, todo.len(), item.criterion, item.question);
            std::io::stdout().flush()?;
            let Some(line) = lines.next() else { return Ok(()) };
            match line?.trim() {
                "p" => break Some(calibration::Label::Pass),
                "f" => break Some(calibration::Label::Fail),
                "u" => break Some(calibration::Label::Unsure),
                "s" => break None,
                "q" => return Ok(()),
                _ => continue,
            }
        };
        if let Some(label) = answer {
            set.items[*i].human = Some(label);
            set.save_labels()?;
        }
    }
    eprintln!("done; check agreement with `onto-eval --calibrate`");
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    if let Some(dir) = &args.label {
        return label(dir);
    }
    if let Some(dir) = &args.calibrate {
        return calibrate(dir, args.live, args.record).await;
    }
    if let Some(paths) = &args.compare {
        let load = |p: &PathBuf| -> Result<Vec<RunResult>> { Ok(serde_json::from_str(&std::fs::read_to_string(p)?)?) };
        let name = |p: &PathBuf| p.parent().and_then(|d| d.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.display().to_string());
        let (first, second) = (&paths[0], &paths[1]);
        print!("{}", onto_testkit::stats::compare(&load(first)?, &load(second)?, (&name(first), &name(second))));
        return Ok(());
    }
    if let Some(path) = &args.report {
        let results: Vec<RunResult> = serde_json::from_str(&std::fs::read_to_string(path)?)?;
        print!("{}", report(&results));
        return Ok(());
    }
    let mut budget = None;
    let simulated = args.human == HumanMode::Simulated;
    let (agent, judge, person) = if std::env::var("ONTO_AGENT").is_ok_and(|a| a == "claude-code") {
        let config = ClaudeCodeConfig::from_env();
        eprintln!("agent: Claude Code ({}), onto at {}", config.model.as_deref().unwrap_or("default model"), config.onto_bin.display());
        // The judge and the person can run on a smaller model (ONTO_JUDGE_MODEL, ONTO_HUMAN_MODEL).
        let role = |var: &str| ClaudeCodeConfig { model: std::env::var(var).ok().filter(|m| !m.is_empty()).or(config.model.clone()), ..config.clone() };
        let (judge, person) = (role("ONTO_JUDGE_MODEL"), role("ONTO_HUMAN_MODEL"));
        (Agent::ClaudeCode(config), (!args.no_judge).then_some(Agent::ClaudeCode(judge)), simulated.then_some(Agent::ClaudeCode(person)))
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
        eprintln!("person: simulated by {name} from each task's persona, history {}", args.person_window);
    }
    let cast = Cast { agent: &agent, judge: judge.as_ref(), person: person.as_ref(), person_history: args.person_window };
    let conditions: Vec<Condition> = args.conditions.iter().map(|c| c.parse::<Condition>().map_err(anyhow::Error::msg)).collect::<Result<_>>()?;
    let mut tasks = Task::load_dir(&args.evals.unwrap_or_else(|| repo_root().join("evals")))?;
    if let Some(overlay) = &args.overlay {
        for task in &mut tasks {
            task.overlay = (overlay != "none").then(|| overlay.clone());
            if let Some(dir) = task.overlay_dir().filter(|d| !d.is_dir()) {
                bail!("unknown overlay {}", dir.display());
            }
        }
        eprintln!("overlay: {overlay} for every task (overrides the tasks' own)");
    }
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
    let mut total = onto_testkit::agent::TokenUsage::default();
    for r in &results {
        total += r.tokens.agent;
        total += r.tokens.person;
        total += r.tokens.judge;
    }
    eprintln!(
        "tokens used: {} new input, {} cached input, {} output, over {} requests",
        total.input + total.cache_write,
        total.cache_read,
        total.output,
        total.requests
    );
    let mut markdown = report(&results);
    if let Some(overlay) = &args.overlay {
        markdown = markdown.replacen("\n", &format!("\n\nEvery task ran on overlay `{overlay}` instead of its own.\n"), 1);
    }
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
