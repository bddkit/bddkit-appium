//! A fake Appium server: enough of the WebDriver wire and the `mobile:` /
//! `windows:` extensions to pin the plugin's side of the protocol without a
//! device, plus the helpers that call the exports. Shared by `stub.rs`
//! (exports in-process) and `e2e.rs` (the real bddkit binary).
#![allow(dead_code)] // each test binary uses a different part of it

use std::ffi::{CStr, CString, c_char};
use std::path::Path;
use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::{Method, StatusCode, Uri};
use axum::{Json, Router};
use serde_json::{Value, json};

pub const ELEMENT_KEY: &str = "element-6066-11e4-a52e-4f735466cecf";
/// A 1×1 transparent PNG, base64.
const PNG_1X1: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";

#[derive(Clone, Default)]
pub struct StubElement {
    /// Found when a `find` sends exactly this strategy and value.
    pub using: String,
    pub value: String,
    /// What `text` answers; `clear` empties it, `value` appends.
    pub text: String,
    /// What `attribute/Name` answers: the control's label, apart from its text.
    pub name: String,
    pub displayed: bool,
    /// `find` answers "no such element" for this element this many times first.
    pub absent_for: u32,
}

impl StubElement {
    pub fn new(using: &str, value: &str, text: &str) -> Self {
        Self {
            using: using.into(),
            value: value.into(),
            text: text.into(),
            name: String::new(),
            displayed: true,
            absent_for: 0,
        }
    }
}

#[derive(Default)]
pub struct StubState {
    /// `METHOD /path body`, in order.
    pub calls: Vec<String>,
    pub elements: Vec<StubElement>,
    /// Element reads answer "stale element reference" this many times first.
    pub stale_first: u32,
    /// Merged into the capabilities `POST /session` answers with.
    pub session_caps: Map,
    /// What `mobile: getCurrentPackage` answers; `None` is an error.
    pub current_package: Option<String>,
    /// `GET /status` answers `ready: false`.
    pub not_ready: bool,
    /// Element reads answer "invalid session id" (a protocol error that is
    /// neither missing nor stale).
    pub session_gone: bool,
    /// `GET .../displayed` answers a string instead of a boolean.
    pub malformed_displayed: bool,
    /// `POST .../element` answers `500 unknown error` (UiAutomator2 while it
    /// cannot read the UI tree) this many times first.
    pub busy_first: u32,
    /// Element reads (`GET .../element/<id>/...`) answer the same busy error
    /// this many times first.
    pub busy_reads: u32,
}

pub type Map = serde_json::Map<String, Value>;
type Shared = Arc<Mutex<StubState>>;

pub struct Stub {
    pub url: String,
    pub state: Shared,
    _rt: tokio::runtime::Runtime,
}

impl Stub {
    pub fn calls(&self) -> Vec<String> {
        self.state.lock().expect("state").calls.clone()
    }

    /// The calls whose `METHOD /path` part contains `needle`.
    pub fn calls_to(&self, needle: &str) -> Vec<String> {
        self.calls()
            .into_iter()
            .filter(|c| c.contains(needle))
            .collect()
    }
}

pub fn start_stub(state: StubState) -> Stub {
    let shared: Shared = Arc::new(Mutex::new(state));
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let listener = rt
        .block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    let app = Router::new()
        .fallback(handle)
        .with_state(Arc::clone(&shared));
    rt.spawn(async move { axum::serve(listener, app).await.expect("serve") });
    Stub {
        url: format!("http://{addr}"),
        state: shared,
        _rt: rt,
    }
}

fn reply(v: Value) -> (StatusCode, Json<Value>) {
    (StatusCode::OK, Json(json!({"value": v})))
}

fn error(status: StatusCode, error: &str, message: String) -> (StatusCode, Json<Value>) {
    (
        status,
        Json(json!({"value": {"error": error, "message": message, "stacktrace": ""}})),
    )
}

async fn handle(
    State(state): State<Shared>,
    method: Method,
    uri: Uri,
    body: String,
) -> (StatusCode, Json<Value>) {
    let mut st = state.lock().expect("stub state");
    let path = uri.path().to_string();
    st.calls
        .push(format!("{method} {path} {body}").trim().to_string());
    let segments: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    let req: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
    match (method.as_str(), segments.as_slice()) {
        ("GET", ["status"]) => reply(json!({"ready": !st.not_ready, "message": "stub"})),
        ("POST", ["session"]) => {
            let mut caps = req["capabilities"]["alwaysMatch"]
                .as_object()
                .cloned()
                .unwrap_or_default();
            caps.extend(st.session_caps.clone());
            reply(json!({"sessionId": "s1", "capabilities": caps}))
        }
        ("DELETE", ["session", "s1"]) => reply(Value::Null),
        ("POST", ["session", "s1", "execute", "sync"]) => {
            match req["script"].as_str().unwrap_or("") {
                "mobile: getCurrentPackage" => match &st.current_package {
                    Some(p) => reply(json!(p)),
                    None => error(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "unknown error",
                        "stub: no package".into(),
                    ),
                },
                _ => reply(Value::Null),
            }
        }
        ("GET", ["session", "s1", "screenshot"]) => reply(json!(PNG_1X1)),
        ("GET", ["session", "s1", "source"]) => reply(json!("<hierarchy><node/></hierarchy>")),
        ("POST", ["session", "s1", "element"]) if st.busy_first > 0 => {
            st.busy_first -= 1;
            error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "unknown error",
                "Timed out after 10071ms waiting for the root AccessibilityNodeInfo".into(),
            )
        }
        ("POST", ["session", "s1", "element"]) => {
            let using = req["using"].as_str().unwrap_or("");
            let value = req["value"].as_str().unwrap_or("");
            match st
                .elements
                .iter()
                .position(|e| e.using == using && e.value == value)
            {
                Some(i) if st.elements[i].absent_for > 0 => {
                    st.elements[i].absent_for -= 1;
                    error(
                        StatusCode::NOT_FOUND,
                        "no such element",
                        "stub: not yet".into(),
                    )
                }
                Some(i) => reply(json!({ELEMENT_KEY: format!("e{i}")})),
                None => error(
                    StatusCode::NOT_FOUND,
                    "no such element",
                    format!("nothing matches {using} {value}"),
                ),
            }
        }
        (_, ["session", "s1", "element", id, rest @ ..]) => {
            let Some(i) = id.strip_prefix('e').and_then(|n| n.parse::<usize>().ok()) else {
                return error(StatusCode::NOT_FOUND, "no such element", id.to_string());
            };
            if method == Method::GET && st.stale_first > 0 {
                st.stale_first -= 1;
                return error(
                    StatusCode::NOT_FOUND,
                    "stale element reference",
                    format!("e{i}"),
                );
            }
            if method == Method::GET && st.busy_reads > 0 {
                st.busy_reads -= 1;
                return error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "unknown error",
                    "Timed out after 10071ms waiting for the root AccessibilityNodeInfo".into(),
                );
            }
            if st.session_gone {
                return error(
                    StatusCode::NOT_FOUND,
                    "invalid session id",
                    "stub: gone".into(),
                );
            }
            let st_malformed = st.malformed_displayed;
            let e = &mut st.elements[i];
            match (method.as_str(), rest) {
                ("POST", ["click"]) => reply(Value::Null),
                ("POST", ["clear"]) => {
                    e.text.clear();
                    reply(Value::Null)
                }
                ("POST", ["value"]) => {
                    e.text.push_str(req["text"].as_str().unwrap_or(""));
                    reply(Value::Null)
                }
                ("GET", ["text"]) => reply(json!(e.text)),
                ("GET", ["attribute", "Name"]) => reply(json!(e.name)),
                ("GET", ["displayed"]) if st_malformed => reply(json!("yes")),
                ("GET", ["displayed"]) => reply(json!(e.displayed)),
                _ => error(
                    StatusCode::NOT_FOUND,
                    "unknown command",
                    format!("{method} {path}"),
                ),
            }
        }
        _ => error(
            StatusCode::NOT_FOUND,
            "unknown command",
            format!("{method} {path}"),
        ),
    }
}

/// Reads a reply the plugin allocated, frees it through the plugin, parses it.
pub fn take(ptr: *mut c_char) -> Value {
    assert!(!ptr.is_null(), "the plugin returned NULL");
    let text = unsafe { CStr::from_ptr(ptr) }
        .to_string_lossy()
        .into_owned();
    unsafe { bddkit_appium::bddkit_free_string(ptr) };
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("not JSON ({e}): {text}"))
}

pub fn envelope(f: extern "C" fn(*const c_char) -> *mut c_char, request: &Value) -> Value {
    let c = CString::new(request.to_string()).expect("no NUL in a JSON request");
    take(f(c.as_ptr()))
}

pub fn instance_request(name: &str, config: Value) -> Value {
    json!({"group": "app", "instance": name, "config": config, "options": {}})
}

pub fn write_locators(dir: &Path, yaml: &str) -> String {
    let path = dir.join("locators.yaml");
    std::fs::write(&path, yaml).expect("write locators");
    path.display().to_string()
}
