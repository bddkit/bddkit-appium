# bddkit-appium

A `bddkit` plugin that drives native Android and Windows applications through an Appium server, so one scenario can do a UI action, check the API effect it caused and read the DB row it wrote — with the steps `bddkit` already provides. Resource group: `app`.

## Building

```bash
cargo build --release
```

The library is written to `target/release/`.

## Installing

Tested against `bddkit` 0.2.2 (the version `ci.yml` pins for the end-to-end jobs). Either `bddkit plugin install bddkit/bddkit-appium`, or point a plugin lock file at the built library. Where bddkit looks for lock files (layers, `plugins.local.yaml`) is in bddkit's `docs/plugin-authoring.md`, section 7.

```yaml
# .bddkit/plugins.yaml
plugin:
  - name: appium
    path: ../../bddkit-appium/target/release/libbddkit_appium.so
```

- `name` must equal the plugin's manifest name, `appium`.
- `path` is absolute, or **relative to the lock file's own directory** (`.bddkit/`, not the project root, and not the current working directory).
- **`~` is not expanded** in `path` — a `~/...` path reaches `dlopen` verbatim and fails with a confusing "no such file".

## Configuring an instance

Declare one or more named instances under `resources.app` in the ordinary `bddkit` config:

```yaml
resources:
  api: {}
  app:
    shop-android:
      platform: android
      remote_url: http://localhost:4723
      app: /srv/apps/shop-debug.apk
      locators: locators/shop-android.yaml
      # optional:
      # reset: clear                       (default; or relaunch)
      # find_timeout_secs: 5               (default; 0 looks once)
      # capabilities:                      (raw Appium capabilities merged on top of what the plugin builds)
      #   appium:udid: emulator-5554
      #   appium:systemPort: 8201
    shop-windows:
      platform: windows
      remote_url: http://windows-box:4723
      app: Microsoft.WindowsCalculator_8wekyb3d8bbwe!App
      locators: locators/shop-windows.yaml
default_app: shop-android
```

| Key | Type | Required | Default | Meaning |
|---|---|---|---|---|
| `platform` | string | yes | — | `android` or `windows`; anything else is refused with the list of implemented ones |
| `remote_url` | string | yes | — | Appium endpoint; must parse as an `http`/`https` URL |
| `app` | string | no* | — | passed verbatim as `appium:app`; the path is the **Appium server's**, not the plugin's |
| `locators` | string | yes | — | path to the locator map, relative to the working directory (same rule as the host's `paths`) |
| `reset` | string | no | `clear` | `clear` or `relaunch`; Android only in effect |
| `find_timeout_secs` | number | no | `5` | how long an **action** waits for its element; `0` = one look |
| `capabilities` | nonscalar | no | `{}` | merged on top of what the plugin builds; raw `appium:*` passthrough |

\* Without `app`, `capabilities` must carry `appium:app`, or `appium:appPackage` (Android), or `appium:appTopLevelWindow` (Windows); otherwise the instance is refused.

- `app` is a path on the **Appium server's** machine, a URL Appium downloads itself, or a Windows App ID — never a path on the machine running `bddkit`, unless that is the same machine.
- `locators` is relative to the working directory, not to the config file.
- `capabilities` win over the ones the plugin builds (`platformName`, `appium:automationName`, `appium:app`) on a key collision.
- Two Android instances behind one Appium server need distinct `appium:udid` **and** `appium:systemPort`, or the second session collides with the first.

`bddkit resource fields app` prints the key table from the plugin's own manifest, and `bddkit doctor --live` asks each declared server for its `/status` without opening a session.

## Locator map

The file named by `locators` is a flat YAML mapping of an element name to a locator. Steps use the name; the map says what it is on this platform, so one feature file can serve two platforms with two maps.

| Prefix | Strategy | Platforms |
|---|---|---|
| `~x` | accessibility id | both (preferred) |
| `#x` | id (Android resource-id) | Android |
| `=x` | name (UI Automation Name) | Windows |
| `//…` | XPath | both |

```yaml
# Android
cart: ~cart
search: "#search_input"
promo: //android.widget.TextView[@text='Sale']
```

```yaml
# Windows
equals: ~equalButton
result: =Calculator results
promo: //Text[@Name='Sale']
```

- A `#` value **must be quoted** (`"#search_input"`): an unquoted `#` starts a YAML comment and the entry becomes empty.
- An XPath locator must start with `//`.
- `#x` is refused on Windows (there the `id` strategy is the RuntimeId, which changes every run) and `=x` is refused on Android (no `name` strategy).

The whole map is checked when `bddkit` starts, before the first request, so a missing file, a duplicate name, an empty locator, a locator with none of the four prefixes or one the platform cannot use stops the run with exit 2.

For a Jetpack Compose app, set `Modifier.testTag("...")` and `testTagsAsResourceId = true` on the root; without the latter, UIAutomator cannot see the tags and `#` locators find nothing.

## Steps

`<element>` is always a name from the locator map.

| # | Kind | Step | Effect |
|---|---|---|---|
| 0 | action | `I tap "<element>"` | waits for the element to appear, then taps it |
| 1 | action | `I type "<text>" into "<element>"` | waits for the element, then types the text into it |
| 2 | action | `I clear the "<element>" field` | waits for the element, then empties it |
| 3 | action | `I press the "<key>" key` | presses back, home or enter on the device |
| 4 | action | `I read the "<element>" text from the screen as "<name>"` | waits for the element, then saves its text into a variable |
| 5 | action | `I capture the screen` | writes a PNG of the screen into the artifacts directory |
| 6 | action | `I dump the screen` | writes a PNG and the UI tree (XML) into the artifacts directory: what every element is called |
| 7 | assertion | `the "<element>" should be on the screen` | the element is found and displayed |
| 8 | assertion | `the "<element>" should not be on the screen` | the element is not found, or found and not displayed |
| 9 | assertion | `the "<element>" text on the screen should be "<text>"` | the element's text equals this exactly |

Keys: `back`, `home` and `enter` on Android; only `enter` on Windows, where `back` and `home` fail with `key "back" has no mapping on windows`.

### Semantics

- An action on an element (0–2 and 4) waits up to `find_timeout_secs` for it, looking every 100 ms; if it never appears, the step fails. `I press`, `I capture` and `I dump` touch no element and wait for nothing.
- An assertion (7–9) looks exactly once and never polls on its own, so a condition that has not settled yet is armed with the host's own eventual assertion: `I expect the next assertion to pass within "<seconds>" seconds`.
- While the device cannot read its UI tree (typically right after the app restarts), an action keeps waiting within `find_timeout_secs` and an assertion answers "not yet".
- `should not be on the screen` passes both when the element is not found and when it is found but hidden.
- Text is what the driver's text endpoint renders: the `text` attribute on Android; on Windows the value of an editable field, otherwise the control's Name.
- `<<null>>` as an argument is refused: there is no NULL on a screen.
- An element name missing from the locator map fails at run time, naming the name and the map file, not at startup — a step's arguments are opaque to the plugin until it runs, so this is the one check the plugin ABI cannot do earlier.

## Sessions and parallel runs

One Appium session per feature file per instance, opened on the file's first app step and closed when the file ends; its scenarios share it. A second file that uses the same instance while the first still holds its session fails, naming `@serial(<name>)`: one device runs one thing at a time. Put the files that share a device in one `@serial(<name>)` chain.

Between scenarios the app restarts inside the session. On Android with `reset: clear` that wipes the app's data; with `reset: relaunch` it only terminates and starts it again. On Windows the app is closed and launched again; a Windows session attached to an existing window (`app: Root` or `appium:appTopLevelWindow`) is neither restarted between scenarios nor closed at the end — its state carries over.

## Evidence

On every failed step the plugin writes a screenshot and the page source into the artifacts directory and reports them with the last WebDriver exchange. `I dump the screen` writes the same two files on demand: it is the way to learn what an element is called. Appium Inspector answers the same question interactively.

## Debugging

`I capture the screen` and `I dump the screen` write into a fresh per-step artifacts directory, and a passing step does not report where. Wrap the steps in `I am in debug mode` and the plugin prints every path it wrote and every WebDriver call (`[appium] METHOD /path status`) to stderr. Set `concurrency: 1` while debugging so parallel files do not interleave their output.

## Stand

Appium 2 or 3 (`npm i -g appium`).

- Android: `appium driver install uiautomator2`, the Android SDK, and an emulator or a device.
- Windows: on the Windows machine itself, turn Developer Mode on, `appium driver install windows`, then `appium driver run windows install-wad` to install WinAppDriver. `I type` types through the active input language on the Windows machine; make English the default input method on the Windows machine (Settings → Time & Language → Language → Keyboard → "Override for default input method") during a run (on a Russian layout "hello" arrives as "руддщ"), and nothing else may take focus while a suite runs.

## Platform risk

The Windows arm rests on WinAppDriver, whose last release is 1.2.1 (November 2020), which is closed source and which names "Windows 10 as the host". If it is retired, the Windows arm becomes a rewrite onto UI Automation directly. Licences: Appium and its drivers are Apache-2.0; WinAppDriver is free proprietary software.

## Not in v1

- Gestures, scroll and long-press.
- Permission dialogs and deep links.
- Several devices per instance, and device farms.
- iOS and macOS.
- Checking element names in a step at startup.
- Live-device CI.

## Example

Two live suites — Android's ApiDemos and Windows' Calculator — live under `examples/`; see `examples/README.md` for how to run them.

## License

Apache-2.0.
