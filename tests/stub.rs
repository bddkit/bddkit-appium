//! The exports driven in-process against a fake Appium. Nothing here needs a
//! device; what it pins is the plugin's side of the protocol and its replies
//! to the host.

mod common;

use std::ffi::CString;
use std::path::Path;

use bddkit_appium::{
    bddkit_dispatch, bddkit_drop_instance, bddkit_init_instance, bddkit_list_steps,
    bddkit_probe_config, bddkit_reset_scenario,
};
use common::*;
use serde_json::{Value, json};

fn android(url: &str, dir: &Path) -> Value {
    json!({
        "platform": "android",
        "remote_url": url,
        "app": "/builds/shop.apk",
        "locators": write_locators(dir, "cart button: ~cart\nsearch field: \"#search_input\"\n"),
        "find_timeout_secs": 1,
    })
}

fn windows(url: &str, dir: &Path) -> Value {
    json!({
        "platform": "windows",
        "remote_url": url,
        "app": "C:\\Shop\\Shop.exe",
        "locators": write_locators(dir, "cart button: ~CartButton\nsearch field: =Search\n"),
        "find_timeout_secs": 1,
    })
}

fn init(name: &str, config: Value) -> Value {
    envelope(bddkit_init_instance, &instance_request(name, config))
}

fn handle(reply: &Value) -> u64 {
    assert_eq!(reply["ok"], true, "{reply}");
    reply["handle"].as_u64().expect("handle")
}

fn drop_instance(h: u64) -> Value {
    take(bddkit_drop_instance(h))
}

fn reset(h: u64) -> Value {
    take(bddkit_reset_scenario(h))
}

fn stub_with_package() -> Stub {
    start_stub(StubState {
        session_caps: [("appPackage".to_string(), json!("io.shop"))]
            .into_iter()
            .collect(),
        ..Default::default()
    })
}

#[test]
fn init_opens_a_session_with_the_platform_capabilities_and_the_extras() {
    let stub = stub_with_package();
    let dir = tempfile::tempdir().expect("tempdir");
    let mut config = android(&stub.url, dir.path());
    config["capabilities"] = json!({"appium:udid": "emulator-5554"});
    let h = handle(&init("caps", config));
    let session = &stub.calls_to("POST /session ")[0];
    let body: Value =
        serde_json::from_str(session.trim_start_matches("POST /session ")).expect("JSON");
    let always = &body["capabilities"]["alwaysMatch"];
    assert_eq!(always["platformName"], "Android");
    assert_eq!(always["appium:automationName"], "UiAutomator2");
    assert_eq!(always["appium:app"], "/builds/shop.apk");
    assert_eq!(always["appium:udid"], "emulator-5554");
    assert_eq!(drop_instance(h)["ok"], true);
    assert_eq!(stub.calls_to("DELETE /session/s1").len(), 1);
}

#[test]
fn a_second_init_of_the_same_instance_is_refused_until_the_first_is_dropped() {
    let stub = stub_with_package();
    let dir = tempfile::tempdir().expect("tempdir");
    let first = handle(&init("phone-excl", android(&stub.url, dir.path())));
    let second = init("phone-excl", android(&stub.url, dir.path()));
    assert_eq!(second["ok"], false);
    let error = second["error"].as_str().expect("error");
    assert!(
        error.contains("\"phone-excl\"") && error.contains("@serial("),
        "{error}"
    );
    assert_eq!(
        stub.calls_to("POST /session ").len(),
        1,
        "the refusal opens nothing"
    );
    let other = handle(&init("tablet-excl", android(&stub.url, dir.path())));
    assert_eq!(drop_instance(first)["ok"], true);
    let again = handle(&init("phone-excl", android(&stub.url, dir.path())));
    drop_instance(again);
    drop_instance(other);
}

#[test]
fn android_reset_clears_then_activates_the_package_the_session_reports() {
    let stub = stub_with_package();
    let dir = tempfile::tempdir().expect("tempdir");
    let h = handle(&init("reset-clear", android(&stub.url, dir.path())));
    assert_eq!(reset(h)["ok"], true);
    let scripts = stub.calls_to("/execute/sync");
    assert_eq!(scripts.len(), 2, "{scripts:?}");
    assert!(
        scripts[0].contains(r#""script":"mobile: clearApp""#)
            && scripts[0].contains(r#""appId":"io.shop""#),
        "{}",
        scripts[0]
    );
    assert!(
        scripts[1].contains(r#""script":"mobile: activateApp""#)
            && scripts[1].contains(r#""appId":"io.shop""#),
        "{}",
        scripts[1]
    );
    drop_instance(h);
}

#[test]
fn relaunch_terminates_instead_and_the_package_can_come_from_the_device() {
    let stub = start_stub(StubState {
        current_package: Some("io.device".into()),
        ..Default::default()
    });
    let dir = tempfile::tempdir().expect("tempdir");
    let mut config = android(&stub.url, dir.path());
    config["reset"] = json!("relaunch");
    let h = handle(&init("reset-relaunch", config));
    assert_eq!(reset(h)["ok"], true);
    let scripts = stub.calls_to("/execute/sync");
    assert!(
        scripts[0].contains("mobile: getCurrentPackage"),
        "{scripts:?}"
    );
    assert!(
        scripts[1].contains("mobile: terminateApp") && scripts[1].contains("io.device"),
        "{scripts:?}"
    );
    assert!(
        scripts[2].contains("mobile: activateApp") && scripts[2].contains("io.device"),
        "{scripts:?}"
    );
    drop_instance(h);
}

#[test]
fn an_android_session_whose_package_cannot_be_told_is_closed_and_the_name_freed() {
    let stub = start_stub(StubState::default());
    let dir = tempfile::tempdir().expect("tempdir");
    let reply = init("no-package", android(&stub.url, dir.path()));
    assert_eq!(reply["ok"], false);
    assert!(
        reply["error"]
            .as_str()
            .expect("error")
            .contains("appium:appPackage"),
        "{reply}"
    );
    assert_eq!(
        stub.calls_to("DELETE /session/s1").len(),
        1,
        "the half-open session is closed"
    );
    stub.state.lock().expect("state").current_package = Some("io.shop".into());
    let h = handle(&init("no-package", android(&stub.url, dir.path())));
    drop_instance(h);
}

#[test]
fn windows_reset_closes_and_relaunches_and_drop_closes_before_deleting() {
    let stub = start_stub(StubState::default());
    let dir = tempfile::tempdir().expect("tempdir");
    let h = handle(&init("win", windows(&stub.url, dir.path())));
    assert!(
        stub.calls_to("/execute/sync").is_empty(),
        "windows needs no package"
    );
    assert_eq!(reset(h)["ok"], true);
    assert_eq!(drop_instance(h)["ok"], true);
    let tail: Vec<String> = stub
        .calls()
        .into_iter()
        .filter(|c| c.contains("execute/sync") || c.starts_with("DELETE"))
        .collect();
    assert!(tail[0].contains("windows: closeApp"), "{tail:?}");
    assert!(tail[1].contains("windows: launchApp"), "{tail:?}");
    assert!(
        tail[2].contains("windows: closeApp"),
        "drop closes the app first: {tail:?}"
    );
    assert!(tail[3].starts_with("DELETE /session/s1"), "{tail:?}");
}

#[test]
fn probe_asks_only_for_status_and_reports_not_ready() {
    let stub = start_stub(StubState::default());
    let dir = tempfile::tempdir().expect("tempdir");
    let request = instance_request("probe", android(&stub.url, dir.path()));
    assert_eq!(envelope(bddkit_probe_config, &request)["ok"], true);
    assert_eq!(stub.calls(), vec!["GET /status".to_string()]);
    stub.state.lock().expect("state").not_ready = true;
    let reply = envelope(bddkit_probe_config, &request);
    assert_eq!(reply["ok"], false);
    assert!(
        reply["error"]
            .as_str()
            .expect("error")
            .contains("not ready"),
        "{reply}"
    );
}

#[test]
fn an_unknown_handle_is_an_error_not_a_crash() {
    assert_eq!(reset(u64::MAX)["error"], "unknown handle");
    assert_eq!(drop_instance(u64::MAX)["error"], "unknown handle");
}

/// Dispatches step `index`; artifacts land in `dir/<n>`.
fn dispatch(h: u64, index: u32, args: &[&str], artifacts: &Path) -> Value {
    let request = json!({
        "args": args, "docstring": null, "table": null,
        "artifacts_dir": artifacts.display().to_string(),
        "workspace_dir": artifacts.display().to_string(),
        "debug": false, "options": {},
    });
    let c = CString::new(request.to_string()).expect("no NUL");
    take(bddkit_dispatch(h, index, c.as_ptr()))
}

fn shop_stub() -> Stub {
    start_stub(StubState {
        session_caps: [("appPackage".to_string(), json!("io.shop"))]
            .into_iter()
            .collect(),
        elements: vec![
            StubElement::new("accessibility id", "cart", "Cart (0)"),
            StubElement::new("id", "search_input", ""),
        ],
        ..Default::default()
    })
}

#[test]
fn list_steps_is_the_ten_step_vocabulary_in_the_app_group() {
    let steps = take(bddkit_list_steps());
    let steps = steps.as_array().expect("array");
    assert_eq!(steps.len(), 10);
    assert!(steps.iter().all(|s| s["group"] == "app"));
    let kinds: Vec<&str> = steps
        .iter()
        .map(|s| s["kind"].as_str().expect("kind"))
        .collect();
    assert_eq!(
        kinds,
        ["action"; 7]
            .iter()
            .chain(["assertion"; 3].iter())
            .copied()
            .collect::<Vec<_>>()
    );
}

/// Texts of every bddkit-browser step that shares a word with this plugin.
/// Loaded together, a step matching both would stop the run as ambiguous.
const BROWSER_STEPS: &[&str] = &[
    r#"I read the "x" element text as "y""#,
    r#"I take a screenshot"#,
    r#"the "x" element should be visible"#,
    r#"the "x" element should not be visible"#,
    r#"I press "x""#,
    r#"I click on "x""#,
    r#"I fill in "x" with "y""#,
    r#"the "x" element should contain "y""#,
];

#[test]
fn no_step_overlaps_bddkit_browser() {
    let steps = take(bddkit_list_steps());
    for s in steps.as_array().expect("array") {
        let re = regex::Regex::new(s["pattern"].as_str().expect("pattern")).expect("valid regex");
        for text in BROWSER_STEPS {
            assert!(
                !re.is_match(text),
                "{} matches the browser step {text:?}",
                s["pattern"]
            );
        }
    }
}

#[test]
fn tap_waits_for_an_element_that_appears_late() {
    let stub = shop_stub();
    stub.state.lock().expect("state").elements[0].absent_for = 3;
    let dir = tempfile::tempdir().expect("tempdir");
    let h = handle(&init("tap-late", android(&stub.url, dir.path())));
    let r = dispatch(h, 0, &["cart button"], &dir.path().join("a"));
    assert_eq!(r["status"], "passed", "{r}");
    assert_eq!(stub.calls_to("/element/e0/click").len(), 1);
    drop_instance(h);
}

#[test]
fn an_action_waits_out_a_busy_ui_tree() {
    let stub = shop_stub();
    stub.state.lock().expect("state").busy_first = 2;
    let dir = tempfile::tempdir().expect("tempdir");
    let h = handle(&init("tap-busy", android(&stub.url, dir.path())));
    let r = dispatch(h, 0, &["cart button"], &dir.path().join("a"));
    assert_eq!(r["status"], "passed", "{r}");
    assert_eq!(stub.calls_to("/element/e0/click").len(), 1);
    drop_instance(h);
}

#[test]
fn an_action_that_stays_busy_past_the_timeout_fails_with_the_servers_message() {
    let stub = shop_stub();
    stub.state.lock().expect("state").busy_first = 1000;
    let dir = tempfile::tempdir().expect("tempdir");
    let mut config = android(&stub.url, dir.path());
    config["find_timeout_secs"] = json!(0);
    let h = handle(&init("tap-busy-forever", config));
    let r = dispatch(h, 0, &["cart button"], &dir.path().join("a"));
    assert_eq!(r["status"], "fatal", "{r}");
    let e = r["error"].as_str().expect("error");
    assert!(e.contains("AccessibilityNodeInfo"), "{r}");
    assert!(!e.contains("within"), "{r}");
    drop_instance(h);
}

#[test]
fn an_assertion_on_a_busy_ui_tree_is_not_yet() {
    let stub = shop_stub();
    let dir = tempfile::tempdir().expect("tempdir");
    let h = handle(&init("assert-busy", android(&stub.url, dir.path())));
    stub.state.lock().expect("state").busy_first = 1;
    let r = dispatch(h, 7, &["cart button"], &dir.path().join("a"));
    assert_eq!(r["status"], "not_yet", "{r}");
    assert!(
        r["error"]
            .as_str()
            .expect("error")
            .contains("AccessibilityNodeInfo"),
        "{r}"
    );
    drop_instance(h);
}

#[test]
fn a_busy_ui_tree_never_answers_not_on_the_screen() {
    let stub = shop_stub();
    let dir = tempfile::tempdir().expect("tempdir");
    let h = handle(&init("assert-off-busy", android(&stub.url, dir.path())));
    let a = dir.path().join("a");
    stub.state.lock().expect("state").busy_first = 1;
    let r = dispatch(h, 8, &["cart button"], &a);
    assert_eq!(
        r["status"], "not_yet",
        "a busy find is not an absent element: {r}"
    );
    stub.state.lock().expect("state").busy_reads = 1;
    let r = dispatch(h, 8, &["cart button"], &a);
    assert_eq!(r["status"], "not_yet", "a busy displayed read: {r}");
    assert!(
        r["error"]
            .as_str()
            .expect("error")
            .contains("AccessibilityNodeInfo"),
        "{r}"
    );
    drop_instance(h);
}

#[test]
fn an_action_that_never_finds_its_element_is_fatal_with_evidence() {
    let stub = shop_stub();
    let dir = tempfile::tempdir().expect("tempdir");
    let mut config = android(&stub.url, dir.path());
    config["locators"] = json!(write_locators(dir.path(), "ghost: ~ghost\n"));
    config["find_timeout_secs"] = json!(0);
    let h = handle(&init("tap-missing", config));
    let artifacts = dir.path().join("fail");
    let r = dispatch(h, 0, &["ghost"], &artifacts);
    assert_eq!(r["status"], "fatal", "{r}");
    assert!(
        r["error"]
            .as_str()
            .expect("error")
            .contains("no element \"ghost\" (~ghost) on the screen within 0s"),
        "{r}"
    );
    let d = r["diagnostics"].as_array().expect("diagnostics");
    assert_eq!(d[0]["kind"], "image");
    assert_eq!(d[1]["title"], "Page source");
    assert_eq!(d[2]["kind"], "http");
    assert!(
        d[2]["content"]
            .as_str()
            .expect("content")
            .contains("POST session/s1/element"),
        "the exchange that failed, not the evidence calls: {}",
        d[2]
    );
    assert!(artifacts.join("screenshot.png").is_file());
    assert_eq!(
        std::fs::read_to_string(artifacts.join("source.xml")).expect("source"),
        "<hierarchy><node/></hierarchy>"
    );
    drop_instance(h);
}

#[test]
fn an_element_name_missing_from_the_map_names_the_map() {
    let stub = shop_stub();
    let dir = tempfile::tempdir().expect("tempdir");
    let h = handle(&init("unknown-name", android(&stub.url, dir.path())));
    let r = dispatch(h, 0, &["basket"], &dir.path().join("a"));
    assert_eq!(r["status"], "fatal");
    let error = r["error"].as_str().expect("error");
    assert!(
        error.starts_with("no element \"basket\" in ") && error.ends_with("locators.yaml"),
        "{error}"
    );
    drop_instance(h);
}

#[test]
fn type_clear_and_read_round_trip_through_the_screen() {
    let stub = shop_stub();
    let dir = tempfile::tempdir().expect("tempdir");
    let h = handle(&init("type-read", android(&stub.url, dir.path())));
    let a = dir.path().join("a");
    assert_eq!(
        dispatch(h, 1, &["shoes", "search field"], &a)["status"],
        "passed"
    );
    assert!(stub.calls_to("/element/e1/value")[0].ends_with(r#"{"text":"shoes"}"#));
    let r = dispatch(h, 4, &["search field", "typed"], &a);
    assert_eq!(r["vars"]["typed"], "shoes", "{r}");
    assert_eq!(dispatch(h, 2, &["search field"], &a)["status"], "passed");
    assert_eq!(
        dispatch(h, 4, &["search field", "typed"], &a)["vars"]["typed"],
        ""
    );
    drop_instance(h);
}

#[test]
fn windows_reads_text_from_the_text_endpoint() {
    let stub = start_stub(StubState {
        elements: vec![StubElement {
            name: "Characters to copy:".into(),
            ..StubElement::new("name", "Search", "boots")
        }],
        ..Default::default()
    });
    let dir = tempfile::tempdir().expect("tempdir");
    let h = handle(&init("win-read", windows(&stub.url, dir.path())));
    let r = dispatch(h, 4, &["search field", "typed"], &dir.path().join("a"));
    assert_eq!(r["vars"]["typed"], "boots", "{r}");
    assert!(stub.calls_to("/attribute/Name").is_empty());
    drop_instance(h);
}

#[test]
fn an_attached_windows_session_is_neither_restarted_nor_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut top = windows("", dir.path());
    top.as_object_mut().expect("map").remove("app");
    top["capabilities"] = json!({"appium:appTopLevelWindow": "0x1234"});
    let mut root = windows("", dir.path());
    root["app"] = json!("Root");
    for (name, mut config) in [("attached-top", top), ("attached-root", root)] {
        let stub = start_stub(StubState::default());
        config["remote_url"] = json!(stub.url);
        let h = handle(&init(name, config));
        assert_eq!(reset(h)["ok"], true);
        assert_eq!(drop_instance(h)["ok"], true);
        assert!(stub.calls_to("/execute/sync").is_empty(), "{name}");
        assert_eq!(stub.calls_to("DELETE /session/s1").len(), 1, "{name}");
    }
}

#[test]
fn keys_map_per_platform_and_refuse_what_the_platform_lacks() {
    let stub = shop_stub();
    let dir = tempfile::tempdir().expect("tempdir");
    let a = dir.path().join("a");
    let h = handle(&init("keys-android", android(&stub.url, dir.path())));
    assert_eq!(dispatch(h, 3, &["back"], &a)["status"], "passed");
    let press = stub.calls_to("/execute/sync").pop().expect("a call");
    assert!(
        press.contains(r#""script":"mobile: pressKey""#) && press.contains(r#""keycode":4"#),
        "{press}"
    );
    let r = dispatch(h, 3, &["escape"], &a);
    assert_eq!(r["status"], "fatal");
    assert!(
        r["error"]
            .as_str()
            .expect("error")
            .contains("back, home, enter"),
        "{r}"
    );
    drop_instance(h);

    let win = start_stub(StubState::default());
    let w = handle(&init("keys-windows", windows(&win.url, dir.path())));
    assert_eq!(dispatch(w, 3, &["enter"], &a)["status"], "passed");
    let keys = win.calls_to("/execute/sync").pop().expect("a call");
    assert!(
        keys.contains(r#""script":"windows: keys""#) && keys.contains(r#""virtualKeyCode":13"#),
        "{keys}"
    );
    let r = dispatch(w, 3, &["back"], &a);
    assert!(
        r["error"]
            .as_str()
            .expect("error")
            .contains("no mapping on windows"),
        "{r}"
    );
    drop_instance(w);
}

#[test]
fn capture_and_dump_write_their_files() {
    let stub = shop_stub();
    let dir = tempfile::tempdir().expect("tempdir");
    let h = handle(&init("capture", android(&stub.url, dir.path())));
    let one = dir.path().join("one");
    assert_eq!(dispatch(h, 5, &[], &one)["status"], "passed");
    assert!(one.join("screenshot.png").is_file());
    assert!(!one.join("source.xml").exists());
    let two = dir.path().join("two");
    assert_eq!(dispatch(h, 6, &[], &two)["status"], "passed");
    assert!(two.join("screenshot.png").is_file() && two.join("source.xml").is_file());
    drop_instance(h);
}

#[test]
fn null_is_refused_before_anything_is_sent() {
    let stub = shop_stub();
    let dir = tempfile::tempdir().expect("tempdir");
    let h = handle(&init("null", android(&stub.url, dir.path())));
    let before = stub.calls().len();
    let r = dispatch(
        h,
        1,
        &["\u{0}__bddkit_null__\u{0}", "search field"],
        &dir.path().join("a"),
    );
    assert_eq!(r["status"], "fatal");
    assert!(
        r["error"]
            .as_str()
            .expect("error")
            .contains("no NULL on a screen"),
        "{r}"
    );
    assert_eq!(stub.calls().len(), before);
    drop_instance(h);
}

#[test]
fn a_failure_that_sent_nothing_shows_no_exchange_from_an_earlier_step() {
    let stub = shop_stub();
    let dir = tempfile::tempdir().expect("tempdir");
    let h = handle(&init("no-stale-exchange", android(&stub.url, dir.path())));
    let a = dir.path().join("a");
    assert_eq!(dispatch(h, 0, &["cart button"], &a)["status"], "passed");
    let r = dispatch(h, 0, &["basket"], &a);
    assert_eq!(r["status"], "fatal", "{r}");
    let d = r["diagnostics"].as_array().expect("diagnostics");
    assert!(d.iter().all(|x| x["kind"] != "http"), "{r}");
    assert_eq!(d[0]["kind"], "image");
    assert_eq!(d[1]["title"], "Page source");
    drop_instance(h);
}

#[test]
fn on_the_screen_is_one_look_and_answers_not_yet() {
    let stub = shop_stub();
    let dir = tempfile::tempdir().expect("tempdir");
    let mut config = android(&stub.url, dir.path());
    config["locators"] = json!(write_locators(
        dir.path(),
        "cart button: ~cart\nghost: ~ghost\n"
    ));
    config["find_timeout_secs"] = json!(5);
    let h = handle(&init("assert-on", config));
    let a = dir.path().join("a");
    assert_eq!(dispatch(h, 7, &["cart button"], &a)["status"], "passed");
    let started = std::time::Instant::now();
    let r = dispatch(h, 7, &["ghost"], &a);
    assert!(
        started.elapsed() < std::time::Duration::from_secs(2),
        "an assertion never waits find_timeout_secs"
    );
    assert_eq!(r["status"], "not_yet", "{r}");
    assert_eq!(
        r["diagnostics"][0]["kind"], "image",
        "not_yet carries evidence too"
    );
    stub.state.lock().expect("state").elements[0].displayed = false;
    let r = dispatch(h, 7, &["cart button"], &a);
    assert_eq!(r["status"], "not_yet");
    assert!(
        r["error"]
            .as_str()
            .expect("error")
            .contains("not displayed"),
        "{r}"
    );
    stub.state.lock().expect("state").elements[0].displayed = true;
    stub.state.lock().expect("state").stale_first = 1;
    assert_eq!(
        dispatch(h, 7, &["cart button"], &a)["status"],
        "not_yet",
        "stale is a changing screen"
    );
    drop_instance(h);
}

#[test]
fn not_on_the_screen_passes_on_missing_hidden_or_stale() {
    let stub = shop_stub();
    let dir = tempfile::tempdir().expect("tempdir");
    let mut config = android(&stub.url, dir.path());
    config["locators"] = json!(write_locators(
        dir.path(),
        "cart button: ~cart\nghost: ~ghost\n"
    ));
    let h = handle(&init("assert-off", config));
    let a = dir.path().join("a");
    assert_eq!(dispatch(h, 8, &["ghost"], &a)["status"], "passed");
    let r = dispatch(h, 8, &["cart button"], &a);
    assert_eq!(r["status"], "not_yet");
    assert!(
        r["error"]
            .as_str()
            .expect("error")
            .contains("still on the screen"),
        "{r}"
    );
    stub.state.lock().expect("state").elements[0].displayed = false;
    assert_eq!(dispatch(h, 8, &["cart button"], &a)["status"], "passed");
    stub.state.lock().expect("state").elements[0].displayed = true;
    stub.state.lock().expect("state").stale_first = 1;
    assert_eq!(
        dispatch(h, 8, &["cart button"], &a)["status"],
        "passed",
        "gone between find and read"
    );
    drop_instance(h);
}

#[test]
fn text_should_be_compares_exactly() {
    let stub = shop_stub();
    let dir = tempfile::tempdir().expect("tempdir");
    let h = handle(&init("assert-text", android(&stub.url, dir.path())));
    let a = dir.path().join("a");
    assert_eq!(
        dispatch(h, 9, &["cart button", "Cart (0)"], &a)["status"],
        "passed"
    );
    let r = dispatch(h, 9, &["cart button", "Cart"], &a);
    assert_eq!(r["status"], "not_yet");
    assert!(
        r["error"]
            .as_str()
            .expect("error")
            .contains(r#"is "Cart (0)", expected "Cart""#),
        "{r}"
    );
    assert_eq!(
        dispatch(h, 9, &["search field", ""], &a)["status"],
        "passed",
        "an empty text is a value"
    );
    drop_instance(h);
}

#[test]
fn a_protocol_error_in_an_assertion_is_fatal() {
    let stub = shop_stub();
    let dir = tempfile::tempdir().expect("tempdir");
    let h = handle(&init("assert-protocol", android(&stub.url, dir.path())));
    stub.state.lock().expect("state").session_gone = true;
    let r = dispatch(h, 7, &["cart button"], &dir.path().join("a"));
    assert_eq!(r["status"], "fatal", "{r}");
    assert!(
        r["error"]
            .as_str()
            .expect("error")
            .contains("invalid session id"),
        "{r}"
    );
    drop_instance(h);
}

#[test]
fn a_displayed_reply_of_the_wrong_type_is_fatal_not_false() {
    let stub = shop_stub();
    let dir = tempfile::tempdir().expect("tempdir");
    let h = handle(&init("assert-malformed", android(&stub.url, dir.path())));
    stub.state.lock().expect("state").malformed_displayed = true;
    let r = dispatch(h, 8, &["cart button"], &dir.path().join("a"));
    assert_eq!(
        r["status"], "fatal",
        "a reply that is not a bool must not read as hidden: {r}"
    );
    drop_instance(h);
}
