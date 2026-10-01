//! One Appium session for one feature file: opened on the file's first app
//! step, the app restarted inside it between scenarios, closed when the file
//! ends.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex, PoisonError};

use serde_json::{Value, json};

use crate::config::{InstanceConfig, Platform, Reset};
use crate::webdriver::{Driver, Session};

/// Instance names that have a live session. A device runs one app session at
/// a time, so a second feature file on the same instance is refused loudly
/// instead of fighting the first over the screen.
static LIVE: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

/// Holding one is owning the instance's device; dropping it frees the name,
/// on every path — a failed open, a close, a panicking file's sweep.
struct Claim(String);

impl Claim {
    fn take(name: &str) -> Result<Self, String> {
        let mut live = LIVE.lock().unwrap_or_else(PoisonError::into_inner);
        if !live.insert(name.to_string()) {
            return Err(format!(
                "instance {name:?} already has a live session on its device; feature files that use it must run one after another — put them in one @serial chain, e.g. @serial({name})"
            ));
        }
        Ok(Self(name.to_string()))
    }
}

impl Drop for Claim {
    fn drop(&mut self) {
        LIVE.lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.0);
    }
}

pub struct Instance {
    pub config: InstanceConfig,
    pub session: Session,
    /// The Android package reset clears and relaunches; empty on Windows.
    app_id: String,
    _claim: Claim,
}

impl Instance {
    pub fn open(name: &str, config: InstanceConfig) -> Result<Self, String> {
        let claim = Claim::take(name)?;
        let driver = Arc::new(Driver::new(config.remote_url.clone(), false));
        let (session, caps) = driver
            .new_session(&config.session_capabilities())
            .map_err(|e| {
                format!(
                    "cannot open a {} session at {}: {e}",
                    config.platform.name(),
                    config.remote_url
                )
            })?;
        let app_id = match config.platform {
            Platform::Windows => String::new(),
            Platform::Android => match android_app_id(&session, &caps) {
                Ok(id) => id,
                Err(e) => {
                    let _ = session.delete();
                    return Err(e);
                }
            },
        };
        Ok(Self {
            config,
            session,
            app_id,
            _claim: claim,
        })
    }

    fn extension(&self, script: &str, args: Value) -> Result<(), String> {
        self.session
            .execute(script, vec![args])
            .map(drop)
            .map_err(|e| format!("{script}: {e}"))
    }

    /// A Windows session attached to a window it did not launch (`app: Root`
    /// or `appium:appTopLevelWindow`): restarting or closing it would hit
    /// whatever else the driver can reach.
    fn attached(&self) -> bool {
        self.config.platform == Platform::Windows
            && (self.config.app.as_deref() == Some("Root")
                || self
                    .config
                    .capabilities
                    .contains_key("appium:appTopLevelWindow"))
    }

    /// `HttpState::reset` for an app: the session stays, the app restarts.
    pub fn reset(&self) -> Result<(), String> {
        if self.attached() {
            return Ok(());
        }
        match self.config.platform {
            Platform::Android => {
                let first = match self.config.reset {
                    Reset::Clear => "mobile: clearApp",
                    Reset::Relaunch => "mobile: terminateApp",
                };
                self.extension(first, json!({"appId": self.app_id}))?;
                self.extension("mobile: activateApp", json!({"appId": self.app_id}))
            }
            Platform::Windows => {
                self.extension("windows: closeApp", json!({}))?;
                self.extension("windows: launchApp", json!({}))
            }
        }
    }

    pub fn close(&self) -> Result<(), String> {
        if self.config.platform == Platform::Windows && !self.attached() {
            // The driver documents that deleting the session leaves the
            // application running; without this every run leaves a window.
            let _ = self.extension("windows: closeApp", json!({}));
        }
        self.session
            .delete()
            .map_err(|e| format!("closing the session: {e}"))
    }

    /// `doctor --live`: is the server there and ready. Opens no session.
    pub fn probe(config: &InstanceConfig) -> Result<(), String> {
        let url = &config.remote_url;
        let status = Driver::new(url.clone(), false)
            .status()
            .map_err(|e| format!("Appium at {url} does not answer: {e}"))?;
        if status["ready"] == Value::Bool(false) {
            return Err(format!(
                "Appium at {url} answers but is not ready: {}",
                status["message"].as_str().unwrap_or("")
            ));
        }
        Ok(())
    }
}

/// The package the session runs: from the capabilities the server returned,
/// else asked of the device.
fn android_app_id(session: &Session, caps: &Value) -> Result<String, String> {
    for key in ["appPackage", "appium:appPackage"] {
        if let Some(id) = caps[key].as_str().filter(|s| !s.is_empty()) {
            return Ok(id.to_string());
        }
    }
    match session.execute("mobile: getCurrentPackage", vec![]) {
        Ok(Value::String(id)) if !id.is_empty() => Ok(id),
        got => {
            let got = match got {
                Ok(other) => format!(" answered {other}"),
                Err(e) => format!(": {e}"),
            };
            Err(format!(
                "cannot tell which package the session runs (mobile: getCurrentPackage{got}); set \"appium:appPackage\" in capabilities"
            ))
        }
    }
}
