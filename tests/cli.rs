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

#[test]
fn interactive_mode_explains_when_no_terminal_is_attached() {
    Command::cargo_bin("crank")
        .unwrap()
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "interactive mode requires a terminal",
        ));
}
