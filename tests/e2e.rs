//! The real `bddkit` binary loads this crate's `cdylib` and runs feature
//! files against the fake Appium from `common`. Skips itself, saying why,
//! when no `bddkit` binary is found, unless `CI` is set: there a missing
//! binary is a failure, since a green run that tested nothing is worse.

mod common;

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use common::*;
use serde_json::json;

/// `BDDKIT_BIN`, then `bddkit` on `PATH`, then a sibling `../bddkit` build.
fn bddkit_bin() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("BDDKIT_BIN") {
        let p = PathBuf::from(p);
        assert!(p.is_file(), "BDDKIT_BIN={} is not a file", p.display());
        // Absolute, so it keeps working after `current_dir(tempdir)`.
        return Some(std::fs::canonicalize(&p).expect("canonicalize BDDKIT_BIN"));
    }
    let exe = format!("bddkit{}", std::env::consts::EXE_SUFFIX);
    if let Ok(path) = std::env::var("PATH")
        && let Some(found) = std::env::split_paths(&path)
            .map(|d| d.join(&exe))
            .find(|p| p.is_file())
    {
        return Some(found);
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    [
        root.join("../bddkit/target/release").join(&exe),
        root.join("../bddkit/target/debug").join(&exe),
    ]
    .into_iter()
    .find(|p| p.is_file())
}

macro_rules! require_bddkit {
    () => {
        match bddkit_bin() {
            Some(bin) => bin,
            None => {
                let why =
                    "no bddkit binary — set BDDKIT_BIN, put bddkit on PATH, or build ../bddkit";
                assert!(
                    std::env::var_os("CI").is_none(),
                    "{why} (CI is set, so this is not skipped)"
                );
                eprintln!("SKIP: {why}");
                return;
            }
        }
    };
}

fn build_plugin() -> PathBuf {
    let out = Command::new(env!("CARGO"))
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .arg("build")
        .output()
        .expect("cargo build");
    assert!(
        out.status.success(),
        "plugin build failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target/debug")
        .join(format!(
            "{}bddkit_appium{}",
            std::env::consts::DLL_PREFIX,
            std::env::consts::DLL_SUFFIX
        ));
    assert!(path.exists(), "missing {}", path.display());
    path
}

fn shop() -> Stub {
    let mut late = StubElement::new("accessibility id", "late", "Sale");
    late.absent_for = 3;
    start_stub(StubState {
        session_caps: [("appPackage".to_string(), json!("io.shop"))]
            .into_iter()
            .collect(),
        elements: vec![
            StubElement::new("accessibility id", "cart", "Cart (0)"),
            StubElement::new("id", "search_input", ""),
            late,
        ],
        ..Default::default()
    })
}

/// A throwaway project: plugin lock, config pointing at `stub`, locator map
/// and the named feature files. Returns its directory.
fn project(name: &str, stub: &Stub, features: &[&str]) -> tempfile::TempDir {
    let dir = tempfile::Builder::new()
        .prefix(&format!("bddkit-appium-e2e-{name}-"))
        .tempdir()
        .expect("tempdir");
    let root = dir.path();
    std::fs::create_dir_all(root.join("features")).expect("mkdir");
    std::fs::create_dir_all(root.join(".bddkit")).expect("mkdir");
    let from = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/features");
    for f in features {
        std::fs::copy(from.join(f), root.join("features").join(f)).expect("copy feature");
    }
    // Forward slashes: the path goes into YAML, where `\` would be an escape.
    let plugin = build_plugin().display().to_string().replace('\\', "/");
    std::fs::write(
        root.join(".bddkit/plugins.yaml"),
        format!("plugin:\n  - name: appium\n    path: \"{plugin}\"\n"),
    )
    .expect("lock");
    std::fs::write(
        root.join("locators.yaml"),
        "cart button: ~cart\nsearch field: \"#search_input\"\nlate banner: ~late\npromo banner: ~promo\n",
    )
    .expect("locators");
    std::fs::write(
        root.join("bddkit.yaml"),
        format!(
            "paths: [features]\nconcurrency: 2\nresources:\n  api: {{}}\n  app:\n    phone:\n      platform: android\n      remote_url: {}\n      app: /stub/shop.apk\n      locators: locators.yaml\n      find_timeout_secs: 2\n",
            stub.url
        ),
    )
    .expect("config");
    dir
}

fn bddkit(bin: &Path, dir: &Path, args: &[&str]) -> Output {
    Command::new(bin)
        .current_dir(dir)
        .args(args)
        // Only this project's lock: a developer's own plugins must not load here.
        .env("BDDKIT_DIR", dir.join(".bddkit"))
        .output()
        .expect("run bddkit")
}

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[test]
fn the_vocabulary_passes_against_the_fake() {
    let bin = require_bddkit!();
    let stub = shop();
    let dir = project("vocabulary", &stub, &["app.feature"]);
    let out = bddkit(&bin, dir.path(), &["run", "--config", "bddkit.yaml"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    assert!(
        !stub.calls_to("mobile: clearApp").is_empty(),
        "the host reset the app between scenarios"
    );
}

#[test]
fn a_failure_exits_1_and_dumps_evidence() {
    let bin = require_bddkit!();
    let stub = shop();
    let dir = project("failing", &stub, &["failing.feature"]);
    let out = bddkit(&bin, dir.path(), &["run", "--config", "bddkit.yaml"]);
    let all = text(&out);
    assert_eq!(out.status.code(), Some(1), "{all}");
    assert!(
        all.contains("no element \"promo banner\" on the screen"),
        "{all}"
    );
    assert!(
        all.contains("Screenshot (image)")
            && all.contains("Page source (text)")
            && all.contains("Last WebDriver exchange (http)"),
        "{all}"
    );
}

#[test]
fn files_in_one_serial_chain_take_the_device_in_turn() {
    let bin = require_bddkit!();
    let stub = shop();
    let dir = project(
        "serial",
        &stub,
        &["serial_one.feature", "serial_two.feature"],
    );
    let out = bddkit(&bin, dir.path(), &["run", "--config", "bddkit.yaml"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    let sessions: Vec<String> = stub
        .calls()
        .into_iter()
        .filter(|c| c.starts_with("POST /session ") || c.starts_with("DELETE /session/s1"))
        .map(|c| c.split_whitespace().take(2).collect::<Vec<_>>().join(" "))
        .collect();
    assert_eq!(
        sessions,
        [
            "POST /session",
            "DELETE /session/s1",
            "POST /session",
            "DELETE /session/s1"
        ],
        "each file opens and closes its session before the next file's opens"
    );
}

#[test]
fn doctor_validates_the_locator_map_and_probes_the_fake() {
    let bin = require_bddkit!();
    let stub = shop();
    let dir = project("doctor", &stub, &["app.feature"]);
    let out = bddkit(
        &bin,
        dir.path(),
        &["doctor", "--live", "--config", "bddkit.yaml"],
    );
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    assert!(
        !stub.calls_to("GET /status").is_empty(),
        "doctor --live probed the fake"
    );
    std::fs::write(dir.path().join("locators.yaml"), "cart button: =Cart\n")
        .expect("break the map");
    let out = bddkit(&bin, dir.path(), &["doctor", "--config", "bddkit.yaml"]);
    let all = text(&out);
    assert_eq!(
        out.status.code(),
        Some(1),
        "doctor reports, and never exits 2: {all}"
    );
    assert!(all.contains("Windows only"), "{all}");
    let out = bddkit(&bin, dir.path(), &["run", "--config", "bddkit.yaml"]);
    let all = text(&out);
    assert_eq!(
        out.status.code(),
        Some(2),
        "a bad locator map stops the run before it starts: {all}"
    );
    assert!(all.contains("Windows only"), "{all}");
}
