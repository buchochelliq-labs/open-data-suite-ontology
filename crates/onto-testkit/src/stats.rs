//! Statistics for small evaluations (#56).
//!
//! A run of the evaluation has a few dozen tasks, each answered a few times
//! under every condition. Comparing conditions by their overall pass rates
//! ignores that they answered the same tasks, and normal-approximation error
//! bars are far too narrow at this size. So:
//!
//! - a run's score is the share of its scored criteria that passed, and a
//!   task's score under a condition is the mean over its runs;
//! - a condition's score is the mean over tasks, with a bootstrap interval that
//!   resamples tasks and keeps each task's runs together;
//! - two conditions are compared by their *paired* differences on the tasks
//!   both answered, with a bootstrap interval on the mean difference and the
//!   smallest difference the task set could detect;
//! - reliability is pass^k: the share of tasks every run of which passed every
//!   criterion.
//!
//! Errored runs and unanswered judgements are not scores and are left out, as
//! skipped criteria are. The bootstrap uses a fixed seed, so a report is
//! reproducible.

use std::collections::BTreeMap;

use crate::eval::{Condition, Outcome, RunResult};

/// Resamples per interval: enough for stable 95% percentiles.
const RESAMPLES: usize = 2_000;
/// The bootstrap's seed: the same results always give the same report.
const SEED: u64 = 0x5EED_0F0E_7A15_2026;

/// A small, reproducible random number generator (SplitMix64). The bootstrap
/// needs only uniform indexes, and a fixed, dependency-free generator keeps
/// reports identical across platforms and crate versions.
struct SplitMix64(u64);

impl SplitMix64 {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// A mean and its 95% bootstrap interval, in [0, 1] (or [-1, 1] for differences).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Estimate {
    pub mean: f64,
    pub low: f64,
    pub high: f64,
}

fn mean(values: &[f64]) -> f64 {
    values.iter().sum::<f64>() / values.len() as f64
}

/// The mean of `values` with a percentile bootstrap interval over them.
pub fn bootstrap(values: &[f64]) -> Option<Estimate> {
    if values.is_empty() {
        return None;
    }
    let mut rng = SplitMix64(SEED);
    let mut means: Vec<f64> = (0..RESAMPLES).map(|_| mean(&(0..values.len()).map(|_| values[rng.below(values.len())]).collect::<Vec<_>>())).collect();
    means.sort_by(f64::total_cmp);
    let at = |q: f64| means[((q * (RESAMPLES - 1) as f64).round() as usize).min(RESAMPLES - 1)];
    Some(Estimate { mean: mean(values), low: at(0.025), high: at(0.975) })
}

/// Per task, its runs' scores under each condition: task → condition → scores.
type Scores = BTreeMap<String, BTreeMap<Condition, Vec<RunScore>>>;

/// One run: the share of scored criteria that passed, and whether all did.
#[derive(Clone, Copy, Debug)]
pub struct RunScore {
    pub share: f64,
    pub all_passed: bool,
}

/// Scores of the runs in `results` that `keep` selects.
pub fn scores(results: &[RunResult], keep: impl Fn(&RunResult) -> bool) -> Scores {
    let mut out: Scores = BTreeMap::new();
    for r in results.iter().filter(|r| r.skipped.is_none() && r.error.is_none() && keep(r)) {
        let scored: Vec<&Outcome> = r.criteria.iter().map(|c| &c.outcome).filter(|o| matches!(o, Outcome::Pass | Outcome::Fail)).collect();
        if scored.is_empty() {
            continue;
        }
        let passed = scored.iter().filter(|o| ***o == Outcome::Pass).count();
        let score = RunScore { share: passed as f64 / scored.len() as f64, all_passed: passed == scored.len() };
        out.entry(r.task.clone()).or_default().entry(r.condition).or_default().push(score);
    }
    out
}

fn task_mean(runs: &[RunScore]) -> f64 {
    runs.iter().map(|s| s.share).sum::<f64>() / runs.len() as f64
}

/// One condition's summary over tasks.
#[derive(Clone, Debug)]
pub struct ConditionSummary {
    pub tasks: usize,
    pub score: Estimate,
    /// Share of runs that passed every criterion.
    pub pass_at_1: f64,
    /// Share of tasks every run of which passed every criterion, and the
    /// fewest runs any task had (the k in pass^k).
    pub pass_all: f64,
    pub k: usize,
}

pub fn summarize(scores: &Scores, condition: Condition) -> Option<ConditionSummary> {
    let runs: Vec<&Vec<RunScore>> = scores.values().filter_map(|by| by.get(&condition)).collect();
    let score = bootstrap(&runs.iter().map(|r| task_mean(r)).collect::<Vec<_>>())?;
    let all_runs: Vec<&RunScore> = runs.iter().flat_map(|r| r.iter()).collect();
    Some(ConditionSummary {
        tasks: runs.len(),
        score,
        pass_at_1: all_runs.iter().filter(|s| s.all_passed).count() as f64 / all_runs.len() as f64,
        pass_all: runs.iter().filter(|r| r.iter().all(|s| s.all_passed)).count() as f64 / runs.len() as f64,
        k: runs.iter().map(|r| r.len()).min().unwrap_or(0),
    })
}

/// Condition `a` minus condition `b` on the tasks both answered.
#[derive(Clone, Debug)]
pub struct Paired {
    pub tasks: usize,
    pub difference: Estimate,
    /// The smallest true difference this many tasks would detect with 80%
    /// power at the 5% level, from the spread of the paired differences.
    pub detectable: Option<f64>,
}

pub fn paired(scores: &Scores, a: Condition, b: Condition) -> Option<Paired> {
    let differences: Vec<f64> = scores.values().filter_map(|by| Some(task_mean(by.get(&a)?) - task_mean(by.get(&b)?))).collect();
    let difference = bootstrap(&differences)?;
    let n = differences.len();
    Some(Paired { tasks: n, difference, detectable: detectable(&differences) })
}

/// The smallest true difference `differences.len()` paired tasks would detect
/// with 80% power at the 5% level; none when there are too few tasks or the
/// differences do not vary (the estimate says nothing then).
fn detectable(differences: &[f64]) -> Option<f64> {
    let n = differences.len();
    if n < 2 {
        return None;
    }
    let m = mean(differences);
    let sd = (differences.iter().map(|d| (d - m).powi(2)).sum::<f64>() / (n - 1) as f64).sqrt();
    // z(0.975) + z(0.8) ≈ 1.96 + 0.84.
    (sd > 0.0).then(|| 2.8 * sd / (n as f64).sqrt())
}

/// The same tasks in two evaluation runs, `a` and `b` (say, with a wiki and
/// without it): per condition, the paired difference a − b over the tasks
/// both runs answered.
pub fn compare(a: &[RunResult], b: &[RunResult], labels: (&str, &str)) -> String {
    let (sa, sb) = (scores(a, |_| true), scores(b, |_| true));
    let mut conditions: Vec<Condition> = sa.values().chain(sb.values()).flat_map(|by| by.keys().copied()).collect();
    conditions.sort();
    conditions.dedup();
    let mut out = format!(
        "## {} − {}\n\nPer condition, the paired difference in task scores on the tasks both runs answered, in percentage points.\n\n\
         | Condition | Tasks | {} | {} | Difference [95% interval] | Detectable |\n|---|---|---|---|---|---|\n",
        labels.0, labels.1, labels.0, labels.1
    );
    for condition in conditions {
        let pairs: Vec<(f64, f64)> =
            sa.iter().filter_map(|(task, by)| Some((task_mean(by.get(&condition)?), task_mean(sb.get(task)?.get(&condition)?)))).collect();
        let differences: Vec<f64> = pairs.iter().map(|(x, y)| x - y).collect();
        let Some(d) = bootstrap(&differences) else { continue };
        let side = |f: fn(&(f64, f64)) -> f64| pct(pairs.iter().map(f).sum::<f64>() / pairs.len() as f64);
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} [{}, {}] | {} |\n",
            condition.name(),
            pairs.len(),
            side(|p| p.0),
            side(|p| p.1),
            points(d.mean),
            points(d.low),
            points(d.high),
            detectable(&differences).map(|x| format!("±{:.0}", x * 100.0)).unwrap_or_else(|| "–".into())
        ));
    }
    out
}

fn pct(x: f64) -> String {
    format!("{:.0}%", x * 100.0)
}

fn points(x: f64) -> String {
    format!("{:+.0}", x * 100.0)
}

/// The statistics section of a report for `results`, headed `title`.
pub fn section(results: &[RunResult], title: &str, keep: impl Fn(&RunResult) -> bool) -> Option<String> {
    let scores = scores(results, keep);
    let mut conditions: Vec<Condition> = scores.values().flat_map(|by| by.keys().copied()).collect();
    conditions.sort();
    conditions.dedup();
    let summaries: Vec<(Condition, ConditionSummary)> = conditions.iter().filter_map(|c| Some((*c, summarize(&scores, *c)?))).collect();
    if summaries.is_empty() {
        return None;
    }
    let mut out = format!(
        "\n## {title}\n\nA run's score is the share of its criteria that passed; a task's score is the mean over its runs; \
         a condition's score is the mean over tasks, with a 95% bootstrap interval over tasks. pass^k: tasks where every run \
         passed every criterion.\n\n| Condition | Tasks | Score [95% interval] | pass@1 | pass^k |\n|---|---|---|---|---|\n"
    );
    for (condition, s) in &summaries {
        out.push_str(&format!(
            "| {} | {} | {} [{}, {}] | {} | {} (k={}) |\n",
            condition.name(),
            s.tasks,
            pct(s.score.mean),
            pct(s.score.low),
            pct(s.score.high),
            pct(s.pass_at_1),
            pct(s.pass_all),
            s.k
        ));
    }
    let pairs: Vec<(Condition, Condition, Paired)> = conditions
        .iter()
        .enumerate()
        .flat_map(|(i, a)| conditions[i + 1..].iter().map(move |b| (*a, *b)))
        .filter_map(|(a, b)| Some((b, a, paired(&scores, b, a)?)))
        .collect();
    if !pairs.is_empty() {
        out.push_str(
            "\nPaired differences, on the tasks both conditions answered, in percentage points. An interval that includes 0 is \
             not a difference; nor is one smaller than the detectable difference.\n\n\
             | Comparison | Tasks | Difference [95% interval] | Detectable |\n|---|---|---|---|\n",
        );
        for (a, b, p) in pairs {
            out.push_str(&format!(
                "| {} − {} | {} | {} [{}, {}] | {} |\n",
                a.name(),
                b.name(),
                p.tasks,
                points(p.difference.mean),
                points(p.difference.low),
                points(p.difference.high),
                p.detectable.map(|d| format!("±{:.0}", d * 100.0)).unwrap_or_else(|| "–".into())
            ));
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bootstrap_is_reproducible_and_brackets_the_mean() {
        let values = [1.0, 0.0, 1.0, 1.0, 0.5, 0.0, 1.0, 1.0];
        let a = bootstrap(&values).unwrap();
        assert_eq!(a, bootstrap(&values).unwrap());
        assert!(a.low <= a.mean && a.mean <= a.high, "{a:?}");
        assert!((a.mean - 0.6875).abs() < 1e-9);
        // No spread, no interval.
        let flat = bootstrap(&[1.0, 1.0, 1.0]).unwrap();
        assert_eq!((flat.low, flat.high), (1.0, 1.0));
        assert!(bootstrap(&[]).is_none());
    }
}
