//! The harness rule (D25 E, contract §11): every suite runs on a fresh root inside its run
//! folder (`COMMONKIT_TEST_RUN`), with the algorithm it asked for. `hash_is_what_was_asked`
//! is the guard every suite that registers a rule scheme carries; this is commonkit's own.

mod child;

use commonkit::{policy, testing};

/// Variables to set (`Some`) or remove (`None`) in a child.
type Env<'a> = Vec<(&'a str, Option<&'a str>)>;

#[test]
fn hash_is_what_was_asked() {
    if child::is_child() {
        return;
    }
    testing::assert_isolated();
    if let Some(asked) = std::env::var_os("COMMONKIT_HASH").filter(|v| !v.is_empty()) {
        let asked = policy::Algo::from_name(&asked.to_string_lossy());
        assert_eq!(Some(policy::current()), asked, "{}", policy::source());
    }
}

/// In a child: say whether `assert_isolated` would pass.
#[test]
fn child_checks_isolation() {
    if !child::is_child() {
        return;
    }
    match testing::check_isolated() {
        Ok(()) => println!("ISOLATED"),
        Err(e) => println!("REFUSED: {e}"),
    }
    let asked = std::env::var("COMMONKIT_HASH").unwrap_or_default();
    println!("SAME={}", policy::current().name() == asked);
}

#[test]
fn assert_isolated_refuses_each_unsafe_case() {
    let elsewhere = tempfile::tempdir().unwrap();
    let default = policy::default_root().unwrap();
    let default_run = default.parent().unwrap().to_string_lossy().into_owned();
    let other = elsewhere.path().to_string_lossy().into_owned();
    let default = default.to_string_lossy().into_owned();
    let cases: [(&str, Env); 4] = [
        ("root unset", vec![("COMMONKIT_STORE_ROOT", None)]),
        ("run unset", vec![("COMMONKIT_TEST_RUN", None)]),
        (
            "root outside the run",
            vec![("COMMONKIT_STORE_ROOT", Some(&other))],
        ),
        (
            "default root",
            vec![
                ("COMMONKIT_STORE_ROOT", Some(&default)),
                ("COMMONKIT_TEST_RUN", Some(&default_run)),
            ],
        ),
    ];
    for (name, env) in cases {
        let d = child::dirs();
        let o = child::run("child_checks_isolation", &d, Some("b3"), &env);
        child::ok(&o);
        assert!(
            child::stdout(&o).contains("REFUSED"),
            "{name}: {}",
            child::stdout(&o)
        );
    }
}

#[test]
fn assert_isolated_passes_on_fresh_and_already_written_roots() {
    let d = child::dirs();
    let o = child::run("child_checks_isolation", &d, Some("b3"), &[]);
    child::ok(&o);
    assert!(child::stdout(&o).contains("ISOLATED"));
    std::fs::create_dir_all(d.root.join("plumarr/ab")).unwrap(); // an earlier test wrote here
    let o = child::run("child_checks_isolation", &d, Some("b3"), &[]);
    assert!(child::stdout(&o).contains("ISOLATED"));
}

#[test]
fn the_guard_sees_a_file_that_overrides_the_variable() {
    let d = child::dirs();
    std::fs::write(d.root.join(".policy.json"), r#"{"HASH": "md5"}"#).unwrap();
    let o = child::run("child_checks_isolation", &d, Some("b3"), &[]);
    assert!(
        child::stdout(&o).contains("SAME=false"),
        "{}",
        child::stdout(&o)
    );
}

#[test]
fn fresh_root_is_empty_and_inside_the_run() {
    if child::is_child() {
        return;
    }
    let r = testing::fresh_root();
    assert_eq!(std::fs::read_dir(&r).unwrap().count(), 0);
    if let Some(run) = std::env::var_os("COMMONKIT_TEST_RUN") {
        let run = std::fs::canonicalize(run).unwrap();
        assert!(std::fs::canonicalize(&r).unwrap().starts_with(run));
    }
    std::fs::remove_dir(r).unwrap();
}
