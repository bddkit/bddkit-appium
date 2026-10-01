//! WebDriver over blocking HTTP, as Appium speaks it. Knows endpoints and the
//! wire shape, nothing about steps. Trimmed from bddkit-browser's module of
//! the same name: no CSS, cookies or navigation; Appium's locator strategies,
//! the page source, and `execute` for the `mobile:` / `windows:` extensions.

use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use serde_json::{Value, json};
use url::Url;

/// The W3C element identifier key.
pub const ELEMENT_KEY: &str = "element-6066-11e4-a52e-4f735466cecf";

/// Bodies are cut at this many bytes in evidence: enough to read an error,
/// small enough that a dump stays readable.
pub const MAX_BODY: usize = 4 * 1024;

#[derive(Debug, Clone)]
pub struct Exchange {
    /// `METHOD path`, path relative to the endpoint.
    pub request: String,
    pub request_body: String,
    pub status: u16,
    pub response_body: String,
}

fn truncate(s: &str) -> String {
    if s.len() <= MAX_BODY {
        return s.to_string();
    }
    let mut end = MAX_BODY;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}… truncated", &s[..end])
}

impl Exchange {
    pub fn render(&self) -> String {
        format!(
            "{}\n{}\n\nstatus: {}\n{}",
            self.request,
            truncate(&self.request_body),
            self.status,
            truncate(&self.response_body)
        )
    }
}

#[derive(Debug)]
pub enum Error {
    /// The request never got a WebDriver reply: refused, timed out, not JSON.
    Transport(String),
    /// The server answered with an error document.
    Protocol {
        status: u16,
        error: String,
        message: String,
    },
}

impl Error {
    pub fn is_no_such_element(&self) -> bool {
        matches!(self, Self::Protocol { error, .. } if error == "no such element")
    }

    /// UiAutomator2's answer while the UI tree cannot be read (typically right
    /// after the app restarts): the screen is still being built, not broken.
    /// It matches any WebDriver "unknown error", not only that timeout.
    pub fn is_busy(&self) -> bool {
        matches!(self, Self::Protocol { error, .. } if error == "unknown error")
    }

    /// The element left the tree between the find and this call.
    pub fn is_stale(&self) -> bool {
        matches!(self, Self::Protocol { error, .. } if error == "stale element reference")
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport(m) => write!(f, "{m}"),
            Self::Protocol {
                status,
                error,
                message,
            } => write!(f, "{error} ({status}): {message}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strategy {
    AccessibilityId,
    Id,
    Name,
    XPath,
}

impl Strategy {
    fn wire(self) -> &'static str {
        match self {
            Self::AccessibilityId => "accessibility id",
            Self::Id => "id",
            Self::Name => "name",
            Self::XPath => "xpath",
        }
    }
}

pub struct Driver {
    agent: ureq::Agent,
    base: Url,
    last: Mutex<Option<Exchange>>,
    debug: AtomicBool,
}

pub struct Session {
    pub driver: Arc<Driver>,
    pub id: String,
}

pub struct Element<'a> {
    session: &'a Session,
    pub id: String,
}

impl Driver {
    pub fn new(mut base: Url, debug: bool) -> Self {
        // `Url::join` treats the last segment as a file unless the path ends
        // with `/`; an Appium at `/wd/hub` would otherwise lose `hub`.
        if !base.path().ends_with('/') {
            let path = format!("{}/", base.path());
            base.set_path(&path);
        }
        let config = ureq::Agent::config_builder()
            // A 404 carries `no such element`; the body is the answer.
            .http_status_as_error(false)
            // ponytail: one global timeout; a first Android session installs the
            // server and the app and can take minutes. Per-call timeouts if a hung
            // find ever needs to fail faster than this.
            .timeout_global(Some(Duration::from_secs(300)))
            .build();
        Self {
            agent: config.new_agent(),
            base,
            last: Mutex::new(None),
            debug: AtomicBool::new(debug),
        }
    }

    /// `debug` follows the request, not the init: `steps::route` calls this
    /// on every dispatch before running the step.
    pub fn set_debug(&self, on: bool) {
        self.debug.store(on, Ordering::Relaxed);
    }

    fn endpoint(&self, path: &str) -> Url {
        self.base.join(path).unwrap_or_else(|_| self.base.clone())
    }

    pub fn last_exchange(&self) -> Option<Exchange> {
        self.last.lock().ok().and_then(|g| g.clone())
    }

    /// Forgets the recorded exchange, so a step that sends nothing cannot
    /// show an earlier step's.
    pub fn clear_last(&self) {
        if let Ok(mut g) = self.last.lock() {
            *g = None;
        }
    }

    fn record(&self, exchange: Exchange) {
        if let Ok(mut g) = self.last.lock() {
            *g = Some(exchange);
        }
    }

    /// Interprets one reply. Separate from the I/O so it is unit-testable.
    fn decode(method: &str, path: &str, status: u16, body: &str) -> Result<Value, Error> {
        let mut parsed: Value = serde_json::from_str(body).map_err(|e| {
            Error::Transport(format!(
                "{method} {path}: status {status} with a non-JSON body: {e}"
            ))
        })?;
        if status != 200 {
            return Err(Error::Protocol {
                status,
                error: parsed["value"]["error"]
                    .as_str()
                    .unwrap_or("unknown error")
                    .to_string(),
                message: parsed["value"]["message"]
                    .as_str()
                    .unwrap_or("")
                    .to_string(),
            });
        }
        Ok(parsed["value"].take())
    }

    fn call(&self, method: &str, path: &str, body: Option<&Value>) -> Result<Value, Error> {
        let url = self.endpoint(path);
        let request_body = body.map(Value::to_string).unwrap_or_default();
        let sent = match method {
            "GET" => self.agent.get(url.as_str()).call(),
            "DELETE" => self.agent.delete(url.as_str()).call(),
            // WebDriver requires a JSON object body on every POST, sent as
            // compact bytes so the recorded body is what went over the wire.
            _ => {
                let compact = body.cloned().unwrap_or_else(|| json!({})).to_string();
                self.agent
                    .post(url.as_str())
                    .content_type("application/json")
                    .send(compact)
            }
        };
        // (status, body) on a reply; (status, message) when there was none.
        let outcome = sent
            .map_err(|e| (0, format!("{method} {url}: {e}")))
            .and_then(|mut response| {
                let status = response.status().as_u16();
                response
                    .body_mut()
                    .read_to_string()
                    .map(|text| (status, text))
                    .map_err(|e| (status, format!("{method} {url}: reading the reply: {e}")))
            });
        let (status, text) = match &outcome {
            Ok(reply) | Err(reply) => reply.clone(),
        };
        if outcome.is_ok() && self.debug.load(Ordering::Relaxed) {
            eprintln!("[appium] {method} /{path} {status}");
        }
        self.record(Exchange {
            request: format!("{method} {path}"),
            request_body,
            status,
            response_body: text,
        });
        match outcome {
            Ok((status, text)) => Self::decode(method, path, status, &text),
            Err((_, message)) => Err(Error::Transport(message)),
        }
    }

    pub fn status(&self) -> Result<Value, Error> {
        self.call("GET", "status", None)
    }

    /// The session and the capabilities the server says it opened it with.
    pub fn new_session(self: &Arc<Self>, body: &Value) -> Result<(Session, Value), Error> {
        let mut value = self.call("POST", "session", Some(body))?;
        let id = value["sessionId"]
            .as_str()
            .ok_or_else(|| {
                Error::Transport("POST session: the reply carries no sessionId".to_string())
            })?
            .to_string();
        Ok((
            Session {
                driver: Arc::clone(self),
                id,
            },
            value["capabilities"].take(),
        ))
    }
}

fn element_id(value: &Value) -> Result<String, Error> {
    value[ELEMENT_KEY]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| Error::Transport(format!("an element reply without {ELEMENT_KEY}: {value}")))
}

impl Session {
    fn path(&self, tail: &str) -> String {
        format!("session/{}/{tail}", self.id)
    }

    fn post(&self, tail: &str, body: Value) -> Result<Value, Error> {
        self.driver.call("POST", &self.path(tail), Some(&body))
    }

    fn get(&self, tail: &str) -> Result<Value, Error> {
        self.driver.call("GET", &self.path(tail), None)
    }

    /// `Ok(None)` on `no such element`; every other error is passed up.
    pub fn find(&self, using: Strategy, value: &str) -> Result<Option<Element<'_>>, Error> {
        match self.post("element", json!({"using": using.wire(), "value": value})) {
            Ok(v) => Ok(Some(Element {
                session: self,
                id: element_id(&v)?,
            })),
            Err(e) if e.is_no_such_element() => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// `mobile: …` / `windows: …` extensions travel as a script name.
    pub fn execute(&self, script: &str, args: Vec<Value>) -> Result<Value, Error> {
        self.post("execute/sync", json!({"script": script, "args": args}))
    }

    pub fn screenshot(&self) -> Result<Vec<u8>, Error> {
        let encoded = self.get("screenshot")?;
        base64::engine::general_purpose::STANDARD
            .decode(encoded.as_str().unwrap_or(""))
            .map_err(|e| Error::Transport(format!("the screenshot is not base64: {e}")))
    }

    /// The UI tree as XML: what every element on the screen is called.
    pub fn source(&self) -> Result<String, Error> {
        Ok(self.get("source")?.as_str().unwrap_or("").to_string())
    }

    pub fn delete(&self) -> Result<(), Error> {
        self.driver
            .call("DELETE", &format!("session/{}", self.id), None)
            .map(drop)
    }
}

impl Element<'_> {
    fn tail(&self, what: &str) -> String {
        format!("element/{}/{what}", self.id)
    }

    pub fn click(&self) -> Result<(), Error> {
        self.session.post(&self.tail("click"), json!({})).map(drop)
    }

    pub fn clear(&self) -> Result<(), Error> {
        self.session.post(&self.tail("clear"), json!({})).map(drop)
    }

    pub fn send_keys(&self, text: &str) -> Result<(), Error> {
        self.session
            .post(&self.tail("value"), json!({"text": text}))
            .map(drop)
    }

    /// What the driver's text endpoint renders: on Android the `text`
    /// attribute; on Windows the value of an editable field, otherwise the
    /// control's Name.
    pub fn text(&self) -> Result<String, Error> {
        let v = self.session.get(&self.tail("text"))?;
        v.as_str()
            .map(str::to_string)
            .ok_or_else(|| Error::Transport(format!("the text reply is not a string: {v}")))
    }

    pub fn displayed(&self) -> Result<bool, Error> {
        let v = self.session.get(&self.tail("displayed"))?;
        v.as_bool()
            .ok_or_else(|| Error::Transport(format!("the displayed reply is not a boolean: {v}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_base_without_a_trailing_slash_still_joins_paths_under_it() {
        let d = Driver::new(Url::parse("http://h:4723/wd/hub").expect("url"), false);
        assert_eq!(d.endpoint("status").as_str(), "http://h:4723/wd/hub/status");
        let d = Driver::new(Url::parse("http://h:4723").expect("url"), false);
        assert_eq!(
            d.endpoint("session/s1/source").as_str(),
            "http://h:4723/session/s1/source"
        );
    }

    #[test]
    fn error_bodies_become_protocol_errors_and_are_classified() {
        let missing = r#"{"value":{"error":"no such element","message":"nothing at ~cart"}}"#;
        let e = Driver::decode("POST", "session/s1/element", 404, missing).expect_err("404");
        assert!(e.is_no_such_element() && !e.is_stale());
        assert!(e.to_string().contains("nothing at ~cart"), "{e}");
        let stale = r#"{"value":{"error":"stale element reference","message":"gone"}}"#;
        let e = Driver::decode("GET", "session/s1/element/e0/text", 404, stale).expect_err("404");
        assert!(e.is_stale() && !e.is_no_such_element());
        let ok =
            Driver::decode("GET", "session/s1/source", 200, r#"{"value":"<x/>"}"#).expect("200");
        assert_eq!(ok, json!("<x/>"));
        let bad = Driver::decode("GET", "status", 200, "<html>").expect_err("HTML is not a reply");
        assert!(matches!(bad, Error::Transport(_)));
    }

    #[test]
    fn strategies_use_appium_wire_names() {
        assert_eq!(Strategy::AccessibilityId.wire(), "accessibility id");
        assert_eq!(Strategy::Id.wire(), "id");
        assert_eq!(Strategy::Name.wire(), "name");
        assert_eq!(Strategy::XPath.wire(), "xpath");
    }

    #[test]
    fn a_refused_connection_still_records_an_exchange() {
        // Port 1 on loopback: nothing listens there.
        let d = Driver::new(Url::parse("http://127.0.0.1:1").expect("url"), false);
        let err = d.status().expect_err("nothing listens on port 1");
        assert!(matches!(err, Error::Transport(_)), "{err}");
        let exchange = d.last_exchange().expect("an exchange was recorded");
        assert_eq!(exchange.status, 0);
        assert_eq!(exchange.request, "GET status");
    }

    #[test]
    fn an_exchange_renders_truncated_bodies() {
        let ex = Exchange {
            request: "POST session/s1/element".into(),
            request_body: "x".repeat(MAX_BODY + 10),
            status: 200,
            response_body: String::new(),
        };
        let rendered = ex.render();
        assert!(rendered.contains("POST session/s1/element"));
        assert!(rendered.contains("… truncated"));
        assert!(!rendered.contains(&"x".repeat(MAX_BODY + 1)));
    }
}
