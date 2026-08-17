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

#[test]
fn help_lists_the_download_subcommands() {
    let out = Command::cargo_bin("swarmling")
        .unwrap()
        .arg("--help")
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&out.get_output().stdout).into_owned();
    for expected in ["search", "add", "status", "rm"] {
        assert!(
            stdout.contains(expected),
            "missing subcommand {expected} in help:\n{stdout}"
        );
    }
}

#[test]
fn add_requires_a_magnet_argument() {
    Command::cargo_bin("swarmling")
        .unwrap()
        .arg("add")
        .assert()
        .failure(); // clap rejects the missing positional before any network work happens
}

#[test]
fn rm_requires_an_infohash_argument() {
    Command::cargo_bin("swarmling")
        .unwrap()
        .arg("rm")
        .assert()
        .failure();
}

#[test]
fn rm_rejects_a_bare_integer_instead_of_an_infohash() {
    // librqbit's own parser treats a bare integer as a session INDEX, so a
    // typo like `rm 0` must be rejected at the CLI before it can reach the
    // engine and delete an unrelated torrent.
    Command::cargo_bin("swarmling")
        .unwrap()
        .args(["rm", "0"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("infohash"));
}

#[test]
fn add_rejects_a_magnet_with_no_usable_infohash() {
    Command::cargo_bin("swarmling")
        .unwrap()
        .args(["add", "magnet:?dn=no+hash+here"])
        .assert()
        .failure();
}

#[test]
fn help_lists_the_vpn_subcommand() {
    let out = Command::cargo_bin("swarmling")
        .unwrap()
        .arg("--help")
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&out.get_output().stdout).into_owned();
    assert!(
        stdout.contains("vpn"),
        "missing vpn subcommand in help:\n{stdout}"
    );
}

#[test]
fn vpn_require_rejects_a_bad_value() {
    Command::cargo_bin("swarmling")
        .unwrap()
        .args(["vpn", "require", "maybe"])
        .assert()
        .failure();
}
