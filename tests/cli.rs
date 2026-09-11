use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn help_does_not_require_a_terminal() {
    Command::cargo_bin("crank")
        .unwrap()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("Usage:"));
}

#[test]
fn validates_embedded_catalog() {
    Command::cargo_bin("crank")
        .unwrap()
        .arg("--validate")
        .assert()
        .success()
        .stdout(predicate::str::contains("catalog is valid"));
}
