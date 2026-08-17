use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn help_lists_search_subcommand() {
    Command::cargo_bin("swarmling")
        .unwrap()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("search"));
}

#[test]
fn version_prints_name_and_version() {
    Command::cargo_bin("swarmling")
        .unwrap()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains("swarmling"));
}
