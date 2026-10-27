use onto_testkit::fixture::repo_root;
use onto_testkit::scenario::Scenario;

#[tokio::test]
async fn all_scenarios_pass() {
    let dir = repo_root().join("scenarios");
    let mut paths: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().path()).filter(|p| p.extension().is_some_and(|e| e == "yaml")).collect();
    paths.sort();
    assert!(!paths.is_empty(), "no scenarios in {}", dir.display());
    for path in paths {
        let scenario = Scenario::load(&path).unwrap();
        scenario.run().await.unwrap_or_else(|e| panic!("{}: {e:#}", path.display()));
    }
}
