use super::*;
use crate::computer::blocklist::AppIdentity;
use crate::computer::coords::Point;
use crate::computer::fake::{FakeClock, FakeDesktop, FakeDialogs, app};
use crate::computer::keys::parse_combo;
use crate::computer::risk::{AxElement, Focus};
use crate::computer::session::{Reason, SESSION_MS, StateEvent};
use crate::computer::{InputEvent, MouseButton};
use serde_json::json;
use std::sync::Mutex;
use std::sync::atomic::Ordering;

// Blocked apps: bundle id, and the name the refusal says.
const SETTINGS: (&str, &str) = ("com.apple.systempreferences", "System Settings");
const PASSWORDS: (&str, &str) = ("com.1password.1password", "1Password");
const KEYCHAIN: (&str, &str) = ("com.apple.keychainaccess", "Keychain Access");
/// An `open_app` name that is blocked.
const BLOCKED_OPEN: &str = "Keychain Access";

const TOKEN: &str = "secret-token";
const OWN_PID: i32 = 4242;
const NOW: u64 = 1_700_000_000_000;

struct Rig {
    helper: Helper,
    desktop: Arc<FakeDesktop>,
    dialogs: Arc<FakeDialogs>,
    clock: Arc<FakeClock>,
    control: Arc<Control>,
    states: Arc<Mutex<Vec<StateEvent>>>,
}

fn rig_with(allow: bool, confirm: bool) -> Rig {
    let desktop = Arc::new(FakeDesktop::default());
    let dialogs = Arc::new(FakeDialogs::new(allow, confirm));
    let clock = Arc::new(FakeClock::new(NOW));
    let states = Arc::new(Mutex::new(Vec::new()));
    let sink = states.clone();
    let control = Arc::new(Control::new(move |event| sink.lock().unwrap().push(event.clone())));
    control.set_enabled(true);
    let helper = Helper::new(desktop.clone(), dialogs.clone(), clock.clone(), control.clone(), TOKEN.into(), OWN_PID);
    Rig { helper, desktop, dialogs, clock, control, states }
}

fn rig() -> Rig {
    rig_with(true, true)
}

fn post(rig: &Rig, authorization: Option<&str>, body: &str) -> HttpReply {
    rig.helper.handle(&HttpRequest { method: "POST", path: "/mcp", authorization, body })
}

impl Rig {
    fn rpc(&self, method: &str, params: Value) -> Value {
        let body = json!({"jsonrpc": "2.0", "id": 7, "method": method, "params": params}).to_string();
        let reply = post(self, Some(&format!("Bearer {TOKEN}")), &body);
        assert_eq!(reply.status, 200, "{reply:?}");
        let body = reply.body.expect("a JSON-RPC reply");
        assert_eq!((body["jsonrpc"].as_str(), body["id"].as_i64()), (Some("2.0"), Some(7)));
        body
    }

    fn call(&self, name: &str, arguments: Value) -> Value {
        let body = self.rpc("tools/call", json!({"name": name, "arguments": arguments}));
        body["result"].clone()
    }

    /// Grants a session and takes the screenshot that coordinates refer to.
    fn ready(&self) {
        assert!(!is_error(&self.call("screenshot", json!({}))));
    }
}

fn is_error(result: &Value) -> bool {
    result["isError"] == json!(true)
}

fn first_text(result: &Value) -> String {
    result["content"][0]["text"].as_str().unwrap_or_default().to_string()
}

fn safari() -> AppIdentity {
    app(200, "com.apple.Safari", "Safari")
}

fn ailfred() -> AppIdentity {
    app(OWN_PID, "com.ailfred", "Ailfred")
}

// --- HTTP and JSON-RPC ---

#[test]
fn requests_without_the_bearer_token_get_401() {
    let rig = rig();
    let body = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}).to_string();
    for authorization in [None, Some("Bearer wrong"), Some(TOKEN), Some("Basic secret-token")] {
        let reply = post(&rig, authorization, &body);
        assert_eq!(reply.status, 401, "{authorization:?}");
    }
    assert_eq!(post(&rig, Some("Bearer secret-token"), &body).status, 200);
}

#[test]
fn only_post_to_mcp_is_served() {
    let rig = rig();
    let auth = Some("Bearer secret-token");
    let get = rig.helper.handle(&HttpRequest { method: "GET", path: "/mcp", authorization: auth, body: "" });
    assert_eq!(get.status, 405);
    let other = rig.helper.handle(&HttpRequest { method: "POST", path: "/other", authorization: auth, body: "{}" });
    assert_eq!(other.status, 404);
}

#[test]
fn initialize_reports_the_server_and_tools_capability() {
    let rig = rig();
    let body = rig.rpc(
        "initialize",
        json!({"protocolVersion": "2025-03-26", "capabilities": {}, "clientInfo": {"name": "t", "version": "1"}}),
    );
    let result = &body["result"];
    assert_eq!(result["protocolVersion"], "2025-03-26");
    assert!(result["capabilities"]["tools"].is_object());
    assert_eq!(result["serverInfo"]["name"], "ailfred-computer");
}

#[test]
fn initialize_answers_an_unknown_protocol_version_with_its_own() {
    let rig = rig();
    let body = rig.rpc("initialize", json!({"protocolVersion": "1999-01-01"}));
    assert_eq!(body["result"]["protocolVersion"], "2025-06-18");
}

#[test]
fn notifications_are_accepted_without_a_body() {
    let rig = rig();
    let body = json!({"jsonrpc": "2.0", "method": "notifications/initialized"}).to_string();
    assert_eq!(post(&rig, Some("Bearer secret-token"), &body), HttpReply { status: 202, body: None });
}

#[test]
fn invalid_json_is_a_parse_error() {
    let reply = post(&rig(), Some("Bearer secret-token"), "{not json");
    assert_eq!(reply.status, 400);
    assert_eq!(reply.body.unwrap()["error"]["code"], -32700);
}

#[test]
fn unknown_methods_are_rejected() {
    let body = rig().rpc("resources/list", json!({}));
    assert_eq!(body["error"]["code"], -32601);
}

#[test]
fn tools_list_describes_every_tool_with_an_input_schema() {
    let body = rig().rpc("tools/list", json!({}));
    let tools = body["result"]["tools"].as_array().unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["screenshot", "click", "move", "drag", "scroll", "type", "key", "open_app", "list_apps", "wait", "batch"]);
    for tool in tools {
        let schema = &tool["inputSchema"];
        assert_eq!(schema["type"], "object", "{tool}");
        assert_eq!(schema["properties"]["intent"]["type"], "string", "{tool}");
        assert!(tool["description"].as_str().is_some_and(|d| !d.is_empty()));
    }
    let click = tools.iter().find(|t| t["name"] == "click").unwrap();
    assert_eq!(click["inputSchema"]["required"], json!(["x", "y"]));
    assert_eq!(click["inputSchema"]["properties"]["button"]["enum"], json!(["left", "right"]));
    let wait = tools.iter().find(|t| t["name"] == "wait").unwrap();
    assert_eq!(wait["inputSchema"]["properties"]["ms"]["maximum"], 5000);
}

#[test]
fn an_unknown_tool_is_a_protocol_error() {
    let rig = rig();
    let body = rig.rpc("tools/call", json!({"name": "format_disk", "arguments": {}}));
    assert_eq!(body["error"]["code"], -32602);
    assert_eq!(rig.dialogs.grants.load(Ordering::SeqCst), 0);
}

// --- Session grant ---

#[test]
fn while_computer_control_is_off_every_tool_is_refused_without_asking() {
    let rig = rig();
    rig.control.set_enabled(false);
    for (name, arguments) in [("screenshot", json!({})), ("list_apps", json!({})), ("open_app", json!({"name": "Notes"}))] {
        let result = rig.call(name, arguments);
        assert!(is_error(&result), "{name}");
        assert_eq!(first_text(&result), OFF, "{name}");
    }
    assert_eq!(rig.dialogs.grants.load(Ordering::SeqCst), 0);
    assert_eq!(rig.desktop.captures.load(Ordering::SeqCst), 0);
    assert!(rig.desktop.opened.lock().unwrap().is_empty());
    // Back on, it asks for a session as usual.
    rig.control.set_enabled(true);
    rig.ready();
    assert_eq!(rig.dialogs.grants.load(Ordering::SeqCst), 1);
}

#[test]
fn turning_it_off_mid_session_drops_the_next_action() {
    let rig = rig();
    rig.ready();
    rig.control.set_enabled(false);
    let result = rig.call("click", json!({"x": 10, "y": 10}));
    assert_eq!(first_text(&result), OFF);
    assert!(rig.desktop.events().is_empty());
}

#[test]
fn deny_returns_a_tool_error_and_starts_nothing() {
    let rig = rig_with(false, true);
    let result = rig.call("screenshot", json!({}));
    assert!(is_error(&result));
    assert!(first_text(&result).contains("not allowed"), "{result}");
    assert_eq!(rig.dialogs.grants.load(Ordering::SeqCst), 1);
    assert_eq!(rig.desktop.captures.load(Ordering::SeqCst), 0);
    assert!(rig.states.lock().unwrap().is_empty());
    assert!(!rig.control.is_active(NOW));
}

#[test]
fn allow_starts_a_session_once_and_reports_it() {
    let rig = rig();
    rig.ready();
    rig.ready();
    assert_eq!(rig.dialogs.grants.load(Ordering::SeqCst), 1);
    assert_eq!(
        *rig.states.lock().unwrap(),
        vec![StateEvent { active: true, ends_at: Some(NOW + SESSION_MS), reason: Some(Reason::Granted) }]
    );
}

#[test]
fn after_the_timeout_the_next_call_asks_again() {
    let rig = rig();
    rig.ready();
    rig.clock.advance(SESSION_MS);
    rig.ready();
    assert_eq!(rig.dialogs.grants.load(Ordering::SeqCst), 2);
}

#[test]
fn after_a_stop_the_next_call_asks_again() {
    let rig = rig();
    rig.ready();
    rig.control.end(Reason::Panic);
    rig.ready();
    assert_eq!(rig.dialogs.grants.load(Ordering::SeqCst), 2);
}

#[test]
fn an_action_queued_before_a_stop_is_dropped() {
    let rig = rig();
    rig.ready();
    let queued_at = rig.control.epoch();
    rig.control.end(Reason::Stopped);
    let result = rig.helper.call_tool_from(queued_at, "click", &json!({"x": 10, "y": 10})).unwrap();
    assert!(is_error(&result));
    assert!(first_text(&result).contains("stopped"), "{result}");
    assert!(rig.desktop.events().is_empty());
    assert_eq!(rig.dialogs.grants.load(Ordering::SeqCst), 1);
}

#[test]
fn stopping_or_switching_off_while_the_grant_is_asked_drops_the_action_even_if_allowed() {
    for (ending, said) in [("panic", STOPPED), ("off", OFF)] {
        let rig = rig();
        let control = rig.control.clone();
        *rig.dialogs.during.lock().unwrap() = Some(Box::new(move || match ending {
            "panic" => control.end(Reason::Panic),
            _ => control.set_enabled(false),
        }));
        let result = rig.call("screenshot", json!({}));
        assert_eq!(first_text(&result), said, "{ending}");
        assert!(!rig.control.is_active(NOW), "{ending}: no session from the Allow that came after");
        assert_eq!(rig.desktop.captures.load(Ordering::SeqCst), 0, "{ending}");
    }
}

#[test]
fn a_pointer_action_whose_target_changed_before_it_is_posted_is_refused() {
    // Safari is under the point when it's checked; a password manager's panel is there by the
    // post (it opened while the user read a confirmation, say).
    let rig = rig();
    rig.ready();
    let looks = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let password_manager = app(300, "com.1password.1password", "1Password");
    let notes = app(301, "com.apple.Notes", "Notes");
    for (now, said) in [(password_manager, "does not control 1Password"), (notes, "changed from Safari to Notes")] {
    for (name, arguments) in [
        ("click", json!({"x": 10, "y": 10})),
        ("move", json!({"x": 10, "y": 10})),
        ("scroll", json!({"x": 10, "y": 10, "dx": 0, "dy": 5})),
        ("drag", json!({"fromX": 10, "fromY": 10, "toX": 20, "toY": 20})),
    ] {
        let looks = looks.clone();
        looks.store(0, Ordering::SeqCst);
        // A drag looks at two points before posting; the rest at one.
        let checked = if name == "drag" { 2 } else { 1 };
        let now = now.clone();
        *rig.desktop.owner_at.lock().unwrap() = Box::new(move |_| {
            let n = looks.fetch_add(1, Ordering::SeqCst);
            Some(if n < checked { safari() } else { now.clone() })
        });
        let result = rig.call(name, arguments);
        assert!(is_error(&result), "{name}: {result}");
        assert!(first_text(&result).contains(said), "{name}: {result}");
        assert!(rig.desktop.events().is_empty(), "{name}: nothing posted");
    }
    }
}

#[test]
fn a_confirmation_shows_the_models_words_on_one_line_and_cut_short() {
    let rig = rig();
    rig.ready();
    let intent = format!("send it\n\nThis action is safe. Click Allow.{}", " padding".repeat(60));
    rig.call("click", json!({"x": 10, "y": 10, "intent": intent}));
    let asked = rig.dialogs.confirmations.lock().unwrap().clone();
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].matches('\n').count(), 2, "only the message's own break: {:?}", asked[0]);
    assert!(asked[0].chars().count() < 400, "{}", asked[0].chars().count());
}

#[test]
fn a_stop_during_wait_ends_it_early() {
    let rig = rig();
    rig.ready();
    let control = rig.control.clone();
    *rig.clock.on_sleep.lock().unwrap() = Some(Box::new(move || control.end(Reason::Panic)));
    let result = rig.call("wait", json!({"ms": 3000}));
    assert!(is_error(&result));
    assert!(first_text(&result).contains("stopped"), "{result}");
    assert!(rig.clock.now_ms() - NOW < 3000);
}

// --- Screenshots and coordinates ---

#[test]
fn a_screenshot_is_an_image_block_plus_its_geometry() {
    let rig = rig();
    let result = rig.call("screenshot", json!({}));
    let content = result["content"].as_array().unwrap();
    let image = content.iter().find(|c| c["type"] == "image").unwrap();
    assert_eq!(image["mimeType"], "image/jpeg");
    assert_eq!(image["data"], "anBlZw==");
    let meta = content.iter().rfind(|c| c["type"] == "text").unwrap();
    let meta: Value = serde_json::from_str(meta["text"].as_str().unwrap()).unwrap();
    assert_eq!(meta, json!({"imageWidth": 1280, "imageHeight": 800, "scale": 0.5, "frontApp": "Safari"}));
}

#[test]
fn coordinates_need_a_screenshot_first() {
    let rig = rig();
    let result = rig.call("click", json!({"x": 10, "y": 10}));
    assert!(is_error(&result));
    assert!(first_text(&result).contains("screenshot"), "{result}");
    assert!(rig.desktop.events().is_empty());
}

#[test]
fn a_new_session_needs_a_new_screenshot() {
    for end in ["stop", "timeout"] {
        let rig = rig();
        rig.ready();
        match end {
            "stop" => rig.control.end(Reason::Stopped),
            _ => rig.clock.advance(SESSION_MS),
        }
        let result = rig.call("click", json!({"x": 10, "y": 10}));
        assert!(is_error(&result), "{end}");
        assert!(first_text(&result).contains("screenshot"), "{result}");
        // The grant comes first; only then is the missing screenshot reported.
        assert_eq!(rig.dialogs.grants.load(Ordering::SeqCst), 2, "{end}");
        assert!(rig.desktop.events().is_empty());
        rig.ready();
        assert!(!is_error(&rig.call("click", json!({"x": 10, "y": 10}))));
    }
}

#[test]
fn click_maps_pixels_to_points_and_returns_a_fresh_screenshot() {
    let rig = rig();
    rig.ready();
    let result = rig.call("click", json!({"x": 100, "y": 50, "button": "right", "count": 2}));
    assert!(!is_error(&result), "{result}");
    let at = Point { x: 200.0, y: 100.0 };
    assert_eq!(rig.desktop.events(), vec![InputEvent::Click { at, button: MouseButton::Right, count: 2 }]);
    assert_eq!(rig.desktop.captures.load(Ordering::SeqCst), 2);
    assert!(result["content"].as_array().unwrap().iter().any(|c| c["type"] == "image"));
}

#[test]
fn move_drag_and_scroll_map_their_points() {
    let rig = rig();
    rig.ready();
    rig.call("move", json!({"x": 1, "y": 2}));
    rig.call("drag", json!({"fromX": 10, "fromY": 20, "toX": 30, "toY": 40}));
    rig.call("scroll", json!({"x": 5, "y": 6, "dx": 0, "dy": 120}));
    let p = |x, y| Point { x, y };
    assert_eq!(
        rig.desktop.events(),
        vec![
            InputEvent::Move(p(2.0, 4.0)),
            InputEvent::Drag { from: p(20.0, 40.0), to: p(60.0, 80.0) },
            InputEvent::Scroll { at: p(10.0, 12.0), dx: 0, dy: 120 },
        ]
    );
}

#[test]
fn invalid_arguments_are_tool_errors() {
    let rig = rig();
    rig.ready();
    for (name, args) in [
        ("click", json!({"x": 1})),
        ("click", json!({"x": 1, "y": 1, "count": 4})),
        ("click", json!({"x": 1, "y": 1, "button": "middle"})),
        ("click", json!({"x": 5000, "y": 1})),
        ("wait", json!({"ms": 5001})),
        ("key", json!({"combo": "cmd+nope"})),
        ("type", json!({})),
    ] {
        let result = rig.call(name, args.clone());
        assert!(is_error(&result), "{name} {args}");
    }
    assert!(rig.desktop.events().is_empty());
}

// --- Permissions ---

#[test]
fn missing_permissions_are_explained_before_asking_for_a_session() {
    let rig = rig();
    rig.desktop.permissions.lock().unwrap().screen_recording = false;
    let result = rig.call("screenshot", json!({}));
    assert_eq!(
        first_text(&result),
        "Ailfred needs the Screen Recording permission. Ask the user to grant it in Ailfred's Settings → Permissions."
    );
    rig.desktop.permissions.lock().unwrap().screen_recording = true;
    rig.desktop.permissions.lock().unwrap().accessibility = false;
    let result = rig.call("key", json!({"combo": "tab"}));
    assert!(first_text(&result).contains("Accessibility"), "{result}");
    assert_eq!(rig.dialogs.grants.load(Ordering::SeqCst), 0);
}

#[test]
fn a_key_result_names_the_whole_combo() {
    let rig = rig();
    rig.ready();
    assert_eq!(first_text(&rig.call("key", json!({"combo": "ctrl+a"}))), "Press ctrl+a in Safari: done.");
    assert_eq!(first_text(&rig.call("key", json!({"combo": "tab"}))), "Press tab in Safari: done.");
}

#[test]
fn screenshots_the_system_cannot_take_are_a_clear_error() {
    let rig = rig();
    rig.desktop.permissions.lock().unwrap().screenshots_supported = false;
    let result = rig.call("screenshot", json!({}));
    assert!(is_error(&result));
    assert_eq!(first_text(&result), "Screenshots need macOS 14 or later, so computer control is unavailable on this Mac.");
    assert_eq!(rig.desktop.captures.load(Ordering::SeqCst), 0);
    assert_eq!(rig.dialogs.grants.load(Ordering::SeqCst), 0);
}

// --- Blocklist and rate limit ---

#[test]
fn pointer_actions_on_a_blocked_window_are_refused() {
    let rig = rig();
    rig.ready();
    *rig.desktop.owner_at.lock().unwrap() = Box::new(|_| Some(app(300, SETTINGS.0, SETTINGS.1)));
    let result = rig.call("click", json!({"x": 10, "y": 10}));
    assert!(is_error(&result));
    assert!(first_text(&result).contains(SETTINGS.1), "{result}");
    assert!(rig.desktop.events().is_empty());
}

#[test]
fn a_drag_onto_a_blocked_window_is_refused() {
    let rig = rig();
    rig.ready();
    *rig.desktop.owner_at.lock().unwrap() =
        Box::new(|at: Point| if at.x > 100.0 { Some(app(301, PASSWORDS.0, PASSWORDS.1)) } else { Some(safari()) });
    let result = rig.call("drag", json!({"fromX": 10, "fromY": 10, "toX": 200, "toY": 10}));
    assert!(is_error(&result));
    assert!(rig.desktop.events().is_empty());
}

#[test]
fn ailfred_own_windows_are_refused() {
    let rig = rig();
    rig.ready();
    *rig.desktop.owner_at.lock().unwrap() =
        Box::new(|_| Some(AppIdentity { pid: OWN_PID, bundle_id: None, name: "ailfred".into() }));
    assert!(is_error(&rig.call("click", json!({"x": 10, "y": 10}))));
}

#[test]
fn typing_stops_between_chunks_when_ailfred_comes_to_front() {
    let rig = rig();
    rig.ready();
    *rig.desktop.front_after_posts.lock().unwrap() =
        Some((1, AppIdentity { pid: OWN_PID, bundle_id: None, name: "ailfred".into() }));
    let result = rig.call("type", json!({"text": "a".repeat(45)}));
    assert!(is_error(&result), "{result}");
    assert!(first_text(&result).contains("Ailfred does not control Ailfred. The action was refused."), "{result}");
    assert!(first_text(&result).contains("Typing stopped after 20 of 45 characters."), "{result}");
    assert_eq!(rig.desktop.events(), vec![InputEvent::Text("a".repeat(20))]);
}

#[test]
fn a_switch_during_a_screenshot_is_not_taken_as_what_the_agent_saw() {
    let rig = rig();
    rig.ready();
    // The image shows Safari; Mail comes to the front while it is taken.
    *rig.desktop.front_during_capture.lock().unwrap() = Some(app(400, "com.apple.mail", "Mail"));
    assert!(!is_error(&rig.call("screenshot", json!({}))));
    let result = rig.call("type", json!({"text": "hello"}));
    assert_eq!(
        first_text(&result),
        "Mail is in front now, not the app of the latest screenshot, so nothing was sent. Take a screenshot first."
    );
    assert!(rig.desktop.events().is_empty());
    // A screenshot taken with Mail steady in front lets the keys through.
    assert!(!is_error(&rig.call("screenshot", json!({}))));
    assert!(!is_error(&rig.call("type", json!({"text": "hello"}))));
}

#[test]
fn typing_stops_between_chunks_when_another_app_comes_to_front() {
    let rig = rig();
    rig.ready();
    *rig.desktop.front_after_posts.lock().unwrap() = Some((1, app(400, "com.apple.mail", "Mail")));
    let result = rig.call("type", json!({"text": "a".repeat(45)}));
    assert!(is_error(&result), "{result}");
    assert_eq!(
        first_text(&result),
        "Mail is in front now, not the app of the latest screenshot, so the rest was not sent. Take a screenshot first. \
Typing stopped after 20 of 45 characters."
    );
    assert_eq!(rig.desktop.events(), vec![InputEvent::Text("a".repeat(20))]);
}

#[test]
fn typing_rechecks_before_return_and_counts_unicode_characters() {
    let rig = rig();
    rig.ready();
    *rig.desktop.front_after_posts.lock().unwrap() = Some((1, app(400, "com.apple.mail", "Mail")));
    let result = rig.call("type", json!({"text": "🦀é\nsecond"}));
    assert!(is_error(&result), "{result}");
    assert_eq!(
        first_text(&result),
        "Mail is in front now, not the app of the latest screenshot, so the rest was not sent. Take a screenshot first. \
Typing stopped after 2 of 9 characters."
    );
    assert_eq!(rig.desktop.events(), vec![InputEvent::Text("🦀é".into())]);
}

#[test]
fn typing_rechecks_after_a_leading_return_and_keeps_the_blocked_app_refusal() {
    let rig = rig();
    rig.ready();
    *rig.desktop.front_after_posts.lock().unwrap() = Some((1, app(302, KEYCHAIN.0, KEYCHAIN.1)));
    let result = rig.call("type", json!({"text": "\r\nnext"}));
    assert!(is_error(&result), "{result}");
    assert_eq!(
        first_text(&result),
        format!("Ailfred does not control {}. The action was refused. Typing stopped after 1 of 5 characters.", KEYCHAIN.1)
    );
    assert_eq!(rig.desktop.events(), vec![InputEvent::Key(parse_combo("return").unwrap())]);
}

#[test]
fn typing_stops_when_an_unblocked_focused_recipient_changes() {
    // Both replacing an existing recipient and moving focus away from the front app.
    for initial in [None, Some(app(401, "com.example.editor", "Editor"))] {
        let rig = rig();
        rig.ready();
        *rig.desktop.focused_app.lock().unwrap() = initial;
        *rig.desktop.focused_after_posts.lock().unwrap() = Some((1, app(402, "com.example.panel", "Panel")));
        let result = rig.call("type", json!({"text": "a".repeat(45)}));
        assert!(is_error(&result), "{result}");
        assert_eq!(
            first_text(&result),
            "The app receiving the keys changed while typing, so the rest was not sent. Typing stopped after 20 of 45 characters."
        );
        assert_eq!(rig.desktop.events(), vec![InputEvent::Text("a".repeat(20))]);
    }
}

#[test]
fn typing_rechecks_front_and_recipient_before_the_first_post() {
    for change_front in [true, false] {
        let rig = rig();
        rig.ready();
        let desktop = rig.desktop.clone();
        *rig.desktop.during_focused_element.lock().unwrap() = Some(Box::new(move || {
            let changed = Some(app(402, "com.example.panel", "Panel"));
            if change_front {
                *desktop.frontmost.lock().unwrap() = changed;
            } else {
                *desktop.focused_app.lock().unwrap() = changed;
            }
        }));
        let result = rig.call("type", json!({"text": "hello"}));
        assert!(is_error(&result), "{result}");
        assert!(first_text(&result).ends_with("Typing stopped after 0 of 5 characters."), "{result}");
        if change_front {
            // Nothing landed yet, so the screenshot guard can still say so.
            assert_eq!(
                first_text(&result),
                "Panel is in front now, not the app of the latest screenshot, so nothing was sent. Take a screenshot first. \
Typing stopped after 0 of 5 characters."
            );
        } else {
            assert_eq!(
                first_text(&result),
                "The app receiving the keys changed before typing, so nothing was sent. Typing stopped after 0 of 5 characters."
            );
        }
        assert!(rig.desktop.events().is_empty());
    }
}

#[test]
fn a_key_rechecks_front_and_recipient_before_it_is_posted() {
    for combo in ["tab", "return"] {
        for change_front in [true, false] {
            let rig = rig();
            rig.ready();
            let desktop = rig.desktop.clone();
            *rig.desktop.during_focused_element.lock().unwrap() = Some(Box::new(move || {
                let changed = Some(app(402, "com.example.panel", "Panel"));
                if change_front {
                    *desktop.frontmost.lock().unwrap() = changed;
                } else {
                    *desktop.focused_app.lock().unwrap() = changed;
                }
            }));
            let result = rig.call("key", json!({"combo": combo}));
            assert!(is_error(&result), "{combo} {change_front}: {result}");
            if !change_front {
                assert_eq!(first_text(&result), KEY_RECIPIENT_CHANGED, "{combo}");
            }
            assert!(rig.desktop.events().is_empty(), "{combo} {change_front}");
        }
    }
}

#[test]
fn a_key_goes_through_to_an_allowed_focused_panel() {
    let rig = rig();
    rig.ready();
    *rig.desktop.focused_app.lock().unwrap() = Some(app(401, "com.example.editor", "Editor"));
    let result = rig.call("key", json!({"combo": "tab"}));
    assert_eq!(first_text(&result), "Press tab in Editor: done.");
    assert_eq!(rig.desktop.events(), vec![InputEvent::Key(parse_combo("tab").unwrap())]);
}

#[test]
fn a_stop_before_the_first_post_reports_nothing_sent_not_an_uncertain_range() {
    let rig = rig();
    rig.ready();
    let clock = rig.clock.clone();
    // The session runs out while the focus is read, before anything is posted.
    *rig.desktop.during_focused_element.lock().unwrap() = Some(Box::new(move || clock.advance(SESSION_MS + 1)));
    let result = rig.call("type", json!({"text": "hello"}));
    assert!(is_error(&result), "{result}");
    assert_eq!(first_text(&result), format!("{STOPPED} Typing stopped after 0 of 5 characters."));
    assert!(rig.desktop.events().is_empty());
}

#[test]
fn typing_post_failures_keep_the_error_and_report_the_uncertain_character_range() {
    for (text, failed_post, expected, sent) in [
        ("a".repeat(45), 2, "Typing stopped within characters 21-40 of 45.", vec![InputEvent::Text("a".repeat(20))]),
        ("🦀é".into(), 1, "Typing stopped within characters 1-2 of 2.", vec![]),
        ("a\nnext".into(), 2, "Typing stopped within characters 2-2 of 6.", vec![InputEvent::Text("a".into())]),
    ] {
        let rig = rig();
        rig.ready();
        *rig.desktop.fail_post.lock().unwrap() = Some((failed_post, "Native input failed.".into()));
        let result = rig.call("type", json!({"text": text}));
        assert!(is_error(&result), "{result}");
        assert_eq!(first_text(&result), format!("Native input failed. {expected}"));
        assert_eq!(rig.desktop.events(), sent);
    }
}

#[test]
fn a_failed_key_post_keeps_the_error() {
    let rig = rig();
    rig.ready();
    *rig.desktop.fail_post.lock().unwrap() = Some((1, "Native input failed.".into()));
    let result = rig.call("key", json!({"combo": "tab"}));
    assert!(is_error(&result), "{result}");
    assert_eq!(first_text(&result), "Native input failed.");
    assert!(rig.desktop.events().is_empty());
}

#[test]
fn typing_sends_every_chunk_and_return_while_the_recipient_stays() {
    for focused_pid in [None, Some(401)] {
        let rig = rig();
        rig.ready();
        *rig.desktop.focused_app.lock().unwrap() = focused_pid.map(|pid| app(pid, "com.example.editor", "Editor"));
        let result = rig.call("type", json!({"text": format!("{}\nend", "a".repeat(45))}));
        assert!(!is_error(&result), "{result}");
        let return_key = InputEvent::Key(parse_combo("return").unwrap());
        let text = |n: usize| InputEvent::Text("a".repeat(n));
        let typed = vec![text(20), text(20), text(5), return_key, InputEvent::Text("end".into())];
        assert_eq!(rig.desktop.events(), typed);
        // A key goes the same way.
        assert!(!is_error(&rig.call("key", json!({"combo": "tab"}))));
        assert_eq!(rig.desktop.events()[typed.len()..], [InputEvent::Key(parse_combo("tab").unwrap())]);
    }
}

#[test]
fn typing_with_unchanged_front_posts_all_chunks() {
    let rig = rig();
    rig.ready();
    let result = rig.call("type", json!({"text": "a".repeat(45)}));
    assert!(!is_error(&result), "{result}");
    assert!(first_text(&result).contains("Type 45 characters in Safari: done."), "{result}");
    assert_eq!(
        rig.desktop.events(),
        vec![InputEvent::Text("a".repeat(20)), InputEvent::Text("a".repeat(20)), InputEvent::Text("a".repeat(5))]
    );
}

#[test]
fn keyboard_actions_check_the_frontmost_app() {
    let rig = rig();
    rig.ready();
    *rig.desktop.frontmost.lock().unwrap() = Some(app(302, KEYCHAIN.0, KEYCHAIN.1));
    assert!(is_error(&rig.call("type", json!({"text": "hello"}))));
    assert!(is_error(&rig.call("key", json!({"combo": "cmd+a"}))));
    assert!(rig.desktop.events().is_empty());
}

#[test]
fn keyboard_actions_check_the_focused_app_too() {
    let rig = rig();
    rig.ready();
    // A non-activating panel takes the keys while Safari stays frontmost.
    *rig.desktop.focused_app.lock().unwrap() = Some(app(303, PASSWORDS.0, PASSWORDS.1));
    let result = rig.call("type", json!({"text": "hello"}));
    assert!(first_text(&result).contains("1Password"), "{result}");
    assert!(is_error(&rig.call("key", json!({"combo": "return"}))));
    assert!(rig.desktop.events().is_empty());
}

#[test]
fn pointer_actions_where_no_app_owns_the_point_are_refused() {
    let rig = rig();
    rig.ready();
    *rig.desktop.owner_at.lock().unwrap() = Box::new(|_| None);
    for (name, args) in [
        ("click", json!({"x": 10, "y": 10})),
        ("move", json!({"x": 10, "y": 10})),
        ("scroll", json!({"x": 10, "y": 10, "dx": 0, "dy": 10})),
        ("drag", json!({"fromX": 10, "fromY": 10, "toX": 20, "toY": 20})),
    ] {
        let result = rig.call(name, args);
        assert!(is_error(&result), "{name}");
        assert!(first_text(&result).contains("which app"), "{result}");
    }
    assert!(rig.desktop.events().is_empty());
}

#[test]
fn opening_a_blocked_app_is_refused() {
    let rig = rig();
    rig.ready();
    let result = rig.call("open_app", json!({"name": BLOCKED_OPEN}));
    assert_eq!(first_text(&result), "Ailfred does not control Keychain Access. The app was not opened.");
    assert!(!is_error(&rig.call("open_app", json!({"name": "com.apple.mail"}))));
    assert_eq!(*rig.desktop.opened.lock().unwrap(), vec!["com.apple.mail".to_string()]);
}

#[test]
fn more_than_ten_calls_per_second_are_refused() {
    let rig = rig();
    for _ in 0..10 {
        assert!(!is_error(&rig.call("list_apps", json!({}))));
    }
    let result = rig.call("list_apps", json!({}));
    assert!(is_error(&result));
    assert!(first_text(&result).contains("10"), "{result}");
    rig.clock.advance(1000);
    assert!(!is_error(&rig.call("list_apps", json!({}))));
}

#[test]
fn calls_refused_before_they_act_do_not_use_up_the_rate_limit() {
    // The user declines the session twelve times in the same second, then allows it: nothing
    // ran yet, so the next call isn't "too many actions".
    let rig = rig_with(false, true);
    for _ in 0..12 {
        let result = rig.call("list_apps", json!({}));
        assert!(is_error(&result) && !first_text(&result).contains("Too many"), "{result}");
    }
    // Nor do calls refused for a missing permission.
    rig.desktop.permissions.lock().unwrap().screen_recording = false;
    for _ in 0..12 {
        assert!(first_text(&rig.call("screenshot", json!({}))).contains("Screen Recording"));
    }
    rig.desktop.permissions.lock().unwrap().screen_recording = true;
    *rig.dialogs.allow.lock().unwrap() = true;
    let result = rig.call("list_apps", json!({}));
    assert!(!is_error(&result), "{result}");
}

// --- Risky actions ---

fn send_button() -> Vec<AxElement> {
    vec![AxElement { role: "AXButton".into(), title: Some("Send".into()), ..AxElement::default() }]
}

#[test]
fn a_risky_click_needs_a_confirmation_showing_action_and_target() {
    let rig = rig();
    rig.ready();
    *rig.desktop.elements.lock().unwrap() = send_button();
    let result = rig.call("click", json!({"x": 10, "y": 10}));
    assert!(!is_error(&result), "{result}");
    let asked = rig.dialogs.confirmations.lock().unwrap().clone();
    assert_eq!(asked.len(), 1);
    assert!(asked[0].contains("Send") && asked[0].contains("Safari"), "{asked:?}");
    assert_eq!(rig.desktop.events().len(), 1);
}

#[test]
fn a_declined_confirmation_is_a_tool_error_and_nothing_happens() {
    let rig = rig_with(true, false);
    rig.ready();
    *rig.desktop.elements.lock().unwrap() = send_button();
    let result = rig.call("click", json!({"x": 10, "y": 10}));
    assert!(is_error(&result));
    assert!(first_text(&result).contains("declined"), "{result}");
    assert!(rig.desktop.events().is_empty());
}

#[test]
fn harmless_actions_are_not_confirmed() {
    let rig = rig();
    rig.ready();
    rig.call("click", json!({"x": 10, "y": 10}));
    rig.call("key", json!({"combo": "cmd+t"}));
    assert!(rig.dialogs.confirmations.lock().unwrap().is_empty());
    assert_eq!(rig.desktop.events().len(), 2);
}

#[test]
fn where_the_app_tells_nothing_a_click_or_return_asks_and_a_delete_shortcut_always_does() {
    let rig = rig_with(true, false);
    rig.ready();
    rig.desktop.elements.lock().unwrap().clear();
    *rig.desktop.focus.lock().unwrap() = None;
    for (name, arguments) in [("click", json!({"x": 10, "y": 10})), ("key", json!({"combo": "return"})), ("type", json!({"text": "ok\n"}))] {
        let result = rig.call(name, arguments);
        assert!(first_text(&result).contains("declined"), "{name}: {result}");
    }
    let asked = rig.dialogs.confirmations.lock().unwrap().clone();
    assert!(asked.len() == 3 && asked.iter().all(|a| a.contains("Accessibility")), "{asked:?}");
    assert!(rig.desktop.events().is_empty());
    let rig = rig_with(true, false);
    rig.ready();
    // A file selected in a Finder list.
    *rig.desktop.focus.lock().unwrap() = Some(Focus { element: AxElement { role: "AXRow".into(), title: Some("Report.pdf".into()), ..AxElement::default() }, ..Focus::default() });
    let result = rig.call("key", json!({"combo": "cmd+backspace"}));
    assert!(first_text(&result).contains("declined"), "{result}");
    // Typing words with nothing to read presses nothing; it doesn't ask.
    rig.desktop.elements.lock().unwrap().clear();
    *rig.desktop.focus.lock().unwrap() = None;
    assert!(!is_error(&rig.call("type", json!({"text": "two words"}))));
    let asked = rig.dialogs.confirmations.lock().unwrap().clone();
    assert!(asked.len() == 1 && asked[0].contains("cmd+delete"), "{asked:?}");
    assert!(rig.desktop.events().iter().all(|e| matches!(e, InputEvent::Text(_))), "only the typed words");
}

#[test]
fn a_drag_over_a_risky_button_is_confirmed() {
    let rig = rig_with(true, false);
    rig.ready();
    *rig.desktop.elements.lock().unwrap() = send_button();
    let result = rig.call("drag", json!({"fromX": 10, "fromY": 10, "toX": 10, "toY": 10}));
    assert!(first_text(&result).contains("declined"), "{result}");
    assert_eq!(rig.dialogs.confirmations.lock().unwrap().len(), 1);
    assert!(rig.desktop.events().is_empty());
}

#[test]
fn space_on_a_focused_send_button_is_confirmed() {
    let rig = rig_with(true, false);
    rig.ready();
    *rig.desktop.focus.lock().unwrap() = Some(Focus { element: send_button().remove(0), ..Focus::default() });
    assert!(is_error(&rig.call("key", json!({"combo": "space"}))));
    assert!(is_error(&rig.call("type", json!({"text": " "}))));
    assert!(rig.desktop.events().is_empty());
    assert!(!is_error(&rig.call("type", json!({"text": "ok"}))));
    assert_eq!(rig.dialogs.confirmations.lock().unwrap().len(), 2);
}

#[test]
fn return_in_a_send_like_field_is_confirmed() {
    let rig = rig_with(true, false);
    rig.ready();
    let composer = AxElement { role: "AXTextArea".into(), description: Some("Message".into()), ..AxElement::default() };
    *rig.desktop.focus.lock().unwrap() = Some(Focus { element: composer, ..Focus::default() });
    assert!(is_error(&rig.call("key", json!({"combo": "cmd+return"}))));
    assert!(rig.desktop.events().is_empty());
}

#[test]
fn a_risky_intent_adds_a_confirmation() {
    let rig = rig();
    rig.ready();
    rig.call("scroll", json!({"x": 1, "y": 1, "dx": 0, "dy": 10, "intent": "buy the ticket"}));
    assert_eq!(rig.dialogs.confirmations.lock().unwrap().len(), 1);
}

#[test]
fn an_approved_action_is_dropped_when_the_previous_app_cannot_be_brought_back() {
    let rig = rig();
    rig.ready();
    *rig.desktop.elements.lock().unwrap() = send_button();
    let frontmost = rig.desktop.clone();
    *rig.dialogs.during.lock().unwrap() = Some(Box::new(move || *frontmost.frontmost.lock().unwrap() = Some(ailfred())));
    let result = rig.call("click", json!({"x": 10, "y": 10}));
    assert!(is_error(&result), "{result}");
    assert!(first_text(&result).contains("could not be brought back"), "{result}");
    // Once after the dialog, then once per try.
    let activated = rig.desktop.activated.lock().unwrap().clone();
    assert_eq!(activated, vec![200; 1 + RESTORE_TRIES as usize]);
    assert!(rig.desktop.events().is_empty());
}

#[test]
fn an_approved_action_is_dropped_when_no_app_was_known_to_be_in_front() {
    let rig = rig();
    rig.ready();
    *rig.desktop.elements.lock().unwrap() = send_button();
    *rig.desktop.frontmost.lock().unwrap() = None;
    let result = rig.call("click", json!({"x": 10, "y": 10}));
    assert!(is_error(&result), "{result}");
    assert!(first_text(&result).contains("cannot tell which app was in front"), "{result}");
    assert_eq!(rig.dialogs.confirmations.lock().unwrap().len(), 1, "the user was asked");
    assert!(rig.desktop.events().is_empty());
}

#[test]
fn an_approved_action_waits_a_moment_for_the_previous_app_to_come_back() {
    // How long a risky click takes when the previous app is back after `needed` activations.
    let elapsed = |needed: usize| {
        let rig = rig();
        rig.ready();
        *rig.desktop.elements.lock().unwrap() = send_button();
        *rig.desktop.activates_to.lock().unwrap() = rig.desktop.frontmost_app();
        *rig.desktop.activations_needed.lock().unwrap() = needed;
        let frontmost = rig.desktop.clone();
        *rig.dialogs.during.lock().unwrap() =
            Some(Box::new(move || *frontmost.frontmost.lock().unwrap() = Some(ailfred())));
        let start = rig.clock.now_ms();
        let result = rig.call("click", json!({"x": 10, "y": 10}));
        assert!(!is_error(&result), "{result}");
        assert_eq!(rig.desktop.activated.lock().unwrap().len(), needed);
        assert_eq!(rig.desktop.events().len(), 1);
        rig.clock.now_ms() - start
    };
    // Back at the third activation: two more tries, each after a step's wait.
    assert_eq!(elapsed(3) - elapsed(1), 2 * RESTORE_STEP_MS);
}

#[test]
fn an_approved_action_runs_once_the_previous_app_is_back() {
    let rig = rig();
    rig.ready();
    *rig.desktop.elements.lock().unwrap() = send_button();
    *rig.desktop.activates_to.lock().unwrap() = rig.desktop.frontmost_app();
    let frontmost = rig.desktop.clone();
    *rig.dialogs.during.lock().unwrap() = Some(Box::new(move || *frontmost.frontmost.lock().unwrap() = Some(ailfred())));
    let result = rig.call("click", json!({"x": 10, "y": 10}));
    assert!(!is_error(&result), "{result}");
    assert_eq!(rig.desktop.events().len(), 1);
}

#[test]
fn the_previous_app_is_brought_back_after_a_dialog() {
    let rig = rig();
    rig.ready();
    *rig.desktop.elements.lock().unwrap() = send_button();
    *rig.desktop.activates_to.lock().unwrap() = rig.desktop.frontmost_app();
    let frontmost = rig.desktop.clone();
    *rig.dialogs.during.lock().unwrap() = Some(Box::new(move || *frontmost.frontmost.lock().unwrap() = Some(ailfred())));
    rig.call("click", json!({"x": 10, "y": 10}));
    assert_eq!(*rig.desktop.activated.lock().unwrap(), vec![200]);
}

// --- The front app changed since the last screenshot ---

#[test]
fn keys_and_text_wait_for_a_screenshot_once_the_front_app_changed() {
    let rig = rig();
    rig.ready();
    *rig.desktop.frontmost.lock().unwrap() = Some(app(300, "com.example.terminal", "Terminal"));
    for (tool, args) in [("key", json!({"combo": "alt+f4"})), ("type", json!({"text": "hola"}))] {
        let result = rig.call(tool, args);
        assert!(is_error(&result), "{result}");
        assert_eq!(
            first_text(&result),
            "Terminal is in front now, not the app of the latest screenshot, so nothing was sent. Take a screenshot first."
        );
    }
    assert!(rig.desktop.events().is_empty());
    // A new screenshot shows the change: keys go through again.
    rig.ready();
    let result = rig.call("key", json!({"combo": "alt+f4"}));
    assert!(!is_error(&result), "{result}");
    assert_eq!(rig.desktop.events().len(), 1);
}

// --- Typing ---

#[test]
fn typing_sends_newlines_as_return_and_checks_them() {
    let rig = rig();
    rig.ready();
    let result = rig.call("type", json!({"text": "hi\nthere"}));
    assert!(!is_error(&result), "{result}");
    assert_eq!(
        rig.desktop.events(),
        vec![InputEvent::Text("hi".into()), InputEvent::Key(parse_combo("return").unwrap()), InputEvent::Text("there".into())]
    );
    // In a send-like field the newline needs a confirmation.
    let rig = rig_with(true, false);
    rig.ready();
    let composer = AxElement { role: "AXTextField".into(), title: Some("Reply".into()), ..AxElement::default() };
    *rig.desktop.focus.lock().unwrap() = Some(Focus { element: composer, ..Focus::default() });
    assert!(is_error(&rig.call("type", json!({"text": "ok\n"}))));
    assert!(rig.desktop.events().is_empty());
}

#[test]
fn typing_control_characters_is_refused() {
    let rig = rig();
    rig.ready();
    for text in ["\u{1b}[A", "a\u{0}b", "\u{7f}", "\u{8}\u{8}"] {
        let result = rig.call("type", json!({"text": text}));
        assert!(is_error(&result), "{text:?}");
        assert!(first_text(&result).contains("control character"), "{result}");
    }
    assert!(rig.desktop.events().is_empty());
    assert!(!is_error(&rig.call("type", json!({"text": "a\tb"}))));
    assert_eq!(rig.desktop.events(), vec![InputEvent::Text("a\tb".into())]);
}

#[test]
fn typing_unicode_line_breaks_is_refused() {
    let rig = rig();
    rig.ready();
    for text in ["a\u{2028}b", "a\u{2029}b", "a\u{85}b"] {
        let result = rig.call("type", json!({"text": text}));
        assert!(is_error(&result), "{text:?}");
        assert!(first_text(&result).contains("control character"), "{result}");
    }
    assert!(rig.desktop.events().is_empty());
}

#[test]
fn typing_space_or_return_after_a_tab_is_refused() {
    // The tab may move the focus onto a send button the risk check never saw.
    let rig = rig();
    rig.ready();
    for text in ["hello\t ", "hello\t\n", "a\tb c"] {
        let result = rig.call("type", json!({"text": text}));
        assert!(is_error(&result), "{text:?}");
        assert!(first_text(&result).contains("after a tab"), "{result}");
    }
    assert!(rig.desktop.events().is_empty());
    assert!(!is_error(&rig.call("type", json!({"text": "a b\tc"}))));
    assert_eq!(rig.desktop.events(), vec![InputEvent::Text("a b\tc".into())]);
}

#[test]
fn long_text_is_sent_in_chunks_of_twenty_utf16_units() {
    let rig = rig();
    rig.ready();
    let text = "ñ".repeat(25) + "😀";
    rig.call("type", json!({"text": text}));
    let chunks: Vec<String> = rig
        .desktop
        .events()
        .into_iter()
        .map(|e| match e {
            InputEvent::Text(t) => t,
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(chunks.concat(), text);
    assert!(chunks.iter().all(|c| c.encode_utf16().count() <= 20), "{chunks:?}");
}

#[test]
fn typing_nothing_sends_nothing() {
    let rig = rig();
    rig.ready();
    assert!(!is_error(&rig.call("type", json!({"text": ""}))));
    assert!(rig.desktop.events().is_empty());
}

#[test]
fn refused_declined_and_stopped_actions_send_nothing() {
    // Declined: a risky click the user says no to.
    let declined = rig_with(true, false);
    declined.ready();
    *declined.desktop.elements.lock().unwrap() = send_button();
    assert!(first_text(&declined.call("click", json!({"x": 10, "y": 10}))).contains("declined"));
    assert!(is_error(&declined.call("type", json!({"text": "hello\t "}))));
    // Refused: a blocked window, a point no app owns, a blocked app in front, invalid arguments.
    let rig_blocked = rig();
    rig_blocked.ready();
    *rig_blocked.desktop.owner_at.lock().unwrap() = Box::new(|_| Some(app(300, SETTINGS.0, SETTINGS.1)));
    assert!(is_error(&rig_blocked.call("click", json!({"x": 10, "y": 10}))));
    assert!(is_error(&rig_blocked.call("drag", json!({"fromX": 1, "fromY": 1, "toX": 2, "toY": 2}))));
    *rig_blocked.desktop.owner_at.lock().unwrap() = Box::new(|_| None);
    assert!(is_error(&rig_blocked.call("scroll", json!({"x": 1, "y": 1, "dx": 0, "dy": 1}))));
    *rig_blocked.desktop.frontmost.lock().unwrap() = Some(app(302, KEYCHAIN.0, KEYCHAIN.1));
    assert!(is_error(&rig_blocked.call("key", json!({"combo": "cmd+a"}))));
    assert!(is_error(&rig_blocked.call("click", json!({"x": 1}))));
    // Stopped: queued before a panic, no session (denied), no screenshot yet.
    let rig_stopped = rig();
    rig_stopped.ready();
    let queued_at = rig_stopped.control.epoch();
    rig_stopped.control.end(Reason::Panic);
    assert!(is_error(&rig_stopped.helper.call_tool_from(queued_at, "move", &json!({"x": 1, "y": 1})).unwrap()));
    let rig_denied = rig_with(false, true);
    assert!(is_error(&rig_denied.call("key", json!({"combo": "tab"}))));
    let rig_fresh = rig();
    assert!(is_error(&rig_fresh.call("click", json!({"x": 1, "y": 1}))));
    for r in [&declined, &rig_blocked, &rig_stopped, &rig_denied, &rig_fresh] {
        assert!(r.desktop.events().is_empty(), "{:?}", r.desktop.events());
    }
}

// --- Speed ---

#[test]
fn an_action_settles_150_ms_and_open_app_still_1500() {
    let rig = rig();
    rig.ready();
    let before = rig.clock.now_ms();
    rig.call("click", json!({"x": 10, "y": 10}));
    assert_eq!(rig.clock.now_ms() - before, 150);
    let before = rig.clock.now_ms();
    rig.call("open_app", json!({"name": "Mail"}));
    assert_eq!(rig.clock.now_ms() - before, 1500);
}

#[test]
fn a_short_message_with_an_emoji_is_two_events_and_one_settle() {
    // Two chunks (four keyboard events, 15 ms apart on the Mac) plus one settle; no confirmation
    // is asked.
    let rig = rig();
    rig.ready();
    let message = "¡Hola, Panda! Te quiero mucho ❤️";
    let before = rig.clock.now_ms();
    assert!(!is_error(&rig.call("type", json!({"text": message}))));
    assert_eq!(rig.clock.now_ms() - before, 150);
    assert!(rig.dialogs.confirmations.lock().unwrap().is_empty());
    let events = rig.desktop.events();
    assert_eq!(events.len(), 2, "{events:?}");
    let typed: String = events
        .into_iter()
        .map(|e| match e {
            InputEvent::Text(t) => t,
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(typed, message);
}

// --- Batches ---

fn batch(steps: Value) -> Value {
    json!({"steps": steps})
}

fn image_count(result: &Value) -> usize {
    result["content"].as_array().map_or(0, |c| c.iter().filter(|b| b["type"] == "image").count())
}

#[test]
fn tools_list_offers_batch_for_predictable_sequences() {
    let rig = rig();
    let body = rig.rpc("tools/list", json!({}));
    let tools = body["result"]["tools"].as_array().unwrap();
    let batch = tools.iter().find(|t| t["name"] == "batch").unwrap();
    let steps = &batch["inputSchema"]["properties"]["steps"];
    assert_eq!((steps["minItems"].as_u64(), steps["maxItems"].as_u64()), (Some(1), Some(20)));
    assert_eq!(steps["items"]["properties"]["tool"]["enum"], json!(["click", "move", "drag", "scroll", "type", "key", "open_app", "wait"]));
    assert!(batch["description"].as_str().unwrap().contains("screenshot"));
    let instructions = rig.rpc("initialize", json!({}))["result"]["instructions"].as_str().unwrap().to_string();
    assert!(instructions.contains("batch"), "{instructions}");
}

#[test]
fn a_batch_runs_its_steps_in_order_and_returns_one_screenshot() {
    let rig = rig();
    rig.ready();
    let result = rig.call(
        "batch",
        batch(json!([
            {"tool": "click", "x": 100, "y": 50},
            {"tool": "type", "text": "Ana"},
            {"tool": "key", "combo": "tab"},
            {"tool": "type", "text": "Hi there"},
            {"tool": "scroll", "x": 5, "y": 6, "dx": 0, "dy": 120},
        ])),
    );
    assert!(!is_error(&result), "{result}");
    let p = |x, y| Point { x, y };
    assert_eq!(
        rig.desktop.events(),
        vec![
            // Mapped with the screenshot taken before the batch (scale 0.5).
            InputEvent::Click { at: p(200.0, 100.0), button: MouseButton::Left, count: 1 },
            InputEvent::Text("Ana".into()),
            InputEvent::Key(parse_combo("tab").unwrap()),
            InputEvent::Text("Hi there".into()),
            InputEvent::Scroll { at: p(10.0, 12.0), dx: 0, dy: 120 },
        ]
    );
    assert_eq!(rig.desktop.captures.load(Ordering::SeqCst), 2);
    assert_eq!(image_count(&result), 1);
    let summary = first_text(&result);
    assert!(summary.contains("5 of 5") && summary.contains("Type 3 characters"), "{summary}");
}

#[test]
fn a_risky_step_asks_once_and_a_decline_stops_the_batch_after_the_earlier_steps() {
    let rig = rig_with(true, false);
    rig.ready();
    *rig.desktop.elements.lock().unwrap() = send_button();
    let result = rig.call(
        "batch",
        batch(json!([
            {"tool": "type", "text": "hi"},
            {"tool": "click", "x": 10, "y": 10},
            {"tool": "type", "text": "never"},
        ])),
    );
    assert!(is_error(&result));
    let message = first_text(&result);
    assert!(message.contains("declined") && message.contains("step 2"), "{message}");
    assert!(message.contains("1 of 3"), "{message}");
    assert_eq!(rig.dialogs.confirmations.lock().unwrap().len(), 1);
    assert_eq!(rig.desktop.events(), vec![InputEvent::Text("hi".into())]);
    // A fresh screenshot shows where the batch stopped.
    assert_eq!(image_count(&result), 1);
}

#[test]
fn a_blocklisted_step_stops_the_batch() {
    let rig = rig();
    rig.ready();
    *rig.desktop.owner_at.lock().unwrap() =
        Box::new(|at: Point| if at.x > 100.0 { Some(app(300, SETTINGS.0, SETTINGS.1)) } else { Some(safari()) });
    let result = rig.call(
        "batch",
        batch(json!([
            {"tool": "click", "x": 10, "y": 10},
            {"tool": "click", "x": 200, "y": 10},
            {"tool": "click", "x": 20, "y": 20},
        ])),
    );
    assert!(is_error(&result));
    assert!(first_text(&result).contains(SETTINGS.1), "{result}");
    assert_eq!(rig.desktop.events().len(), 1);
}

#[test]
fn a_front_app_switch_stops_a_batch_of_keys() {
    let rig = rig();
    rig.ready();
    *rig.desktop.front_after_posts.lock().unwrap() = Some((1, app(400, "com.apple.mail", "Mail")));
    let result = rig.call(
        "batch",
        batch(json!([
            {"tool": "type", "text": "hi"},
            {"tool": "type", "text": "never"},
            {"tool": "type", "text": "later"}
        ])),
    );
    assert!(is_error(&result), "{result}");
    assert_eq!(
        first_text(&result),
        "Stopped at step 2 of 3 (type): Mail is in front now, not the app of the latest screenshot, so nothing was sent. \
Take a screenshot first.\nRan 1 of 3 steps:\n1. Type 2 characters in Safari: done."
    );
    assert_eq!(rig.desktop.events(), vec![InputEvent::Text("hi".into())]);
}

#[test]
fn a_panic_during_a_batch_drops_the_remaining_steps() {
    let rig = rig();
    rig.ready();
    let control = rig.control.clone();
    *rig.clock.on_sleep.lock().unwrap() = Some(Box::new(move || control.end(Reason::Panic)));
    let result = rig.call(
        "batch",
        batch(json!([
            {"tool": "click", "x": 10, "y": 10},
            {"tool": "click", "x": 20, "y": 20},
            {"tool": "click", "x": 30, "y": 30},
        ])),
    );
    assert!(is_error(&result));
    assert!(first_text(&result).contains("stopped"), "{result}");
    assert_eq!(rig.desktop.events().len(), 1);
    // No screenshot once the session ended.
    assert_eq!(rig.desktop.captures.load(Ordering::SeqCst), 1);
    assert_eq!(image_count(&result), 0);
}

#[test]
fn malformed_batches_are_refused_before_anything_runs() {
    let rig = rig();
    let wait = json!({"tool": "wait", "ms": 0});
    let too_many = Value::Array(vec![wait.clone(); 21]);
    for args in [
        json!({}),
        batch(json!("click")),
        batch(json!([])),
        batch(too_many),
        batch(json!(["click"])),
        batch(json!([{"tool": "batch", "steps": [wait]}])),
        batch(json!([{"tool": "format_disk"}])),
        batch(json!([{"tool": "screenshot"}])),
        batch(json!([{"x": 1, "y": 1}])),
        batch(json!([{"tool": "wait", "ms": 0}, {"tool": "click", "x": 1}])),
    ] {
        let result = rig.call("batch", args.clone());
        assert!(is_error(&result), "{args}");
    }
    assert_eq!(rig.dialogs.grants.load(Ordering::SeqCst), 0);
    assert!(rig.desktop.events().is_empty());
}

#[test]
fn an_intent_in_a_batch_only_adds_checks() {
    // A harmless step intent cannot hide a risky batch intent or a risky target.
    let rig = rig_with(true, false);
    rig.ready();
    let result = rig.call(
        "batch",
        json!({"intent": "buy the ticket", "steps": [{"tool": "scroll", "x": 1, "y": 1, "dx": 0, "dy": 10, "intent": "just scrolling"}]}),
    );
    assert!(first_text(&result).contains("declined"), "{result}");
    *rig.desktop.elements.lock().unwrap() = send_button();
    let result = rig.call(
        "batch",
        json!({"intent": "look around", "steps": [{"tool": "click", "x": 1, "y": 1, "intent": "harmless"}]}),
    );
    assert!(first_text(&result).contains("declined"), "{result}");
    assert_eq!(rig.dialogs.confirmations.lock().unwrap().len(), 2);
    assert!(rig.desktop.events().is_empty());
}

#[test]
fn a_batch_waits_for_the_rate_limit_instead_of_failing() {
    // Each step counts as one action; a full window delays the batch, it does not refuse it.
    let rig = rig();
    rig.ready();
    for _ in 0..9 {
        assert!(!is_error(&rig.call("list_apps", json!({}))));
    }
    let steps: Vec<Value> = (0..3).map(|i| json!({"tool": "move", "x": i, "y": i})).collect();
    let result = rig.call("batch", batch(Value::Array(steps)));
    assert!(!is_error(&result), "{result}");
    assert_eq!(rig.desktop.events().len(), 3);
    assert!(rig.clock.now_ms() >= NOW + 1000);
}

#[test]
fn a_batch_asks_for_the_session_once() {
    let rig = rig_with(false, true);
    let result = rig.call("batch", batch(json!([{"tool": "key", "combo": "tab"}, {"tool": "key", "combo": "tab"}])));
    assert!(first_text(&result).contains("not allowed"), "{result}");
    assert_eq!(rig.dialogs.grants.load(Ordering::SeqCst), 1);
    assert!(rig.desktop.events().is_empty());
}

#[test]
fn list_apps_returns_names_and_bundle_ids() {
    let rig = rig();
    let result = rig.call("list_apps", json!({}));
    let apps: Value = serde_json::from_str(&first_text(&result)).unwrap();
    assert_eq!(
        apps,
        json!([
            {"name": "Safari", "bundleId": "com.apple.Safari", "active": true},
            {"name": "Mail", "bundleId": "com.apple.mail", "active": false},
        ])
    );
}
