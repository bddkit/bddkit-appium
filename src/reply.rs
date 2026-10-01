//! Every reply the plugin sends is built here, so the shape the host parses
//! has one author.

use serde_json::{Value, json};

/// The per-dispatch context the host hands over with every step.
pub struct Ctx {
    pub artifacts_dir: String,
    pub debug: bool,
}

/// One piece of evidence in a failure dump. `kind` is what the host renders
/// in the `--- <title> (<kind>) ---` header.
#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub title: String,
    pub kind: &'static str,
    pub content: Option<String>,
    pub path: Option<String>,
}

impl Diagnostic {
    pub fn text(title: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            kind: "text",
            content: Some(content.into()),
            path: None,
        }
    }

    /// A text file on disk, shown by its path.
    pub fn text_file(title: impl Into<String>, path: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            kind: "text",
            content: None,
            path: Some(path.into()),
        }
    }

    pub fn http(title: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            kind: "http",
            content: Some(content.into()),
            path: None,
        }
    }

    pub fn image(title: impl Into<String>, path: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            kind: "image",
            content: None,
            path: Some(path.into()),
        }
    }

    fn to_json(&self) -> Value {
        json!({"title": self.title, "kind": self.kind, "content": self.content, "path": self.path})
    }
}

fn diagnostics_json(diagnostics: &[Diagnostic]) -> Value {
    Value::Array(diagnostics.iter().map(Diagnostic::to_json).collect())
}

/// The envelope shared by validate / init / drop / reset / probe.
pub fn ok() -> String {
    r#"{"ok":true}"#.to_string()
}

pub fn err(error: impl Into<String>) -> String {
    json!({"ok": false, "error": error.into()}).to_string()
}

/// `ok()` or `err(..)` from a result.
pub fn envelope(result: Result<(), String>) -> String {
    result.map_or_else(err, |()| ok())
}

pub fn passed() -> String {
    r#"{"status":"passed"}"#.to_string()
}

pub fn passed_with(vars: Value) -> String {
    json!({"status": "passed", "vars": vars}).to_string()
}

/// One fresh observation says the condition is not met yet. Only an
/// assertion may answer this.
pub fn not_yet(error: &str, diagnostics: &[Diagnostic]) -> String {
    json!({"status": "not_yet", "error": error, "diagnostics": diagnostics_json(diagnostics)})
        .to_string()
}

/// The observation itself failed; retrying cannot help.
pub fn fatal(error: &str, diagnostics: &[Diagnostic]) -> String {
    json!({"status": "fatal", "error": error, "diagnostics": diagnostics_json(diagnostics)})
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_passed_reply_carries_vars_and_nothing_else() {
        let v: Value = serde_json::from_str(&passed_with(json!({"typed": "shoes"}))).expect("JSON");
        assert_eq!(v["status"], "passed");
        assert_eq!(v["vars"]["typed"], "shoes");
        assert!(v.get("diagnostics").is_none());
    }

    #[test]
    fn a_failure_renders_every_diagnostic_kind() {
        let diagnostics = [
            Diagnostic::image("Screenshot", "/tmp/a/screenshot.png"),
            Diagnostic::text_file("Page source", "/tmp/a/source.xml"),
            Diagnostic::http("Last WebDriver exchange", "GET session/s1/source"),
            Diagnostic::text("Screenshot", "not taken: gone"),
        ];
        let v: Value = serde_json::from_str(&fatal("boom", &diagnostics)).expect("JSON");
        assert_eq!(v["status"], "fatal");
        assert_eq!(v["error"], "boom");
        assert_eq!(v["diagnostics"][0]["kind"], "image");
        assert!(v["diagnostics"][0]["content"].is_null());
        assert_eq!(v["diagnostics"][1]["kind"], "text");
        assert_eq!(v["diagnostics"][1]["path"], "/tmp/a/source.xml");
        assert_eq!(v["diagnostics"][2]["kind"], "http");
        assert_eq!(v["diagnostics"][3]["content"], "not taken: gone");
    }

    #[test]
    fn not_yet_and_envelopes_have_the_documented_shape() {
        let v: Value = serde_json::from_str(&not_yet("still loading", &[])).expect("JSON");
        assert_eq!(v["status"], "not_yet");
        assert_eq!(v["diagnostics"], json!([]));
        assert_eq!(ok(), r#"{"ok":true}"#);
        let e: Value = serde_json::from_str(&err("bad")).expect("JSON");
        assert_eq!(e["ok"], false);
        assert_eq!(e["error"], "bad");
    }
}
