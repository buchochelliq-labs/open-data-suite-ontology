//! The judge against the hand-labelled calibration set (#57), from its
//! recorded verdicts: no model is called.

use onto_testkit::calibration::{MIN_COMPARED, MIN_KAPPA, Set, agreement};
use onto_testkit::fixture::repo_root;

#[test]
fn the_judge_agrees_with_the_hand_labels() {
    let set = Set::load(&repo_root().join("evals/judge-calibration")).unwrap();
    assert!(set.items.len() >= 40, "the set has {} items", set.items.len());
    let agreement = agreement(&set.items, &set.recorded);
    assert_eq!(agreement.no_verdict, 0, "every labelled item has a recorded verdict");
    if let Err(e) = agreement.check(MIN_KAPPA, MIN_COMPARED) {
        panic!("{e}\n\n{}", agreement.report("recorded"));
    }
}

#[test]
fn every_item_is_a_judged_criterion_of_its_task() {
    let set = Set::load(&repo_root().join("evals/judge-calibration")).unwrap();
    for item in &set.items {
        assert!(!item.question.trim().is_empty(), "{} / {}", item.conversation, item.criterion);
        assert!(
            set.recorded.iter().any(|v| v.conversation == item.conversation && v.criterion == item.criterion),
            "{} / {} has no recorded verdict",
            item.conversation,
            item.criterion
        );
    }
}
