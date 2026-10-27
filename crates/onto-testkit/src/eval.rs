//! Does the ontology make an agent's data-model design better?
//!
//! Each task in `evals/*.yaml` is a modelling question on a fixture, with a
//! scripted person and a rubric. It runs under conditions that differ only in
//! which tools the agent sees:
//!
//! - `ods_only`: OpenDataSuite's tools, the baseline for a dbt project;
//! - `ontology_only`: the ontology's tools;
//! - `both`: the combination this project is for.
//!
//! The system prompt is the same everywhere, plus the instructions of the
//! servers whose tools are visible. Criteria are deterministic checks on the
//! transcript where possible, and model-judged questions where not. Safety
//! invariants are checked on every run. Runs repeat, because models vary.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::agent::{ModelConfig, Reply, ScriptedHuman, complete, run_episode};
use crate::fixture::{Fixture, ods_available, ods_binary};
use crate::invariants;
use crate::session::Session;
use crate::transcript::{Event, Transcript};

pub const SYSTEM_PROMPT: &str = "You are a data-modelling assistant for an analytics project. \
Use the available tools to ground every claim before answering, and say which tool or source a fact came from. \
Ask the user when information is missing or sources disagree. Never invent models, columns or keys. Be concise.";

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Condition {
    OdsOnly,
    OntologyOnly,
    Both,
}

impl Condition {
    pub const ALL: [Condition; 3] = [Condition::OdsOnly, Condition::OntologyOnly, Condition::Both];

    fn prefixes(self) -> &'static [&'static str] {
        match self {
            Condition::OdsOnly => &["ods_"],
            Condition::OntologyOnly => &["ontology_"],
            Condition::Both => &["ods_", "ontology_"],
        }
    }

    fn needs_ods(self) -> bool {
        self != Condition::OntologyOnly
    }

    pub fn name(self) -> &'static str {
        match self {
            Condition::OdsOnly => "ods_only",
            Condition::OntologyOnly => "ontology_only",
            Condition::Both => "both",
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Task {
    pub name: String,
    pub fixture: String,
    /// The person's opening message.
    pub prompt: String,
    /// The person's replies, in order, whenever the agent stops to talk.
    #[serde(default)]
    pub replies: Vec<String>,
    /// Conditions that make sense for this task (default: all).
    #[serde(default)]
    pub conditions: Option<Vec<Condition>>,
    #[serde(default)]
    pub max_turns: Option<usize>,
    pub criteria: Vec<Criterion>,
}

#[derive(Debug, Deserialize)]
pub struct Criterion {
    pub id: String,
    #[serde(flatten)]
    pub check: Check,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Check {
    /// Some agent message contains one of these (case-insensitive).
    AnswerContainsAny(Vec<String>),
    /// No agent message contains any of these.
    AnswerNotContains(Vec<String>),
    /// One of these tools was called successfully.
    CalledAny(Vec<String>),
    /// A successful `ontology_propose` with this predicate (and object text).
    Proposed {
        predicate: String,
        #[serde(default)]
        contains: Option<String>,
    },
    /// A yes/no question for the judge model about the whole transcript.
    Judge(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Pass,
    Fail,
    Skipped,
    /// The judge could not be reached or answered unusably: not a score.
    Error,
}

#[derive(Debug, Serialize)]
pub struct CriterionResult {
    pub id: String,
    pub outcome: Outcome,
    pub detail: String,
}

#[derive(Debug, Serialize)]
pub struct RunResult {
    pub task: String,
    pub condition: Condition,
    pub run: usize,
    /// Set when the run could not happen (e.g. `ods` missing).
    pub skipped: Option<String>,
    /// Set when the model could not be reached or failed mid-episode; the
    /// run's scores then say nothing about the ontology.
    pub error: Option<String>,
    pub criteria: Vec<CriterionResult>,
    pub tool_calls: usize,
    pub failed_tool_calls: usize,
    pub violations: Vec<String>,
    pub warnings: Vec<String>,
    #[serde(skip)]
    pub transcript: Transcript,
}

impl Task {
    pub fn load_dir(dir: &Path) -> Result<Vec<Task>> {
        let mut paths: Vec<_> =
            std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))?.map(|e| e.map(|e| e.path())).collect::<Result<_, _>>()?;
        paths.retain(|p| p.extension().is_some_and(|e| e == "yaml"));
        paths.sort();
        paths.iter().map(|p| serde_norway::from_str(&std::fs::read_to_string(p)?).with_context(|| format!("parsing {}", p.display()))).collect()
    }

    pub fn applies_to(&self, condition: Condition) -> bool {
        self.conditions.as_ref().is_none_or(|c| c.contains(&condition))
    }
}

fn answer(transcript: &Transcript) -> String {
    transcript.agent_texts().collect::<Vec<_>>().join("\n").to_lowercase()
}

fn render(transcript: &Transcript) -> String {
    transcript
        .events
        .iter()
        .map(|event| match event {
            Event::Human { text } => format!("USER: {text}"),
            Event::Agent { text } => format!("ASSISTANT: {text}"),
            Event::ToolCall { tool, arguments, ok, result } => {
                let result: String = result.to_string().chars().take(600).collect();
                format!("TOOL {tool} {arguments} ok={ok}: {result}")
            }
            Event::HumanAction { action, detail } => format!("USER ACTION {action}: {detail}"),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

async fn judge(http: &reqwest::Client, config: &ModelConfig, question: &str, transcript: &Transcript) -> Result<(bool, String)> {
    let messages = [
        json!({ "role": "system", "content": "You grade a conversation between a user and an AI data-modelling assistant. Answer only with JSON: {\"pass\": true|false, \"reason\": \"one sentence\"}." }),
        json!({ "role": "user", "content": format!("Question: {question}\n\nConversation:\n{}", render(transcript)) }),
    ];
    let message = complete(http, config, &messages, &[]).await?;
    let text = message["content"].as_str().unwrap_or_default();
    let start = text.find('{').context("judge reply has no JSON")?;
    let end = text.rfind('}').context("judge reply has no JSON")?;
    let verdict: Value = serde_json::from_str(&text[start..=end]).context("judge reply is not JSON")?;
    Ok((verdict["pass"].as_bool().unwrap_or(false), verdict["reason"].as_str().unwrap_or_default().to_string()))
}

async fn score(criterion: &Criterion, transcript: &Transcript, judge_model: Option<&ModelConfig>, http: &reqwest::Client) -> CriterionResult {
    let text = answer(transcript);
    let (outcome, detail) = match &criterion.check {
        Check::AnswerContainsAny(needles) => match needles.iter().find(|n| text.contains(&n.to_lowercase())) {
            Some(n) => (Outcome::Pass, format!("mentions {n:?}")),
            None => (Outcome::Fail, format!("mentions none of {needles:?}")),
        },
        Check::AnswerNotContains(needles) => match needles.iter().find(|n| text.contains(&n.to_lowercase())) {
            Some(n) => (Outcome::Fail, format!("mentions {n:?}")),
            None => (Outcome::Pass, String::new()),
        },
        Check::CalledAny(tools) => match transcript.tool_calls().find(|(t, _, ok, _)| *ok && tools.iter().any(|x| x == t)) {
            Some((t, ..)) => (Outcome::Pass, format!("called {t}")),
            None => (Outcome::Fail, format!("called none of {tools:?}")),
        },
        Check::Proposed { predicate, contains } => {
            let found = transcript.tool_calls().any(|(t, args, ok, _)| {
                ok && t == "ontology_propose"
                    && args["predicate"] == predicate.as_str()
                    && contains.as_ref().is_none_or(|c| args["object"].to_string().to_lowercase().contains(&c.to_lowercase()))
            });
            if found { (Outcome::Pass, String::new()) } else { (Outcome::Fail, format!("no successful proposal of `{predicate}`")) }
        }
        Check::Judge(question) => match judge_model {
            None => (Outcome::Skipped, "no judge model configured".into()),
            Some(config) => match judge(http, config, question, transcript).await {
                Ok((true, reason)) => (Outcome::Pass, reason),
                Ok((false, reason)) => (Outcome::Fail, reason),
                Err(e) => (Outcome::Error, format!("judge failed: {e:#}")),
            },
        },
    };
    CriterionResult { id: criterion.id.clone(), outcome, detail }
}

/// Run one task once under one condition.
pub async fn run_task(task: &Task, condition: Condition, run: usize, agent: &ModelConfig, judge_model: Option<&ModelConfig>) -> Result<RunResult> {
    let mut result = RunResult {
        task: task.name.clone(),
        condition,
        run,
        skipped: None,
        error: None,
        criteria: Vec::new(),
        tool_calls: 0,
        failed_tool_calls: 0,
        violations: Vec::new(),
        warnings: Vec::new(),
        transcript: Transcript::default(),
    };
    if condition.needs_ods() && !ods_available() {
        result.skipped = Some("OpenDataSuite `ods` is not available".into());
        return Ok(result);
    }
    let fixture = Fixture::new(&task.fixture)?;
    let before = fixture.repository_files()?;
    let mut session = Session::start(&fixture).await?;
    if condition.needs_ods() {
        session.attach_ods(&ods_binary(), &fixture.root, &fixture.root.join("target")).await?;
    }
    session.restrict(condition.prefixes());

    let mut human = ScriptedHuman::new(task.replies.iter().cloned().map(Reply::Say).chain([Reply::End]));
    let mut config = agent.clone();
    if let Some(turns) = task.max_turns {
        config.max_turns = turns;
    }
    let episode = run_episode(&mut session, &config, SYSTEM_PROMPT, &task.prompt, &mut human).await;
    let transcript = session.close().await?;
    match episode {
        Ok(stats) => {
            result.tool_calls = stats.tool_calls;
            result.failed_tool_calls = stats.failed_tool_calls;
        }
        Err(e) => result.error = Some(format!("{e:#}")),
    }

    let report = invariants::check(&transcript, &before, &fixture.repository_files()?);
    result.violations = report.violations;
    result.warnings.extend(report.warnings);
    let http = reqwest::Client::builder().timeout(agent.timeout).build()?;
    for criterion in &task.criteria {
        result.criteria.push(score(criterion, &transcript, judge_model, &http).await);
    }
    result.transcript = transcript;
    Ok(result)
}

/// Pass rates per task, criterion and condition, as a Markdown report.
pub fn report(results: &[RunResult]) -> String {
    let mut conditions: Vec<Condition> = results.iter().map(|r| r.condition).collect();
    conditions.sort();
    conditions.dedup();
    // (task, criterion) -> condition -> (passes, scored)
    let mut table: BTreeMap<(String, String), BTreeMap<Condition, (usize, usize)>> = BTreeMap::new();
    let mut totals: BTreeMap<Condition, (usize, usize)> = BTreeMap::new();
    let mut violations = 0;
    for r in results.iter().filter(|r| r.skipped.is_none()) {
        violations += r.violations.len();
        for c in &r.criteria {
            let cell = table.entry((r.task.clone(), c.id.clone())).or_default().entry(r.condition).or_default();
            let total = totals.entry(r.condition).or_default();
            if c.outcome != Outcome::Skipped {
                cell.1 += 1;
                total.1 += 1;
                if c.outcome == Outcome::Pass {
                    cell.0 += 1;
                    total.0 += 1;
                }
            }
        }
    }
    let cell = |v: Option<&(usize, usize)>| match v {
        Some((p, n)) if *n > 0 => format!("{p}/{n}"),
        _ => "–".into(),
    };
    let mut out = String::from("# Ontology evaluation\n\n");
    out.push_str(&format!(
        "| Task | Criterion | {} |\n|---|---|{}\n",
        conditions.iter().map(|c| c.name()).collect::<Vec<_>>().join(" | "),
        "---|".repeat(conditions.len())
    ));
    for ((task, criterion), row) in &table {
        out.push_str(&format!("| {task} | {criterion} | {} |\n", conditions.iter().map(|c| cell(row.get(c))).collect::<Vec<_>>().join(" | ")));
    }
    out.push_str(&format!("| **All** | | {} |\n", conditions.iter().map(|c| cell(totals.get(c))).collect::<Vec<_>>().join(" | ")));
    let skipped: Vec<String> = results.iter().filter_map(|r| r.skipped.as_ref().map(|s| format!("{} / {}: {s}", r.task, r.condition.name()))).collect();
    out.push_str(&format!("\nInvariant violations: {violations}\n"));
    let errors = errors(results);
    if !errors.is_empty() {
        out.push_str(&format!("\n**Errors ({}): these runs or judgements are not scores.**\n- {}\n", errors.len(), errors.join("\n- ")));
    }
    if !skipped.is_empty() {
        out.push_str(&format!("\nSkipped runs:\n- {}\n", skipped.join("\n- ")));
    }
    out
}

/// Runs whose model failed and judged criteria the judge could not answer.
/// A provider outage must not read as a low score.
pub fn errors(results: &[RunResult]) -> Vec<String> {
    let mut out = Vec::new();
    for r in results {
        let at = format!("{} / {} / run {}", r.task, r.condition.name(), r.run);
        if let Some(error) = &r.error {
            out.push(format!("{at}: {}", error.chars().take(300).collect::<String>()));
        }
        for c in r.criteria.iter().filter(|c| c.outcome == Outcome::Error) {
            out.push(format!("{at}: {}: {}", c.id, c.detail.chars().take(300).collect::<String>()));
        }
    }
    out
}
