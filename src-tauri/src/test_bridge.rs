//! How the UI flows (tests/e2e) drive the app on macOS, where WebKit has no WebDriver for an app's webview.
//! Only in debug builds on macOS, and only when tests/e2e/harness.py starts the app with `NUZKY_TEST_BRIDGE`
//! set to a secret. The window then stays off every screen and the app never becomes the active one, so a run
//! leaves the person at the Mac alone. The harness sends the same few WebDriver requests it sends WebKitWebDriver
//! on Linux, plus real key and pointer events, over a private socket like the agents' one: a 0700 folder in
//! `XDG_RUNTIME_DIR`, a 0600 socket and the secret in every request. Nothing listens on the network.
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, anyhow, bail, ensure};
use base64::Engine as _;
use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Bool, Imp, Sel};
use objc2::{class, ffi, msg_send, sel};
use objc2_app_kit::{
    NSBitmapImageFileType, NSBitmapImageRep, NSButton, NSEvent, NSEventModifierFlags, NSEventType, NSImage, NSView,
    NSWindow, NSWindowCollectionBehavior,
};
use objc2_foundation::{NSActivityOptions, NSArray, NSDictionary, NSError, NSPoint, NSProcessInfo, NSString};
use objc2_web_kit::WKWebView;
use serde_json::{Value, json};
use tauri::{LogicalSize, Manager, WebviewWindow};

static TOKEN: OnceLock<Option<String>> = OnceLock::new();

/// Takes the secret out of the environment before any other thread exists, so the agents and `nuzky mcp` the
/// app starts never inherit it.
pub fn take_token() {
    let token = std::env::var("NUZKY_TEST_BRIDGE").ok().filter(|token| token.len() >= 32);
    if token.is_some() {
        unsafe { std::env::remove_var("NUZKY_TEST_BRIDGE") };
    }
    TOKEN.set(token).ok();
}

fn token() -> Option<&'static str> {
    TOKEN.get().and_then(|token| token.as_deref())
}

/// Creates the window hidden and without a frame, and keeps the app from ever becoming the active app: wry
/// activates it when it creates the webview and tao when it has launched.
pub fn prepare(
    builder: tauri::Builder<tauri::Wry>,
    mut context: tauri::Context<tauri::Wry>,
) -> (tauri::Builder<tauri::Wry>, tauri::Context<tauri::Wry>) {
    if token().is_none() {
        return (builder, context);
    }
    for window in &mut context.config_mut().app.windows {
        window.visible = false;
        window.focus = false;
        window.decorations = false;
        window.background_throttling = Some(tauri::utils::config::BackgroundThrottlingPolicy::Disabled);
    }
    unsafe extern "C-unwind" fn stay_inactive() {}
    // Before tao creates the application object, so its subclass inherits the change.
    let class = (class!(NSApplication) as *const AnyClass).cast_mut();
    unsafe {
        ffi::class_replaceMethod(class, sel!(activateIgnoringOtherApps:), stay_inactive, c"v@:B".as_ptr());
        ffi::class_replaceMethod(class, sel!(activate), stay_inactive, c"v@:".as_ptr());
    }
    (builder.activate_ignoring_other_apps(false), context)
}

pub fn start(app: &mut tauri::App) -> Result<()> {
    let Some(token) = token() else { return Ok(()) };
    // No Dock icon and no place in Cmd-Tab. Switched to only after launch, the window's page would stay hidden.
    app.set_activation_policy(tauri::ActivationPolicy::Accessory);
    // An app nobody looks at is what App Nap slows down; the flows time what they see.
    let activity = NSProcessInfo::processInfo().beginActivityWithOptions_reason(
        NSActivityOptions::UserInitiated | NSActivityOptions::LatencyCritical,
        &NSString::from_str("Nuzky UI flows"),
    );
    std::mem::forget(activity);
    let directory =
        std::path::PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").context("XDG_RUNTIME_DIR is not set")?)
            .join("nuzky");
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&directory)?;
    let path = directory.join("webdriver.sock");
    // Left behind by the app this run started before, which a flow may close and open again.
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path).with_context(|| format!("binding {}", path.display()))?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    let app = app.handle().clone();
    std::thread::Builder::new().name("nuzky-test-bridge".into()).spawn(move || {
        for stream in listener.incoming().flatten() {
            let app = app.clone();
            std::thread::spawn(move || connection(stream, &app, token));
        }
    })?;
    Ok(())
}

/// Once the app has launched, shows the window off every screen, drawn and focused as if a person used it.
pub fn ready(app: &tauri::AppHandle) -> Result<()> {
    if token().is_none() {
        return Ok(());
    }
    let window = app.get_webview_window("main").context("the main window is missing")?;
    window.with_webview(|platform| unsafe {
        let webview = &*platform.inner().cast::<WKWebView>();
        let ns_window = &*platform.ns_window().cast::<NSWindow>();
        // WebKit stops animation frames and timers of a page it thinks nobody sees, as WebKitTestRunner knows.
        let _: () = msg_send![webview, _setWindowOcclusionDetectionEnabled: false];
        // Pixels as under gamescope on Linux, so screenshots and the flows' thresholds mean the same.
        let _: () = msg_send![webview, _setOverrideDeviceScaleFactor: 1.0f64];
        // The window of an app that is never active is never the key window, so WebKit would treat the page as
        // unfocused; like WebKitTestRunner, this window says it is and tells WebKit so.
        unsafe extern "C-unwind" fn always_key(_: *mut AnyObject, _: Sel) -> Bool {
            Bool::YES
        }
        let always_key =
            std::mem::transmute::<unsafe extern "C-unwind" fn(*mut AnyObject, Sel) -> Bool, Imp>(always_key);
        let class = ffi::object_getClass((ns_window as *const NSWindow).cast()).cast_mut();
        ffi::class_replaceMethod(class, sel!(isKeyWindow), always_key, c"B@:".as_ptr());
        ns_window.setFrameOrigin(NSPoint::new(-20000.0, -20000.0));
        // Nor in Mission Control or the window cycle.
        ns_window
            .setCollectionBehavior(NSWindowCollectionBehavior::Transient | NSWindowCollectionBehavior::IgnoresCycle);
        ns_window.orderBack(None);
        ns_window.makeFirstResponder(Some(webview));
        let center: *mut AnyObject = msg_send![class!(NSNotificationCenter), defaultCenter];
        let became_key = NSString::from_str("NSWindowDidBecomeKeyNotification");
        let _: () = msg_send![center, postNotificationName: &*became_key, object: ns_window];
    })?;
    Ok(())
}

/// Moves a file to the Trash of the home the UI flows gave the app, or `None` outside the flows.
pub fn trash(path: &std::path::Path) -> Option<Result<()>> {
    token()?;
    let moved = (|| {
        let trash = std::path::PathBuf::from(std::env::var_os("HOME").context("HOME is not set")?).join(".Trash");
        std::fs::create_dir_all(&trash)?;
        Ok(std::fs::rename(path, trash.join(path.file_name().context("the path has no file name")?))?)
    })();
    Some(moved)
}

/// One JSON request per line, `{"token", "method", "path", "body"}`, answered by `{"value"}` or `{"error"}`.
fn connection(stream: UnixStream, app: &tauri::AppHandle, token: &str) -> Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut writer = stream;
    let mut line = String::new();
    while reader.read_line(&mut line)? > 0 {
        let request: Value = serde_json::from_str(&line)?;
        line.clear();
        if request["token"].as_str() != Some(token) {
            writeln!(writer, "{}", json!({"error": "UNAUTHORIZED"}))?;
            return Ok(());
        }
        let method = request["method"].as_str().unwrap_or_default();
        let path = request["path"].as_str().unwrap_or_default();
        let reply = match handle(app, method, path, &request["body"]) {
            Ok(value) => json!({"value": value}),
            Err(error) => json!({"error": format!("{method} {path}: {error:#}")}),
        };
        writeln!(writer, "{reply}")?;
    }
    Ok(())
}

fn handle(app: &tauri::AppHandle, method: &str, path: &str, body: &Value) -> Result<Value> {
    // `/session/<id>/<command>`; there is only one session, the app itself.
    let command = path.splitn(4, '/').nth(3).unwrap_or_default();
    let window = app.get_webview_window("main").context("the window is closed")?;
    if (method, path) == ("POST", "/session") {
        // Like WebKitWebDriver, once the page has loaded.
        evaluate(&window, "1", Instant::now() + Duration::from_secs(60))?;
        return Ok(json!({"sessionId": "nuzky", "capabilities": {}}));
    }
    match (method, command) {
        ("DELETE", "") => Ok(Value::Null),
        ("POST", "execute/sync") => execute(&window, body, false),
        ("POST", "execute/async") => execute(&window, body, true),
        ("GET", "screenshot") => screenshot(&window),
        ("POST", "window/rect") => {
            if let (Some(width), Some(height)) = (body["width"].as_f64(), body["height"].as_f64()) {
                window.set_size(LogicalSize::new(width, height))?;
                // AppKit resizes the page in more than one step and the app closes its menus on each, so the
                // answer waits until the page has its new size and no resize for a moment, as under gamescope.
                let settled = format!(
                    "(() => {{ if (window.__nuzkyResizedAt === undefined) {{ window.__nuzkyResizedAt = performance.now();
                        addEventListener('resize', () => {{ window.__nuzkyResizedAt = performance.now(); }}); }}
                        return innerWidth === {width} && innerHeight === {height} && performance.now() - window.__nuzkyResizedAt > 300; }})()"
                );
                let deadline = Instant::now() + Duration::from_secs(5);
                while evaluate(&window, &settled, deadline)? != "true" && Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
            let size = window.inner_size()?.to_logical::<f64>(window.scale_factor()?);
            Ok(json!({"x": 0, "y": 0, "width": size.width, "height": size.height}))
        }
        // The title-bar button: Tauri's close asks the same CloseRequested question.
        ("DELETE", "window") => {
            window.close()?;
            Ok(Value::Null)
        }
        ("POST", "nuzky/keys") => keys(&window, serde_json::from_value(body["keys"].clone())?),
        ("POST", "nuzky/pointer") => pointer(&window, body),
        ("GET", "alert/text") => on_main(&window, |_, ns_window| {
            let sheet = sheet(ns_window)?;
            Ok(json!(sheet_text(&sheet)))
        }),
        // Return: the dialog's first button, the default one.
        ("POST", "alert/accept") => on_main(&window, |_, ns_window| {
            let alert: *mut AnyObject = unsafe { msg_send![&*sheet(ns_window)?, delegate] };
            ensure!(!alert.is_null(), "the dialog has no alert");
            let buttons: Retained<NSArray<NSButton>> = unsafe { msg_send![alert, buttons] };
            ensure!(buttons.count() > 0, "the dialog has no buttons");
            unsafe { buttons.objectAtIndex(0).performClick(None) };
            Ok(Value::Null)
        }),
        // The page's own process, whose memory the waveform flow watches.
        ("GET", "nuzky/web-process") => on_main(&window, |webview, _| {
            let pid: i32 = unsafe { msg_send![webview, _webProcessIdentifier] };
            Ok(json!(pid))
        }),
        _ => bail!("not supported"),
    }
}

/// Runs `script` as a function of `args`, as WebDriver's Execute Script does: the asynchronous form gets a
/// callback as its last argument, and a promise the synchronous form returns is awaited. The page keeps the
/// result until a second evaluation collects it, because WebKit's own result cannot wait for a promise.
fn execute(window: &WebviewWindow, body: &Value, asynchronous: bool) -> Result<Value> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    let script = serde_json::to_string(body["script"].as_str().unwrap_or_default())?;
    let args = match &body["args"] {
        Value::Null => "[]".to_owned(),
        args => serde_json::to_string(args)?,
    };
    window.eval(format!(
        "(() => {{
            const put = (reply) => {{ (window.__nuzkyBridge ??= {{}})[{id}] = reply; }};
            const answer = (ok, value) => {{
                // Elements as WebDriver's references; the flows only count them.
                const element = (key, v) => v instanceof Node ? {{'element-6066-11e4-a52e-4f735466cecf': ''}} : v;
                try {{ put(JSON.stringify(ok ? {{ok, value: value === undefined ? null : value}}
                                              : {{ok, error: `${{value}}\n${{value && value.stack || ''}}`}}, element)); }}
                catch (error) {{ put(JSON.stringify({{ok: false, error: String(error)}})); }}
            }};
            new Promise((done) => {{
                const value = new Function({script}).apply(null, {asynchronous} ? [...{args}, done] : {args});
                if (!{asynchronous}) done(value);
            }}).then((value) => answer(true, value), (error) => answer(false, error));
        }})()"
    ))?;
    let collect = format!(
        "(() => {{ const kept = window.__nuzkyBridge; const reply = kept && kept[{id}];
                   if (reply !== undefined) delete kept[{id}]; return reply ?? null; }})()"
    );
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let json = evaluate(window, &collect, deadline)?;
        if let Some(reply) = serde_json::from_str::<Option<String>>(&json).ok().flatten() {
            let reply: Value = serde_json::from_str(&reply)?;
            return match reply["ok"] == true {
                true => Ok(reply["value"].clone()),
                false => Err(anyhow!("javascript error: {}", reply["error"].as_str().unwrap_or_default())),
            };
        }
        if Instant::now() > deadline {
            bail!("the script did not finish within 120 s");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// The JSON of what `script` evaluates to. wry drops the callback of a script it is given while the page loads,
/// so the script is given again until `deadline`.
fn evaluate(window: &WebviewWindow, script: &str, deadline: Instant) -> Result<String> {
    loop {
        let (tx, rx) = mpsc::channel();
        window.eval_with_callback(script, move |json| {
            tx.send(json).ok();
        })?;
        match rx.recv_timeout(Duration::from_secs(30)) {
            Ok(json) => return Ok(json),
            Err(mpsc::RecvTimeoutError::Disconnected) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(_) => bail!("the page did not answer"),
        }
    }
}

/// The page as a PNG, base64 encoded like WebDriver's Take Screenshot.
fn screenshot(window: &WebviewWindow) -> Result<Value> {
    let (tx, rx) = mpsc::channel();
    window.with_webview(move |platform| unsafe {
        let webview = &*platform.inner().cast::<WKWebView>();
        let block = RcBlock::new(move |image: *mut NSImage, _error: *mut NSError| {
            tx.send(png(image.as_ref())).ok();
        });
        webview.takeSnapshotWithConfiguration_completionHandler(None, &block);
    })?;
    let png = rx.recv_timeout(Duration::from_secs(10)).context("WebKit did not take the snapshot")??;
    Ok(Value::String(base64::engine::general_purpose::STANDARD.encode(png)))
}

fn png(image: Option<&NSImage>) -> Result<Vec<u8>> {
    let tiff = image.and_then(|image| image.TIFFRepresentation()).context("WebKit gave no snapshot")?;
    let bitmap = NSBitmapImageRep::imageRepWithData(&tiff).context("reading the snapshot")?;
    let png = unsafe { bitmap.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new()) };
    Ok(png.context("encoding the snapshot")?.to_vec())
}

/// Key presses delivered to the webview as the window server would deliver them, so focus moves on Tab and a
/// focused button acts on Return, which events made by the page's JavaScript do not do.
fn keys(window: &WebviewWindow, keys: Vec<String>) -> Result<Value> {
    on_main(window, move |_, ns_window| {
        for key in &keys {
            let (code, text) = key_code(key)?;
            for kind in [NSEventType::KeyDown, NSEventType::KeyUp] {
                let event = key_event(ns_window, kind, code, &text)?;
                ns_window.sendEvent(&event);
            }
        }
        Ok(Value::Null)
    })
}

fn key_event(window: &NSWindow, kind: NSEventType, code: u16, text: &str) -> Result<Retained<NSEvent>> {
    let text = NSString::from_str(text);
    NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
        kind,
        NSPoint::new(0.0, 0.0),
        NSEventModifierFlags::empty(),
        NSProcessInfo::processInfo().systemUptime(),
        window.windowNumber(),
        None,
        &text,
        &text,
        false,
        code,
    )
    .context("making a key event")
}

/// The left mouse button `down` or `up` at a point of the page, with the click count that makes a second click a
/// double-click. WebKit ignores pointer moves made this way, so the flows on macOS cannot hover.
fn pointer(window: &WebviewWindow, body: &Value) -> Result<Value> {
    let (x, y) = (body["x"].as_f64().context("x")?, body["y"].as_f64().context("y")?);
    let kind = match body["kind"].as_str() {
        Some("down") => NSEventType::LeftMouseDown,
        Some("up") => NSEventType::LeftMouseUp,
        kind => bail!("no pointer action {kind:?}"),
    };
    let clicks = body["clicks"].as_i64().unwrap_or(1) as isize;
    on_main(window, move |_, ns_window| {
        // Window coordinates start at the bottom left; the page's at the top left.
        let content = ns_window.contentView().context("the window has no content")?;
        let height = content.frame().size.height;
        let event = NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
            kind,
            NSPoint::new(x, height - y),
            NSEventModifierFlags::empty(),
            NSProcessInfo::processInfo().systemUptime(),
            ns_window.windowNumber(),
            None,
            0,
            clicks,
            if kind == NSEventType::LeftMouseDown { 1.0 } else { 0.0 },
        )
        .context("making a pointer event")?;
        // Straight to the view under the pointer, as WebKitTestRunner does: the window server would not route
        // events into a window off every screen.
        let target = content.hitTest(event.locationInWindow()).context("nothing is under the pointer")?;
        match kind {
            NSEventType::LeftMouseDown => target.mouseDown(&event),
            _ => target.mouseUp(&event),
        }
        Ok(Value::Null)
    })
}

/// The question the app asks in a sheet on its window, such as whether to quit while work runs.
fn sheet(window: &NSWindow) -> Result<Retained<NSWindow>> {
    window.attachedSheet().context("no dialog is open")
}

fn sheet_text(sheet: &NSWindow) -> Vec<String> {
    let texts =
        views(sheet).into_iter().filter(|view| unsafe { msg_send![&**view, isKindOfClass: class!(NSTextField)] });
    texts
        .map(|text| unsafe { msg_send![&*text, stringValue] })
        .map(|text: Retained<NSString>| text.to_string())
        .collect()
}

/// Every view in the window.
fn views(window: &NSWindow) -> Vec<Retained<NSView>> {
    let mut found = Vec::new();
    let mut pending: Vec<Retained<NSView>> = window.contentView().into_iter().collect();
    while let Some(view) = pending.pop() {
        let children = view.subviews();
        pending.extend((0..children.count()).map(|i| children.objectAtIndex(i)));
        found.push(view);
    }
    found
}

/// Runs `f` on the main thread with the webview and its window, and waits for its answer.
fn on_main<T: Send + 'static>(
    window: &WebviewWindow,
    f: impl FnOnce(&WKWebView, &NSWindow) -> Result<T> + Send + 'static,
) -> Result<T> {
    let (tx, rx) = mpsc::channel();
    window.with_webview(move |platform| unsafe {
        tx.send(f(&*platform.inner().cast(), &*platform.ns_window().cast())).ok();
    })?;
    rx.recv_timeout(Duration::from_secs(10)).context("the window did not answer")?
}

/// The key code and text of an X11 key name, as the Linux flows use them, or of one character.
fn key_code(key: &str) -> Result<(u16, String)> {
    let special = |code: u16, text: char| Ok((code, text.to_string()));
    match key {
        "Return" => special(36, '\r'),
        "Tab" => special(48, '\t'),
        "space" => special(49, ' '),
        "Right" => special(124, '\u{f703}'),
        _ if key.chars().count() == 1 => Ok((0, key.to_owned())),
        _ => bail!("no key {key:?}"),
    }
}
