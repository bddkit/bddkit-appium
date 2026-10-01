//! The instance body from `resources.app.<name>`. The host has no schema for
//! it and never looks inside, so every check a typo could trip is here.
//! Shape only: nothing in this module opens a socket.

use std::path::Path;
use std::time::Duration;

use serde_json::{Map, Value, json};
use url::Url;

use crate::locators::Locators;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Android,
    Windows,
}

impl Platform {
    const IMPLEMENTED: &'static str = "android, windows";

    fn parse(s: &str) -> Option<Self> {
        match s {
            "android" => Some(Self::Android),
            "windows" => Some(Self::Windows),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Android => "android",
            Self::Windows => "windows",
        }
    }

    fn platform_name(self) -> &'static str {
        match self {
            Self::Android => "Android",
            Self::Windows => "Windows",
        }
    }

    fn automation_name(self) -> &'static str {
        match self {
            Self::Android => "UiAutomator2",
            Self::Windows => "Windows",
        }
    }

    /// A capability that names the application instead of `app`.
    fn app_capabilities(self) -> &'static [&'static str] {
        match self {
            Self::Android => &["appium:app", "appium:appPackage"],
            Self::Windows => &["appium:app", "appium:appTopLevelWindow"],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reset {
    Clear,
    Relaunch,
}

#[derive(Debug, Clone)]
pub struct InstanceConfig {
    pub platform: Platform,
    pub remote_url: Url,
    pub app: Option<String>,
    pub locators: Locators,
    pub reset: Reset,
    pub find_timeout: Duration,
    pub capabilities: Map<String, Value>,
}

struct Field {
    name: &'static str,
    required: bool,
    value_type: Option<&'static str>,
    description: &'static str,
    example: Option<&'static str>,
}

/// The one list of accepted keys: `parse` refuses anything not named here,
/// and the manifest's `fields` is derived from it, so the two cannot drift.
const FIELDS: &[Field] = &[
    Field {
        name: "platform",
        required: true,
        value_type: None,
        description: "android or windows",
        example: Some("android"),
    },
    Field {
        name: "remote_url",
        required: true,
        value_type: None,
        description: "the Appium server",
        example: Some("http://localhost:4723"),
    },
    Field {
        name: "app",
        required: false,
        value_type: None,
        description: "sent as appium:app — a path on the Appium server's machine, a URL, or a Windows App ID; may be omitted when capabilities carry appium:app, appium:appPackage (android) or appium:appTopLevelWindow (windows)",
        example: Some("build/shop-debug.apk"),
    },
    Field {
        name: "locators",
        required: true,
        value_type: None,
        description: "path to this instance's locator map (name: locator), relative to the working directory",
        example: Some("locators/shop-android.yaml"),
    },
    Field {
        name: "reset",
        required: false,
        value_type: None,
        description: "on android: clear (default) wipes the app's data and relaunches it, relaunch only restarts it; on windows: closes and relaunches between scenarios unless the session is attached (app: Root or appium:appTopLevelWindow), which is left alone",
        example: Some("clear"),
    },
    Field {
        name: "find_timeout_secs",
        required: false,
        value_type: Some("number"),
        description: "how long an action waits for its element to appear, in whole seconds; default 5, 0 looks once",
        example: Some("5"),
    },
    Field {
        name: "capabilities",
        required: false,
        value_type: Some("nonscalar"),
        description: "raw Appium capabilities merged over what the plugin builds (appium:udid, appium:systemPort, ...)",
        example: None,
    },
];

pub fn fields_json() -> Value {
    FIELDS
        .iter()
        .map(|f| {
            let mut entry =
                json!({"name": f.name, "required": f.required, "description": f.description});
            if let Some(t) = f.value_type {
                entry["type"] = Value::String(t.to_string());
            }
            if let Some(example) = f.example {
                entry["example"] = Value::String(example.to_string());
            }
            entry
        })
        .collect()
}

fn optional_string<'a>(body: &'a Map<String, Value>, key: &str) -> Result<Option<&'a str>, String> {
    match body.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.as_str())),
        Some(other) => Err(format!("\"{key}\" must be a string, got {other}")),
    }
}

impl InstanceConfig {
    pub fn parse(v: &Value) -> Result<Self, String> {
        let body = v.as_object().ok_or("the instance body must be a mapping")?;
        if let Some(unknown) = body.keys().find(|k| !FIELDS.iter().any(|f| f.name == *k)) {
            let known: Vec<&str> = FIELDS.iter().map(|f| f.name).collect();
            return Err(format!(
                "unknown key {unknown:?}; known keys: {}",
                known.join(", ")
            ));
        }
        let platform_raw = optional_string(body, "platform")?
            .ok_or_else(|| format!("\"platform\" is required: one of {}", Platform::IMPLEMENTED))?;
        let platform = Platform::parse(platform_raw).ok_or_else(|| {
            format!(
                "platform {platform_raw:?} is not implemented; implemented: {}",
                Platform::IMPLEMENTED
            )
        })?;
        let remote_raw = optional_string(body, "remote_url")?
            .ok_or("\"remote_url\" is required: the Appium server, e.g. http://localhost:4723")?;
        let remote_url =
            Url::parse(remote_raw).map_err(|e| format!("\"remote_url\" {remote_raw:?}: {e}"))?;
        if !matches!(remote_url.scheme(), "http" | "https") {
            return Err(format!(
                "\"remote_url\" {remote_raw:?} must be http:// or https://"
            ));
        }
        let capabilities = match body.get("capabilities") {
            None | Some(Value::Null) => Map::new(),
            Some(Value::Object(m)) => m.clone(),
            Some(other) => return Err(format!("\"capabilities\" must be a mapping, got {other}")),
        };
        let app = optional_string(body, "app")?.map(str::to_string);
        let named = platform.app_capabilities();
        if app.is_none() && !named.iter().any(|k| capabilities.contains_key(*k)) {
            return Err(format!(
                "no application: set \"app\", or one of {} in \"capabilities\"",
                named.join(", ")
            ));
        }
        let reset = match optional_string(body, "reset")? {
            None | Some("clear") => Reset::Clear,
            Some("relaunch") => Reset::Relaunch,
            Some(other) => return Err(format!("reset {other:?} is not one of clear, relaunch")),
        };
        let find_timeout_secs = match body.get("find_timeout_secs") {
            None | Some(Value::Null) => 5,
            Some(v) => v.as_u64().ok_or_else(|| {
                format!(
                    "\"find_timeout_secs\" must be a whole number of seconds, 0 or more, got {v}"
                )
            })?,
        };
        let locators_raw = optional_string(body, "locators")?
            .ok_or("\"locators\" is required: the path to this instance's locator map")?;
        let locators = Locators::load(Path::new(locators_raw), platform)?;
        Ok(Self {
            platform,
            remote_url,
            app,
            locators,
            reset,
            find_timeout: Duration::from_secs(find_timeout_secs),
            capabilities,
        })
    }

    /// The `POST /session` body. `capabilities` win over what is built here.
    pub fn session_capabilities(&self) -> Value {
        let mut always = Map::new();
        always.insert("platformName".into(), json!(self.platform.platform_name()));
        always.insert(
            "appium:automationName".into(),
            json!(self.platform.automation_name()),
        );
        if let Some(app) = &self.app {
            always.insert("appium:app".into(), json!(app));
        }
        always.extend(self.capabilities.clone());
        json!({"capabilities": {"alwaysMatch": always}})
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A locator map on disk and an android body pointing at it.
    fn android() -> (tempfile::TempDir, Value) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("l.yaml");
        std::fs::write(&path, "cart: ~cart\n").expect("write");
        let body = json!({"platform": "android", "remote_url": "http://localhost:4723", "app": "/a.apk", "locators": path.display().to_string()});
        (dir, body)
    }

    fn refusal(body: &Value) -> String {
        InstanceConfig::parse(body).expect_err("refused")
    }

    #[test]
    fn a_minimal_body_parses_with_defaults() {
        let (_d, body) = android();
        let c = InstanceConfig::parse(&body).expect("valid");
        assert_eq!(c.platform, Platform::Android);
        assert_eq!(c.reset, Reset::Clear);
        assert_eq!(c.find_timeout, Duration::from_secs(5));
        assert_eq!(c.locators.get("cart").expect("cart").to_string(), "~cart");
    }

    #[test]
    fn platform_is_required_and_closed() {
        let (_d, mut body) = android();
        body["platform"] = json!("ios");
        let e = refusal(&body);
        assert!(
            e.contains("\"ios\" is not implemented") && e.contains("android, windows"),
            "{e}"
        );
        body.as_object_mut().expect("map").remove("platform");
        assert!(refusal(&body).contains("\"platform\" is required"));
    }

    #[test]
    fn remote_url_must_be_an_http_url() {
        let (_d, mut body) = android();
        body["remote_url"] = json!("not a url");
        assert!(refusal(&body).contains("\"remote_url\""));
        body["remote_url"] = json!("ftp://h:4723");
        assert!(refusal(&body).contains("http:// or https://"));
        body.as_object_mut().expect("map").remove("remote_url");
        assert!(refusal(&body).contains("\"remote_url\" is required"));
    }

    #[test]
    fn an_unknown_key_is_refused_by_name() {
        let (_d, mut body) = android();
        body["find_timeout"] = json!(5);
        let e = refusal(&body);
        assert!(
            e.contains("unknown key \"find_timeout\"") && e.contains("find_timeout_secs"),
            "{e}"
        );
    }

    #[test]
    fn the_app_may_come_from_capabilities_the_platform_understands() {
        let (_d, mut body) = android();
        body.as_object_mut().expect("map").remove("app");
        assert!(refusal(&body).contains("no application"));
        body["capabilities"] = json!({"appium:appPackage": "io.shop"});
        InstanceConfig::parse(&body).expect("appPackage names the app on android");
        body["platform"] = json!("windows");
        assert!(
            refusal(&body).contains("appium:appTopLevelWindow"),
            "appPackage means nothing on windows"
        );
        body["capabilities"] = json!({"appium:appTopLevelWindow": "0x1234"});
        InstanceConfig::parse(&body).expect("a top-level window names the app on windows");
    }

    #[test]
    fn reset_timeout_and_capabilities_are_typed() {
        let (_d, mut body) = android();
        body["reset"] = json!("wipe");
        assert!(refusal(&body).contains("not one of clear, relaunch"));
        body["reset"] = json!("relaunch");
        assert_eq!(
            InstanceConfig::parse(&body).expect("valid").reset,
            Reset::Relaunch
        );
        body["find_timeout_secs"] = json!("5");
        assert!(refusal(&body).contains("whole number"));
        body["find_timeout_secs"] = json!(-1);
        assert!(refusal(&body).contains("whole number"));
        body["find_timeout_secs"] = json!(0);
        assert_eq!(
            InstanceConfig::parse(&body).expect("valid").find_timeout,
            Duration::ZERO
        );
        body["capabilities"] = json!(["x"]);
        assert!(refusal(&body).contains("\"capabilities\" must be a mapping"));
    }

    #[test]
    fn a_bad_locator_map_fails_the_config() {
        let (d, mut body) = android();
        let path = d.path().join("bad.yaml");
        std::fs::write(&path, "search: =Search\n").expect("write");
        body["locators"] = json!(path.display().to_string());
        assert!(refusal(&body).contains("Windows only"));
        body.as_object_mut().expect("map").remove("locators");
        assert!(refusal(&body).contains("\"locators\" is required"));
    }

    #[test]
    fn session_capabilities_are_built_per_platform_and_extras_win() {
        let (_d, mut body) = android();
        body["capabilities"] =
            json!({"appium:udid": "emulator-5554", "appium:automationName": "Espresso"});
        let caps = InstanceConfig::parse(&body)
            .expect("valid")
            .session_capabilities();
        let always = &caps["capabilities"]["alwaysMatch"];
        assert_eq!(always["platformName"], "Android");
        assert_eq!(always["appium:app"], "/a.apk");
        assert_eq!(always["appium:udid"], "emulator-5554");
        assert_eq!(
            always["appium:automationName"], "Espresso",
            "capabilities win"
        );
        body["platform"] = json!("windows");
        body.as_object_mut().expect("map").remove("capabilities");
        let d2 = tempfile::tempdir().expect("tempdir");
        let win = d2.path().join("w.yaml");
        std::fs::write(&win, "cart: ~CartButton\n").expect("write");
        body["locators"] = json!(win.display().to_string());
        let caps = InstanceConfig::parse(&body)
            .expect("valid")
            .session_capabilities();
        assert_eq!(
            caps["capabilities"]["alwaysMatch"]["platformName"],
            "Windows"
        );
        assert_eq!(
            caps["capabilities"]["alwaysMatch"]["appium:automationName"],
            "Windows"
        );
    }

    #[test]
    fn every_field_is_described_and_typed_where_it_is_not_a_string() {
        let fields = fields_json();
        let names: Vec<&str> = fields
            .as_array()
            .expect("array")
            .iter()
            .map(|f| f["name"].as_str().expect("name"))
            .collect();
        assert_eq!(
            names,
            [
                "platform",
                "remote_url",
                "app",
                "locators",
                "reset",
                "find_timeout_secs",
                "capabilities"
            ]
        );
        assert_eq!(fields[5]["type"], "number");
        assert_eq!(fields[6]["type"], "nonscalar");
        assert!(
            fields
                .as_array()
                .expect("array")
                .iter()
                .all(|f| f["example"].is_null() || f["example"].is_string())
        );
    }
}
