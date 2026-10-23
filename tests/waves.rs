//! Waves: how a run is cut into steps and where it stops. The repos here have no
//! credential rule, so each one fails at its first step without any network call.
//! That is enough to test the wave logic itself.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use fleetify::credentials::Credentials;
use fleetify::hosts::HostType;
use fleetify::hosts::client_for;
use fleetify::recipe::Recipe;
use fleetify::report::{RepoResult, Report};
use fleetify::repos::Repo;
use fleetify::run::{
    Loaded, Mode, RunLayout, WavePlan, Waves, execute_loaded, parse_waves, split_waves,
};

const RECIPE: &str = r#"schema = 1
name = "waves"
branch = "chore/waves"
pr_title = "chore: waves"
"#;

fn repo(path: &str) -> Repo {
    Repo {
        host: "github.com".into(),
        host_type: HostType::Github,
        path: path.into(),
        name: path.rsplit('/').next().unwrap().into(),
        clone_url: format!("https://github.com/{path}.git"),
        line: 1,
    }
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("fleetify-waves-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn run_waves(
    name: &str,
    paths: &[&str],
    plan: &WavePlan,
    proceed: &dyn Fn(&[RepoResult], usize) -> bool,
) -> Report {
    let base = scratch(name);
    let recipe = Recipe::parse(RECIPE).unwrap();
    let client = client_for(HostType::Github, "github.com", None).unwrap();
    let loaded = Loaded {
        credentials: Credentials::parse("").unwrap(),
        repos: paths.iter().map(|p| repo(p)).collect(),
        clients: BTreeMap::from([("github.com".to_string(), client)]),
        agent: None,
        model: None,
        merge: false,
    };
    let layout = RunLayout::new(&base, name);
    let report = execute_loaded(
        &layout,
        &recipe,
        &base,
        &base,
        loaded,
        Mode::Pipeline { parallel: 1 },
        &Waves { plan, proceed },
    )
    .unwrap();
    let _ = fs::remove_dir_all(&base);
    report
}

#[test]
fn waves_parse_and_refuse_the_wrong_shapes() {
    assert_eq!(parse_waves("1,all").unwrap(), vec![Some(1), None]);
    assert_eq!(
        parse_waves("1,5,all").unwrap(),
        vec![Some(1), Some(5), None]
    );
    assert_eq!(parse_waves("all").unwrap(), vec![None]);
    for bad in ["1,5", "all,1", "0,all", "x,all", ""] {
        assert!(parse_waves(bad).is_err(), "{bad} must be refused");
    }
}

#[test]
fn repos_are_cut_into_waves_in_list_order() {
    let repos: Vec<Repo> = ["o/a", "o/b", "o/c", "o/d", "o/e"]
        .iter()
        .map(|p| repo(p))
        .collect();
    let plan = WavePlan {
        sizes: vec![Some(1), Some(2), None],
        canaries: Vec::new(),
    };
    let waves = split_waves(&repos, &plan).unwrap();
    let sizes: Vec<usize> = waves.iter().map(Vec::len).collect();
    assert_eq!(sizes, vec![1, 2, 2]);
    assert_eq!(waves[1][0].path, "o/b");
}

#[test]
fn canaries_become_the_first_wave_and_unknown_canaries_are_refused() {
    let repos: Vec<Repo> = ["o/a", "o/b", "o/c"].iter().map(|p| repo(p)).collect();
    let plan = WavePlan {
        sizes: vec![Some(1), None],
        canaries: vec!["c".into()],
    };
    let waves = split_waves(&repos, &plan).unwrap();
    assert_eq!(waves[0][0].path, "o/c");
    assert_eq!(waves.iter().map(Vec::len).sum::<usize>(), 3);

    let unknown = WavePlan {
        sizes: vec![None],
        canaries: vec!["nope".into()],
    };
    assert!(split_waves(&repos, &unknown).is_err());
}

#[test]
fn a_run_that_is_not_told_to_continue_stops_after_the_first_wave() {
    let plan = WavePlan::default();
    let report = run_waves("stop", &["o/a", "o/b", "o/c"], &plan, &|_, _| false);
    assert_eq!(report.summary.total, 1, "{report:?}");
    assert_eq!(report.summary.pending, 2);
    assert_eq!(
        report.exit_code, 1,
        "a failed repo means exit 1 even when stopped"
    );
}

#[test]
fn a_run_told_to_continue_processes_every_wave() {
    let plan = WavePlan::default();
    let report = run_waves("all", &["o/a", "o/b", "o/c"], &plan, &|_, _| true);
    assert_eq!(report.summary.total, 3);
    assert_eq!(report.summary.pending, 0);
}

#[test]
fn the_gate_sees_only_the_wave_that_just_finished() {
    let plan = WavePlan {
        sizes: vec![Some(2), None],
        canaries: Vec::new(),
    };
    let seen = std::cell::RefCell::new(Vec::new());
    let report = run_waves("gate", &["o/a", "o/b", "o/c"], &plan, &|done, remaining| {
        seen.borrow_mut().push((done.len(), remaining));
        true
    });
    assert_eq!(*seen.borrow(), vec![(2, 1)]);
    assert_eq!(report.summary.total, 3);
}
