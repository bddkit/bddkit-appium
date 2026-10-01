//! The step table and the dispatch on its index — kept adjacent so they
//! cannot drift. The index of a step in `STEPS` is its identity.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};

use crate::config::Platform;
use crate::instance::Instance;
use crate::locators::Locator;
use crate::reply::{self, Ctx, Diagnostic};
use crate::webdriver::{Element, Error, Session};

pub struct Step {
    pub pattern: &'static str,
    pub kind: &'static str,
    pub description: &'static str,
}

pub const STEPS: &[Step] = &[
    Step {
        pattern: r#"^I tap "(?P<element>[^"]+)"$"#,
        kind: "action",
        description: "waits for the element to appear, then taps it",
    },
    Step {
        pattern: r#"^I type "(?P<text>[^"]*)" into "(?P<element>[^"]+)"$"#,
        kind: "action",
        description: "waits for the element, then types the text into it",
    },
    Step {
        pattern: r#"^I clear the "(?P<element>[^"]+)" field$"#,
        kind: "action",
        description: "waits for the element, then empties it",
    },
    Step {
        pattern: r#"^I press the "(?P<key>[^"]+)" key$"#,
        kind: "action",
        description: "presses back, home or enter on the device",
    },
    Step {
        pattern: r#"^I read the "(?P<element>[^"]+)" text from the screen as "(?P<name>[^"]+)"$"#,
        kind: "action",
        description: "waits for the element, then saves its text into a variable",
    },
    Step {
        pattern: r#"^I capture the screen$"#,
        kind: "action",
        description: "writes a PNG of the screen into the artifacts directory",
    },
    Step {
        pattern: r#"^I dump the screen$"#,
        kind: "action",
        description: "writes a PNG and the UI tree (XML) into the artifacts directory: what every element is called",
    },
    Step {
        pattern: r#"^the "(?P<element>[^"]+)" should be on the screen$"#,
        kind: "assertion",
        description: "the element is found and displayed",
    },
    Step {
        pattern: r#"^the "(?P<element>[^"]+)" should not be on the screen$"#,
        kind: "assertion",
        description: "the element is not found, or found and not displayed",
    },
    Step {
        pattern: r#"^the "(?P<element>[^"]+)" text on the screen should be "(?P<text>[^"]*)"$"#,
        kind: "assertion",
        description: "the element's text equals this exactly",
    },
];

pub fn steps_json() -> String {
    let steps: Vec<Value> = STEPS
        .iter()
        .map(|s| json!({"pattern": s.pattern, "group": "app", "kind": s.kind, "description": s.description}))
        .collect();
    Value::Array(steps).to_string()
}

pub struct Request {
    pub args: Vec<String>,
    pub ctx: Ctx,
}

impl Request {
    pub fn parse(v: &Value) -> Self {
        let args = v["args"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|x| x.as_str().unwrap_or("").to_string())
                    .collect()
            })
            .unwrap_or_default();
        Self {
            args,
            ctx: Ctx {
                artifacts_dir: v["artifacts_dir"].as_str().unwrap_or("").to_string(),
                debug: v["debug"].as_bool().unwrap_or(false),
            },
        }
    }
}

enum Fail {
    NotYet(String),
    Fatal(String),
}

fn fatal(error: impl Into<String>) -> Fail {
    Fail::Fatal(error.into())
}

fn not_yet(error: impl Into<String>) -> Fail {
    Fail::NotYet(error.into())
}

impl From<Error> for Fail {
    fn from(e: Error) -> Self {
        fatal(e.to_string())
    }
}

pub fn route(instance: &Instance, index: u32, req: &Request) -> String {
    instance.session.driver.set_debug(req.ctx.debug);
    instance.session.driver.clear_last();
    // `<<null>>` arrives as a string wrapped in NUL bytes, which no screen
    // text can contain.
    if req.args.iter().any(|a| a.contains('\0')) {
        return reply::fatal(
            "there is no NULL on a screen: <<null>> cannot be an argument of an app step",
            &[],
        );
    }
    match run(instance, index, req) {
        Ok(vars) if vars.is_empty() => reply::passed(),
        Ok(vars) => reply::passed_with(Value::Object(vars)),
        Err(fail) => {
            let diagnostics = evidence(instance, req);
            match fail {
                Fail::NotYet(error) => reply::not_yet(&error, &diagnostics),
                Fail::Fatal(error) => reply::fatal(&error, &diagnostics),
            }
        }
    }
}

/// What `I press the "<key>" key` sends: the extension and its argument.
pub fn key_command(platform: Platform, key: &str) -> Result<(&'static str, Value), String> {
    let android = |keycode: u32| Ok(("mobile: pressKey", json!({"keycode": keycode})));
    match (platform, key) {
        (Platform::Android, "back") => android(4),
        (Platform::Android, "home") => android(3),
        (Platform::Android, "enter") => android(66),
        (Platform::Windows, "enter") => Ok((
            "windows: keys",
            json!({"actions": [{"virtualKeyCode": 13}]}),
        )),
        (Platform::Windows, "back" | "home") => {
            Err(format!("key {key:?} has no mapping on {}", platform.name()))
        }
        _ => Err(format!(
            "unknown key {key:?}; known keys: back, home, enter"
        )),
    }
}

/// Writes one artifact, creating `artifacts_dir` on first use (the host
/// allocates the path but never creates it).
fn write_artifact(ctx: &Ctx, name: &str, bytes: &[u8]) -> Result<PathBuf, String> {
    std::fs::create_dir_all(&ctx.artifacts_dir)
        .map_err(|e| format!("creating {}: {e}", ctx.artifacts_dir))?;
    let path = Path::new(&ctx.artifacts_dir).join(name);
    std::fs::write(&path, bytes).map_err(|e| format!("writing {}: {e}", path.display()))?;
    if ctx.debug {
        eprintln!("[appium] wrote {}", path.display());
    }
    Ok(path)
}

fn locator<'a>(instance: &'a Instance, name: &str) -> Result<&'a Locator, Fail> {
    instance.config.locators.get(name).map_err(fatal)
}

/// An action's wait: look every 100 ms up to `timeout`. The one sleep in the
/// plugin — tapping a button that is about to appear is the ordinary case,
/// not the eventual one. Assertions never come here.
fn wait_for<'a>(
    session: &'a Session,
    l: &Locator,
    timeout: Duration,
) -> Result<Option<Element<'a>>, Error> {
    let deadline = Instant::now() + timeout;
    loop {
        // A busy UI tree counts as "not there yet"; if the deadline passes
        // while it still is, the server's own message is the failure.
        let busy = match session.find(l.strategy, &l.value) {
            Ok(Some(element)) => return Ok(Some(element)),
            Ok(None) => None,
            Err(e) if e.is_busy() => Some(e),
            Err(e) => return Err(e),
        };
        if Instant::now() >= deadline {
            return busy.map_or(Ok(None), Err);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// An action's element: wait up to `find_timeout`, then fail naming it.
fn act<'a>(instance: &'a Instance, name: &str) -> Result<Element<'a>, Fail> {
    let l = locator(instance, name)?;
    let timeout = instance.config.find_timeout;
    wait_for(&instance.session, l, timeout)?.ok_or_else(|| {
        fatal(format!(
            "no element {name:?} ({l}) on the screen within {}s",
            timeout.as_secs()
        ))
    })
}

/// An assertion's element: one look, no wait — the host polls.
fn look<'a>(instance: &'a Instance, name: &str) -> Result<Option<Element<'a>>, Fail> {
    let l = locator(instance, name)?;
    instance
        .session
        .find(l.strategy, &l.value)
        .map_err(|e| observe_err(&e))
}

/// An assertion's read: a stale element or a busy UI tree means the screen
/// is changing under the look, which the next attempt may settle.
fn observe<T>(r: Result<T, Error>) -> Result<T, Fail> {
    r.map_err(|e| observe_err(&e))
}

/// Stale or busy means the screen is not settled: not yet. Anything else is fatal.
fn observe_err(e: &Error) -> Fail {
    if e.is_stale() {
        not_yet(format!("the element changed while it was read: {e}"))
    } else if e.is_busy() {
        not_yet(e.to_string())
    } else {
        fatal(e.to_string())
    }
}

fn run(instance: &Instance, index: u32, req: &Request) -> Result<Map<String, Value>, Fail> {
    let s = &instance.session;
    let arg = |n: usize| req.args.get(n).cloned().unwrap_or_default();
    let mut vars = Map::new();
    match index {
        0 => act(instance, &arg(0))?.click()?,
        1 => act(instance, &arg(1))?.send_keys(&arg(0))?,
        2 => act(instance, &arg(0))?.clear()?,
        3 => {
            let (script, args) = key_command(instance.config.platform, &arg(0)).map_err(fatal)?;
            s.execute(script, vec![args])?;
        }
        4 => {
            let text = act(instance, &arg(0))?.text()?;
            vars.insert(arg(1), Value::String(text));
        }
        5 => {
            write_artifact(&req.ctx, "screenshot.png", &s.screenshot()?).map_err(fatal)?;
        }
        6 => {
            write_artifact(&req.ctx, "screenshot.png", &s.screenshot()?).map_err(fatal)?;
            write_artifact(&req.ctx, "source.xml", s.source()?.as_bytes()).map_err(fatal)?;
        }
        7 => match look(instance, &arg(0))? {
            None => return Err(not_yet(format!("no element {:?} on the screen", arg(0)))),
            Some(e) if !observe(e.displayed())? => {
                return Err(not_yet(format!(
                    "element {:?} is found but not displayed",
                    arg(0)
                )));
            }
            Some(_) => {}
        },
        8 => {
            if let Some(e) = look(instance, &arg(0))? {
                match e.displayed() {
                    // Gone between find and read: not on the screen.
                    Err(err) if err.is_stale() => {}
                    Err(err) => return Err(observe_err(&err)),
                    Ok(false) => {}
                    Ok(true) => {
                        return Err(not_yet(format!(
                            "element {:?} is still on the screen",
                            arg(0)
                        )));
                    }
                }
            }
        }
        9 => {
            let e = look(instance, &arg(0))?
                .ok_or_else(|| not_yet(format!("no element {:?} on the screen", arg(0))))?;
            let have = observe(e.text())?;
            if have != arg(1) {
                return Err(not_yet(format!(
                    "the text of {:?} is {:?}, expected {:?}",
                    arg(0),
                    have,
                    arg(1)
                )));
            }
        }
        _ => return Err(fatal(format!("no step at index {index}"))),
    }
    Ok(vars)
}

/// One piece of evidence: the capture written to `name` and shown by its
/// path, or a text line saying why it could not be taken.
fn captured(
    ctx: &Ctx,
    title: &str,
    name: &str,
    bytes: Result<Vec<u8>, Error>,
    by_path: fn(String, String) -> Diagnostic,
) -> Diagnostic {
    match bytes
        .map_err(|e| e.to_string())
        .and_then(|b| write_artifact(ctx, name, &b))
    {
        Ok(path) => by_path(title.to_string(), path.display().to_string()),
        Err(e) => Diagnostic::text(title, format!("not taken: {e}")),
    }
}

/// What a failed step attaches. The last WebDriver exchange is taken first,
/// before the evidence calls below replace it.
// ponytail: every not_yet attempt pays for a screenshot, so polling on a
// device runs slower than interval_ms; an option to skip evidence on not_yet
// if that ever bites.
fn evidence(instance: &Instance, req: &Request) -> Vec<Diagnostic> {
    let s = &instance.session;
    let last = s.driver.last_exchange();
    let mut out = vec![
        captured(
            &req.ctx,
            "Screenshot",
            "screenshot.png",
            s.screenshot(),
            Diagnostic::image,
        ),
        captured(
            &req.ctx,
            "Page source",
            "source.xml",
            s.source().map(String::into_bytes),
            Diagnostic::text_file,
        ),
    ];
    if let Some(exchange) = last {
        out.push(Diagnostic::http(
            "Last WebDriver exchange",
            exchange.render(),
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_per_platform() {
        assert_eq!(
            key_command(Platform::Android, "home").expect("home").1,
            json!({"keycode": 3})
        );
        assert_eq!(
            key_command(Platform::Android, "enter").expect("enter").1,
            json!({"keycode": 66})
        );
        assert!(
            key_command(Platform::Windows, "home")
                .expect_err("no home")
                .contains("no mapping on windows")
        );
    }
}
