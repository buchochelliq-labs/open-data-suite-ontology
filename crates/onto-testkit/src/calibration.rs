//! Judge calibration (#57): how often the judge model agrees with a person.
//!
//! A calibration set (`evals/judge-calibration/`) holds conversations from
//! past eval runs, the judged criteria asked about each, a person's label for
//! each criterion (`labels.yaml`) and the judge's verdicts on the same
//! questions (`recorded.json`). The labels and the verdicts are in separate
//! files so that a person labelling does not see what the judge said.
//!
//! Agreement is Cohen's κ: the share of items on which the two agree,
//! corrected for the agreement expected by chance from how often each says
//! pass. It is computed from the recorded verdicts with no model, so a test
//! can hold the judge to [`MIN_KAPPA`]; `onto-eval --calibrate --live` asks
//! the judge again and can record its new verdicts.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

/// The agreement the judge must reach with the labels: "substantial" on the
/// usual reading of κ, and the threshold #57 starts from.
pub const MIN_KAPPA: f64 = 0.7;

/// Below this many items with both a label and a verdict, κ is reported but
/// not enforced: a handful of labels says little either way.
pub const MIN_COMPARED: usize = 20;

/// A verdict on one criterion, by a person or the judge.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Label {
    Pass,
    Fail,
    /// The person could not decide: left out of the agreement.
    Unsure,
}

impl Label {
    fn passed(self) -> Option<bool> {
        match self {
            Label::Pass => Some(true),
            Label::Fail => Some(false),
            Label::Unsure => None,
        }
    }
}

/// One judged criterion about one conversation, with a person's label.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Item {
    /// The conversation's file stem under `conversations/`.
    pub conversation: String,
    /// The criterion id in its task (`followup.<id>` for a follow-up's).
    pub criterion: String,
    /// The judge's question, as the task asked it when the verdict was recorded.
    pub question: String,
    /// The person's label; unset until labelled.
    #[serde(default)]
    pub human: Option<Label>,
    /// Why, when the call was close.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// The judge's verdict on one item.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Verdict {
    pub conversation: String,
    pub criterion: String,
    /// `pass` or `fail`; a verdict the judge could not give is left out.
    pub judge: Label,
    pub reason: String,
}

/// A calibration set on disk.
#[derive(Debug)]
pub struct Set {
    pub dir: PathBuf,
    pub items: Vec<Item>,
    pub recorded: Vec<Verdict>,
}

impl Set {
    pub fn load(dir: &Path) -> Result<Set> {
        let labels = dir.join("labels.yaml");
        let items: Vec<Item> = serde_norway::from_str(&std::fs::read_to_string(&labels).with_context(|| format!("reading {}", labels.display()))?)
            .with_context(|| format!("parsing {}", labels.display()))?;
        let recorded = dir.join("recorded.json");
        let recorded: Vec<Verdict> = serde_json::from_str(&std::fs::read_to_string(&recorded).with_context(|| format!("reading {}", recorded.display()))?)
            .with_context(|| format!("parsing {}", recorded.display()))?;
        let set = Set { dir: dir.to_path_buf(), items, recorded };
        let mut seen = std::collections::BTreeSet::new();
        for item in &set.items {
            if !seen.insert((&item.conversation, &item.criterion)) {
                bail!("{}: {} / {} is listed twice", labels.display(), item.conversation, item.criterion);
            }
            if !set.conversation_path(&item.conversation).is_file() {
                bail!("{}: no conversation {}", labels.display(), item.conversation);
            }
        }
        Ok(set)
    }

    /// The conversation's transcript (JSON lines, as the eval runner writes it).
    pub fn conversation_path(&self, conversation: &str) -> PathBuf {
        self.dir.join("conversations").join(format!("{conversation}.jsonl"))
    }

    /// Writes the labels back, keeping the items' order.
    pub fn save_labels(&self) -> Result<()> {
        std::fs::write(self.dir.join("labels.yaml"), serde_norway::to_string(&self.items)?)?;
        Ok(())
    }

    /// Replaces the recorded verdicts (after a live run).
    pub fn save_recorded(&self, verdicts: &[Verdict]) -> Result<()> {
        std::fs::write(self.dir.join("recorded.json"), serde_json::to_string_pretty(verdicts)? + "\n")?;
        Ok(())
    }
}

/// Cohen's κ for paired pass/fail calls (person, judge), with an approximate
/// 95% interval. `None` when there is nothing to compare or both always gave
/// the same call (chance agreement is then 1 and κ is undefined).
pub fn kappa(pairs: &[(bool, bool)]) -> Option<(f64, f64, f64)> {
    let n = pairs.len() as f64;
    if pairs.is_empty() {
        return None;
    }
    let observed = pairs.iter().filter(|(a, b)| a == b).count() as f64 / n;
    let first = pairs.iter().filter(|(a, _)| *a).count() as f64 / n;
    let second = pairs.iter().filter(|(_, b)| *b).count() as f64 / n;
    let chance = first * second + (1.0 - first) * (1.0 - second);
    if chance >= 1.0 {
        return None;
    }
    let k = (observed - chance) / (1.0 - chance);
    // The large-sample standard error (Cohen 1960): rough for small sets, but
    // it shows how far a few dozen labels are from a precise κ.
    let se = (observed * (1.0 - observed) / (n * (1.0 - chance).powi(2))).sqrt();
    Some((k, (k - 1.96 * se).max(-1.0), (k + 1.96 * se).min(1.0)))
}

/// An item on which the person and the judge disagree.
#[derive(Debug)]
pub struct Disagreement {
    pub conversation: String,
    pub criterion: String,
    pub question: String,
    pub human: Label,
    pub judge: Label,
    pub reason: String,
    pub note: Option<String>,
}

/// How the judge's verdicts compare with the labels.
#[derive(Debug, Default)]
pub struct Agreement {
    /// Items with both a pass/fail label and a verdict.
    pub compared: usize,
    pub agreed: usize,
    /// Cohen's κ and its approximate 95% interval.
    pub kappa: Option<(f64, f64, f64)>,
    /// Counts by (person, judge): `[pass][pass]`, `[pass][fail]`, `[fail][pass]`, `[fail][fail]`.
    pub matrix: [[usize; 2]; 2],
    pub disagreements: Vec<Disagreement>,
    /// Items not labelled yet.
    pub unlabelled: usize,
    /// Items labelled unsure.
    pub unsure: usize,
    /// Labelled items the judge gave no verdict on.
    pub no_verdict: usize,
}

pub fn agreement(items: &[Item], verdicts: &[Verdict]) -> Agreement {
    let by_item: BTreeMap<(&str, &str), &Verdict> = verdicts.iter().map(|v| ((v.conversation.as_str(), v.criterion.as_str()), v)).collect();
    let mut out = Agreement::default();
    let mut pairs = Vec::new();
    for item in items {
        let Some(label) = item.human else {
            out.unlabelled += 1;
            continue;
        };
        let Some(human) = label.passed() else {
            out.unsure += 1;
            continue;
        };
        let Some(verdict) = by_item.get(&(item.conversation.as_str(), item.criterion.as_str())) else {
            out.no_verdict += 1;
            continue;
        };
        let Some(judge) = verdict.judge.passed() else {
            out.no_verdict += 1;
            continue;
        };
        pairs.push((human, judge));
        out.matrix[usize::from(!human)][usize::from(!judge)] += 1;
        if human == judge {
            out.agreed += 1;
        } else {
            out.disagreements.push(Disagreement {
                conversation: item.conversation.clone(),
                criterion: item.criterion.clone(),
                question: item.question.clone(),
                human: label,
                judge: verdict.judge,
                reason: verdict.reason.clone(),
                note: item.note.clone(),
            });
        }
    }
    out.compared = pairs.len();
    out.kappa = kappa(&pairs);
    out
}

impl Agreement {
    /// Whether the judge is good enough: κ at least `min_kappa` once at least
    /// `min_compared` items can be compared. The error says why not.
    pub fn check(&self, min_kappa: f64, min_compared: usize) -> std::result::Result<(), String> {
        if self.compared < min_compared {
            return Ok(());
        }
        match self.kappa {
            Some((k, ..)) if k >= min_kappa => Ok(()),
            Some((k, ..)) => Err(format!("judge agreement κ = {k:.2} is below {min_kappa:.2} over {} labelled items", self.compared)),
            // Everyone said the same thing: agreement is all that can be checked.
            None if self.agreed == self.compared => Ok(()),
            None => Err(format!("judge agreement cannot be computed over {} labelled items", self.compared)),
        }
    }

    /// The report: κ, the confusion counts and every disagreement.
    pub fn report(&self, source: &str) -> String {
        let mut out = format!("# Judge calibration\n\nVerdicts: {source}.\n\n");
        let total = self.compared + self.unlabelled + self.unsure + self.no_verdict;
        let _ = writeln!(
            out,
            "{} of {total} items compared ({} not labelled yet, {} labelled unsure, {} without a verdict).\n",
            self.compared, self.unlabelled, self.unsure, self.no_verdict
        );
        if self.compared == 0 {
            out.push_str("Nothing to compare yet: label the items in `labels.yaml` (`onto-eval --label`).\n");
            return out;
        }
        let _ = writeln!(out, "- Agreement: {} of {} ({:.0}%)", self.agreed, self.compared, 100.0 * self.agreed as f64 / self.compared as f64);
        match self.kappa {
            Some((k, low, high)) => {
                let _ = writeln!(out, "- Cohen's κ: {k:.2} [{low:.2}, {high:.2}] (threshold {MIN_KAPPA:.2})");
            }
            None => out.push_str("- Cohen's κ: undefined (both gave the same call on every item)\n"),
        }
        if self.compared < MIN_COMPARED {
            let _ = writeln!(out, "- Not enforced below {MIN_COMPARED} compared items.");
        }
        let [[pp, pf], [fp, ff]] = self.matrix;
        let _ = write!(out, "\n| Person \\ Judge | pass | fail |\n|---|---|---|\n| pass | {pp} | {pf} |\n| fail | {fp} | {ff} |\n");
        if !self.disagreements.is_empty() {
            out.push_str("\n## Disagreements\n");
            for d in &self.disagreements {
                let label = |l: Label| match l {
                    Label::Pass => "pass",
                    Label::Fail => "fail",
                    Label::Unsure => "unsure",
                };
                let _ = write!(
                    out,
                    "\n### {} / {}\n\n{}\n\n- Person: {}{}\n- Judge: {}: {}\n",
                    d.conversation,
                    d.criterion,
                    d.question,
                    label(d.human),
                    d.note.as_ref().map(|n| format!(" ({n})")).unwrap_or_default(),
                    label(d.judge),
                    d.reason
                );
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kappa_matches_a_worked_example() {
        // 20 pass/pass, 5 pass/fail, 10 fail/pass, 15 fail/fail: po 0.7, pe 0.5, κ 0.4.
        let mut pairs = vec![(true, true); 20];
        pairs.extend(vec![(true, false); 5]);
        pairs.extend(vec![(false, true); 10]);
        pairs.extend(vec![(false, false); 15]);
        let (k, low, high) = kappa(&pairs).unwrap();
        assert!((k - 0.4).abs() < 1e-9, "{k}");
        assert!(low < k && k < high);
        assert_eq!(kappa(&[(true, true), (false, false)]).unwrap().0, 1.0);
        assert!(kappa(&[(true, true); 3]).is_none(), "no spread: undefined");
        assert!(kappa(&[]).is_none());
    }

    fn item(conversation: &str, human: Option<Label>) -> Item {
        Item { conversation: conversation.into(), criterion: "c".into(), question: "q?".into(), human, note: None }
    }

    fn verdict(conversation: &str, judge: Label) -> Verdict {
        Verdict { conversation: conversation.into(), criterion: "c".into(), judge, reason: "because. Verdict: x.".into() }
    }

    #[test]
    fn agreement_counts_disagreements_and_leaves_out_unlabelled_and_unsure() {
        let items = [item("a", Some(Label::Pass)), item("b", Some(Label::Fail)), item("c", None), item("d", Some(Label::Unsure)), item("e", Some(Label::Pass))];
        let verdicts = [verdict("a", Label::Pass), verdict("b", Label::Pass), verdict("c", Label::Fail), verdict("d", Label::Fail)];
        let a = agreement(&items, &verdicts);
        assert_eq!((a.compared, a.agreed, a.unlabelled, a.unsure, a.no_verdict), (2, 1, 1, 1, 1));
        assert_eq!(a.matrix, [[1, 0], [1, 0]]);
        assert_eq!(a.disagreements.len(), 1);
        assert_eq!(a.disagreements[0].conversation, "b");
        let report = a.report("recorded");
        assert!(report.contains("### b / c") && report.contains("- Person: fail"), "{report}");
    }

    #[test]
    fn the_threshold_applies_once_enough_items_are_labelled() {
        let mut items = Vec::new();
        let mut verdicts = Vec::new();
        for i in 0..30 {
            let id = format!("x{i}");
            let human = if i < 15 { Label::Pass } else { Label::Fail };
            // The judge passes everything: agreement 50%, κ 0.
            items.push(item(&id, Some(human)));
            verdicts.push(verdict(&id, Label::Pass));
        }
        let a = agreement(&items, &verdicts);
        assert!(a.check(MIN_KAPPA, MIN_COMPARED).unwrap_err().contains("below 0.70"));
        assert!(a.check(MIN_KAPPA, 31).is_ok(), "too few to enforce");
        assert!(agreement(&[], &[]).check(MIN_KAPPA, MIN_COMPARED).is_ok());
    }
}
