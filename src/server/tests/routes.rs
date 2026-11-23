use std::io::Read;
use std::sync::Mutex;
use std::sync::mpsc::SyncSender;

use ailfred_protocol::command::{self, Command};

use super::super::hub::Frame;
use super::super::routes::{Admitted, Answer, COMMANDS, Core, EVENTS, Gate, Head, MAX_BODY, MAX_IN_FLIGHT, MAX_STREAMS, Outcome, serve};
use super::PNG;

const PORT: u16 = 4000;
const TOKEN: &str = "f00d";
const BEARER: &str = "Bearer f00d";

/// A core that keeps what it got and answers 202.
#[derive(Default)]
struct Fake {
    commands: Mutex<Vec<Command>>,
    joined: Mutex<usize>,
}

impl Core for Fake {
    fn join(&self, watcher: SyncSender<Frame>) {
        *self.joined.lock().unwrap() += 1;
        let _ = watcher.send("data: {}\n\n".into());
    }

    fn command(&self, command: Command) -> Answer {
        self.commands.lock().unwrap().push(command);
        Answer::accepted("t", "r")
    }
}

/// A body that must not be read.
struct Untouched;

impl Read for Untouched {
    fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
        panic!("the body was read")
    }
}

fn command(body: &str) -> Head<'static> {
    let _ = body;
    Head { method: "POST", path: COMMANDS, host: Some("127.0.0.1:4000"), origin: None, authorization: Some(BEARER), length: None }
}

fn events() -> Head<'static> {
    Head { method: "GET", path: EVENTS, ..command("") }
}

fn gate() -> Gate {
    Gate::new(PORT, TOKEN.into())
}

/// What a request gets, its body read from `body`.
fn handle(gate: &Gate, head: &Head, body: &mut dyn Read, core: &dyn Core) -> Result<Outcome, Answer> {
    Ok(serve(gate.admit(head)?, body, core))
}

fn status(gate: &Gate, head: &Head) -> u16 {
    match handle(gate, head, &mut Untouched, &Fake::default()) {
        Ok(_) => panic!("let in"),
        Err(answer) => answer.status,
    }
}

#[test]
fn a_host_other_than_this_servers_loopback_address_is_refused() {
    let gate = gate();
    for host in [Some("evil.example:4000"), Some("127.0.0.1:4001"), Some("127.0.0.1"), Some("0.0.0.0:4000"), None] {
        assert_eq!(status(&gate, &Head { host, ..command("") }), 403, "{host:?}");
    }
    for host in ["127.0.0.1:4000", "localhost:4000", "LOCALHOST:4000"] {
        assert!(gate.admit(&Head { host: Some(host), ..command("") }).is_ok(), "{host}");
    }
}

#[test]
fn a_host_port_is_plain_digits_without_a_sign_or_a_leading_zero() {
    let gate = gate();
    for host in ["127.0.0.1:+4000", "127.0.0.1:04000", "localhost:-4000", "127.0.0.1: 4000", "127.0.0.1:"] {
        assert_eq!(status(&gate, &Head { host: Some(host), ..command("") }), 403, "{host}");
    }
}

#[test]
fn a_request_from_a_web_page_is_refused_whatever_its_origin() {
    for origin in ["http://evil.example", "http://127.0.0.1:4000", "null"] {
        assert_eq!(status(&gate(), &Head { origin: Some(origin), ..command("") }), 403, "{origin}");
    }
}

#[test]
fn without_the_token_the_body_is_never_read() {
    for authorization in [None, Some("Bearer nope"), Some("Basic f00d"), Some("Bearer f00d0"), Some("f00d")] {
        assert_eq!(status(&gate(), &Head { authorization, length: Some(10), ..command("") }), 401, "{authorization:?}");
    }
    let answer = gate().admit(&Head { authorization: None, ..command("") }).unwrap_err();
    assert!(answer.body["error"].is_string());
}

#[test]
fn unknown_paths_are_404_and_wrong_methods_405() {
    let gate = gate();
    assert_eq!(status(&gate, &Head { path: "/v1/nope", ..command("") }), 404);
    assert_eq!(status(&gate, &Head { path: "/", method: "GET", ..command("") }), 404);
    assert_eq!(status(&gate, &Head { method: "GET", ..command("") }), 405);
    assert_eq!(status(&gate, &Head { method: "POST", ..events() }), 405);
}

#[test]
fn a_body_over_16_mib_is_refused_said_or_found() {
    let gate = gate();
    assert_eq!(status(&gate, &Head { length: Some(MAX_BODY + 1), ..command("") }), 413, "by its Content-Length, unread");
    let mut long = std::io::repeat(b' ').take(MAX_BODY + 1);
    match handle(&gate, &command(""), &mut long, &Fake::default()) {
        Ok(Outcome::Answer(answer)) => assert_eq!(answer.status, 413),
        _ => panic!("taken"),
    }
}

#[test]
fn the_ninth_request_in_flight_and_the_fifth_stream_are_turned_away() {
    let gate = gate();
    let held: Vec<Admitted> = (0..MAX_IN_FLIGHT).map(|_| gate.admit(&command("")).unwrap()).collect();
    assert_eq!(gate.admit(&command("")).unwrap_err().status, 503);
    drop(held);
    assert!(gate.admit(&command("")).is_ok(), "room again once they're done");
    let streams: Vec<Admitted> = (0..MAX_STREAMS).map(|_| gate.admit(&events()).unwrap()).collect();
    assert_eq!(gate.admit(&events()).unwrap_err().status, 503);
    assert!(gate.admit(&command("")).is_ok(), "commands still go");
    drop(streams);
    assert!(gate.admit(&events()).is_ok());
}

#[test]
fn a_malformed_command_is_400_with_a_short_reason() {
    let core = Fake::default();
    for body in ["{nope", r#"{"type":"jump"}"#, r#"{"type":"stop"}"#, ""] {
        match handle(&gate(), &command(body), &mut body.as_bytes(), &core) {
            Ok(Outcome::Answer(answer)) => {
                assert_eq!(answer.status, 400, "{body}");
                let reason = answer.body["error"].as_str().unwrap();
                assert!(!reason.is_empty() && reason.len() <= 200, "{reason}");
            }
            _ => panic!("taken: {body}"),
        }
    }
    assert!(core.commands.lock().unwrap().is_empty());
}

#[test]
fn a_command_reaches_the_core_and_its_answer_comes_back() {
    let core = Fake::default();
    let body = r#"{"type":"stop","threadId":"t","runId":"r"}"#;
    match handle(&gate(), &command(body), &mut body.as_bytes(), &core) {
        Ok(Outcome::Answer(answer)) => assert_eq!(answer, Answer::accepted("t", "r")),
        _ => panic!("no answer"),
    }
    assert_eq!(*core.commands.lock().unwrap(), [Command::Stop { thread_id: "t".into(), run_id: "r".into() }]);
}

/// A core whose chat has no hub yet: the watcher goes unused.
struct Unfed;

impl Core for Unfed {
    fn join(&self, _: SyncSender<Frame>) {}

    fn command(&self, _: Command) -> Answer {
        unreachable!("no command here")
    }
}

#[test]
fn a_join_before_the_chat_feeds_the_hub_is_503_not_an_empty_stream() {
    match handle(&gate(), &events(), &mut Untouched, &Unfed) {
        Ok(Outcome::Answer(answer)) => assert_eq!(answer.status, 503, "{answer:?}"),
        Ok(Outcome::Stream { .. }) => panic!("an empty stream"),
        Err(answer) => panic!("refused: {answer:?}"),
    }
}

/// A PNG whose header says `width`×`height`, and nothing after it.
fn png_claiming(width: u32, height: u32) -> Vec<u8> {
    let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
    png.extend(width.to_be_bytes());
    png.extend(height.to_be_bytes());
    // Depth, color type, compression, filter, interlace, and a CRC never read.
    png.extend([8, 6, 0, 0, 0, 0, 0, 0, 0]);
    png
}

fn answer_to(core: &Fake, body: &str) -> Answer {
    match handle(&gate(), &command(body), &mut body.as_bytes(), core) {
        Ok(Outcome::Answer(answer)) => answer,
        _ => panic!("no answer"),
    }
}

#[test]
fn an_image_too_large_or_broken_is_400_before_the_chat_gets_it() {
    let core = Fake::default();
    let cases = [
        (png_claiming(100_000, 100_000), "100000×100000"),
        (png_claiming(0, 10), "no width"),
        (png_claiming(8193, 10), "wider than 8192"),
        (png_claiming(8000, 8000), "64 megapixels"),
        (PNG[..40].to_vec(), "truncated"),
    ];
    for (png, why) in cases {
        let body = serde_json::to_string(&command::send("t", "c1", &[], "look", &[&png])).unwrap();
        let answer = answer_to(&core, &body);
        assert_eq!(answer.status, 400, "{why}: {answer:?}");
    }
    assert!(core.commands.lock().unwrap().is_empty(), "none reached the chat");
    let body = serde_json::to_string(&command::send("t", "c1", &[], "look", &[PNG])).unwrap();
    assert_eq!(answer_to(&core, &body).status, 202, "a whole PNG goes");
}

#[test]
fn a_message_with_more_than_eight_images_is_400() {
    let core = Fake::default();
    let nine = vec![PNG; 9];
    let body = serde_json::to_string(&command::send("t", "c1", &[], "look", &nine)).unwrap();
    let answer = answer_to(&core, &body);
    assert_eq!(answer.status, 400, "{answer:?}");
    assert!(answer.body["error"].as_str().unwrap().contains("8 images"), "{answer:?}");
    assert!(core.commands.lock().unwrap().is_empty());
    let eight = vec![PNG; 8];
    let body = serde_json::to_string(&command::send("t", "c1", &[], "look", &eight)).unwrap();
    assert_eq!(answer_to(&core, &body).status, 202, "eight go");
}

#[test]
fn images_over_40_megapixels_in_all_are_400_by_their_headers_before_any_is_decoded() {
    let core = Fake::default();
    // Each fits alone (32.8 megapixels), and has no image data: decoding either would fail.
    let (a, b) = (png_claiming(8192, 4000), png_claiming(8192, 4000));
    let body = serde_json::to_string(&command::send("t", "c1", &[], "look", &[&a, &b])).unwrap();
    let answer = answer_to(&core, &body);
    assert_eq!(answer.status, 400, "{answer:?}");
    assert!(answer.body["error"].as_str().unwrap().contains("in all"), "refused by size, not by decoding: {answer:?}");
    assert!(core.commands.lock().unwrap().is_empty());
}

#[test]
fn the_events_route_joins_the_core_and_streams_what_it_sends() {
    let core = Fake::default();
    match handle(&gate(), &events(), &mut Untouched, &core) {
        Ok(Outcome::Stream { first, .. }) => assert_eq!(first, "data: {}\n\n"),
        _ => panic!("no stream"),
    }
    assert_eq!(*core.joined.lock().unwrap(), 1);
}
