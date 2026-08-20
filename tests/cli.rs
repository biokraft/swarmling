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

/// A magnet with a well-formed infohash, so `add` gets past parsing and
/// reaches the VPN check. Nothing is ever transferred: `add` only writes the
/// queue file, and here that file lives in a temporary directory.
const VALID_MAGNET: &str =
    "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567&dn=example";

fn vpn_is_up(data_dir: &std::path::Path) -> bool {
    let out = Command::cargo_bin("swarmling")
        .unwrap()
        .env("SWARMLING_DATA_DIR", data_dir)
        .args(["vpn", "status"])
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&out.get_output().stdout).into_owned();
    stdout.contains("state: connected")
}

#[test]
fn add_honours_the_vpn_requirement() {
    let dir = tempfile::tempdir().unwrap();
    // Turn the requirement on through the CLI itself, so the whole path is
    // covered: settings are written, then read back by `add`.
    Command::cargo_bin("swarmling")
        .unwrap()
        .env("SWARMLING_DATA_DIR", dir.path())
        .args(["vpn", "require", "on"])
        .assert()
        .success();

    let assertion = Command::cargo_bin("swarmling")
        .unwrap()
        .env("SWARMLING_DATA_DIR", dir.path())
        .args(["add", VALID_MAGNET])
        .assert();

    // The outcome depends on whether the machine running the suite actually
    // has a VPN up, so both branches are asserted rather than assuming one.
    if vpn_is_up(dir.path()) {
        assertion.success();
    } else {
        assertion
            .failure()
            .stderr(predicate::str::contains("vpn_required"));
    }
}

#[test]
fn add_is_not_blocked_when_the_vpn_requirement_is_off() {
    let dir = tempfile::tempdir().unwrap();
    Command::cargo_bin("swarmling")
        .unwrap()
        .env("SWARMLING_DATA_DIR", dir.path())
        .args(["vpn", "require", "off"])
        .assert()
        .success();
    Command::cargo_bin("swarmling")
        .unwrap()
        .env("SWARMLING_DATA_DIR", dir.path())
        .args(["add", VALID_MAGNET])
        .assert()
        .success()
        .stdout(predicate::str::contains("added"));
}
