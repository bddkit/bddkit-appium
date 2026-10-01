//! The locator map: a flat YAML file of `name: locator`, one per instance.
//! Features name elements; this file says what each name is on one platform.

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};

use crate::config::Platform;
use crate::webdriver::Strategy;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Locator {
    pub strategy: Strategy,
    pub value: String,
}

impl fmt::Display for Locator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let prefix = match self.strategy {
            Strategy::AccessibilityId => "~",
            Strategy::Id => "#",
            Strategy::Name => "=",
            Strategy::XPath => "",
        };
        write!(f, "{prefix}{}", self.value)
    }
}

fn parse_locator(raw: &str, platform: Platform) -> Result<Locator, String> {
    let (strategy, value) = if raw.starts_with("//") {
        (Strategy::XPath, raw)
    } else if let Some(v) = raw.strip_prefix('~') {
        (Strategy::AccessibilityId, v)
    } else if let Some(v) = raw.strip_prefix('#') {
        (Strategy::Id, v)
    } else if let Some(v) = raw.strip_prefix('=') {
        (Strategy::Name, v)
    } else {
        return Err(format!(
            "{raw:?} starts with none of ~ (accessibility id), # (resource-id), = (name), // (XPath)"
        ));
    };
    if value.is_empty() {
        return Err(format!("{raw:?} has a prefix and nothing after it"));
    }
    match (strategy, platform) {
        (Strategy::Id, Platform::Windows) => Err(format!(
            "{raw:?}: # (resource-id) is Android only; on Windows use ~ (AutomationId) or = (Name)"
        )),
        (Strategy::Name, Platform::Android) => Err(format!(
            "{raw:?}: = (Name) is Windows only; on Android use ~ (content-desc) or # (resource-id)"
        )),
        _ => Ok(Locator {
            strategy,
            value: value.to_string(),
        }),
    }
}

#[derive(Debug, Clone)]
pub struct Locators {
    path: PathBuf,
    map: HashMap<String, Locator>,
}

impl Locators {
    pub fn load(path: &Path, platform: Platform) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("locators: cannot read {}: {e}", path.display()))?;
        Self::parse(&text, path, platform)
    }

    fn parse(text: &str, path: &Path, platform: Platform) -> Result<Self, String> {
        let at = |e: String| format!("locators {}: {e}", path.display());
        // A `Mapping`, not a `HashMap`: only the former refuses a duplicate
        // key; a HashMap target silently keeps the last one.
        let mapping: serde_yaml_ng::Mapping =
            serde_yaml_ng::from_str(text).map_err(|e| at(e.to_string()))?;
        let mut map = HashMap::new();
        for (key, value) in mapping {
            let name = key
                .as_str()
                .ok_or_else(|| at(format!("the key {key:?} is not a string")))?;
            let raw = value.as_str().ok_or_else(|| {
                let hint = if value.is_null() {
                    " (an unquoted # starts a YAML comment — quote it: \"#id\")"
                } else {
                    ""
                };
                at(format!(
                    "{name:?} must be a string locator, got {value:?}{hint}"
                ))
            })?;
            let locator = parse_locator(raw, platform).map_err(|e| at(format!("{name:?}: {e}")))?;
            map.insert(name.to_string(), locator);
        }
        Ok(Self {
            path: path.to_path_buf(),
            map,
        })
    }

    pub fn get(&self, name: &str) -> Result<&Locator, String> {
        self.map
            .get(name)
            .ok_or_else(|| format!("no element {name:?} in {}", self.path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str, platform: Platform) -> Result<Locators, String> {
        Locators::parse(text, Path::new("l.yaml"), platform)
    }

    #[test]
    fn every_prefix_maps_to_its_strategy() {
        let l = parse(
            "cart: ~cart\nsearch: \"#search_input\"\npromo: //android.widget.TextView[@text='Sale']\n",
            Platform::Android,
        )
        .expect("valid android map");
        assert_eq!(
            l.get("cart").expect("cart"),
            &Locator {
                strategy: Strategy::AccessibilityId,
                value: "cart".into()
            }
        );
        assert_eq!(l.get("search").expect("search").strategy, Strategy::Id);
        assert_eq!(l.get("promo").expect("promo").strategy, Strategy::XPath);
        assert_eq!(
            l.get("promo").expect("promo").value,
            "//android.widget.TextView[@text='Sale']"
        );
        let w = parse("search: =Search\n", Platform::Windows).expect("valid windows map");
        assert_eq!(w.get("search").expect("search").strategy, Strategy::Name);
        assert_eq!(w.get("search").expect("search").to_string(), "=Search");
    }

    #[test]
    fn a_prefix_the_platform_lacks_is_refused_naming_the_alternatives() {
        let e = parse("search: \"#search_input\"\n", Platform::Windows).expect_err("# on windows");
        assert!(
            e.contains("Android only") && e.contains("\"search\""),
            "{e}"
        );
        let e = parse("search: =Search\n", Platform::Android).expect_err("= on android");
        assert!(e.contains("Windows only"), "{e}");
    }

    #[test]
    fn malformed_entries_are_refused() {
        let e = parse("cart: cart\n", Platform::Android).expect_err("no prefix");
        assert!(e.contains("starts with none of"), "{e}");
        let e = parse("cart: \"~\"\n", Platform::Android).expect_err("empty after prefix");
        assert!(e.contains("nothing after it"), "{e}");
        let e = parse("cart: ~a\ncart: ~b\n", Platform::Android).expect_err("duplicate");
        assert!(e.contains("duplicate"), "{e}");
        let e = parse("cart: 5\n", Platform::Android).expect_err("number");
        assert!(e.contains("must be a string locator"), "{e}");
        let e = parse("cart:\n", Platform::Android).expect_err("null");
        assert!(e.contains("must be a string locator"), "{e}");
        let e = parse("search: #search_input\n", Platform::Android).expect_err("comment");
        assert!(e.contains("quote it"), "{e}");
        let e = parse("- ~cart\n", Platform::Android).expect_err("a list is not a map");
        assert!(e.starts_with("locators l.yaml:"), "{e}");
    }

    #[test]
    fn a_missing_file_and_an_unknown_name_say_which_file() {
        let e = Locators::load(Path::new("/nonexistent/l.yaml"), Platform::Android)
            .expect_err("missing");
        assert!(e.contains("/nonexistent/l.yaml"), "{e}");
        let l = parse("cart: ~cart\n", Platform::Android).expect("valid");
        assert_eq!(
            l.get("basket").expect_err("unknown"),
            "no element \"basket\" in l.yaml"
        );
    }
}
