//! Computer control's server over real sockets: the token, the guards on the head, and what a
//! client without the token, a slow client, or too many of them, can't do to the others.

use super::super::control::Control;
use super::super::fake::{FakeClock, FakeDesktop, FakeDialogs};
use super::*;
use std::io::{self, BufRead, BufReader, ErrorKind, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

#[test]
fn tokens_are_long_random_and_fresh() {
    let (a, b) = (new_token().unwrap(), new_token().unwrap());
    assert_eq!(a.len(), 64);
    assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
    assert_ne!(a, b);
}

#[test]
fn the_endpoint_never_prints_its_token() {
    let endpoint = Endpoint { url: "http://127.0.0.1:1/mcp".into(), token: "secret".into() };
    assert!(!format!("{endpoint:?}").contains("secret"));
}

fn exchange(url: &str, request: &str) -> String {
    let address = url.trim_start_matches("http://").trim_end_matches("/mcp");
    let mut stream = TcpStream::connect(address).unwrap();
    stream.write_all(request.as_bytes()).unwrap();
    let mut reply = String::new();
    stream.read_to_string(&mut reply).unwrap();
    reply
}

/// A helper on fakes, opened by `token`.
fn helper(token: &str) -> Arc<Helper> {
    Arc::new(Helper::new(
        Arc::new(FakeDesktop::default()),
        Arc::new(FakeDialogs::new(false, false)),
        Arc::new(FakeClock::new(0)),
        Arc::new(Control::new(|_| {})),
        token.to_owned(),
        1,
    ))
}

#[test]
fn the_loopback_server_checks_the_token_and_answers_json() {
    let token = new_token().unwrap();
    let serving = start(helper(&token), token.clone()).unwrap();
    let endpoint = serving.endpoint();
    assert!(endpoint.url.starts_with("http://127.0.0.1:") && endpoint.url.ends_with("/mcp"));
    let host = endpoint.url.trim_start_matches("http://").trim_end_matches("/mcp");

    let body = r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#;
    let request = |auth: &str| {
        format!(
            "POST /mcp HTTP/1.1\r\nHost: {host}\r\n{auth}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
    };
    let denied = exchange(&endpoint.url, &request(""));
    assert!(denied.starts_with("HTTP/1.1 401"), "{denied}");
    assert!(denied.contains("WWW-Authenticate: Bearer\r\n"), "{denied}");
    let allowed = exchange(&endpoint.url, &request(&format!("Authorization: Bearer {token}\r\n")));
    assert!(allowed.starts_with("HTTP/1.1 200"), "{allowed}");
    assert!(allowed.to_ascii_lowercase().contains("content-type: application/json"), "{allowed}");
    assert!(allowed.contains("\"screenshot\""), "{allowed}");
}

/// Limits short enough for a test.
const QUICK: Limits = Limits { first: Duration::from_millis(500), head: Duration::from_millis(500), body: Duration::from_secs(2), write: Duration::from_millis(300) };

/// The server on a real port, where it is and its token. Stopped when dropped.
struct Served {
    serving: Option<Serving>,
    address: String,
    token: String,
}

fn served(limits: Limits) -> Served {
    let token = new_token().unwrap();
    let serving = start_with(helper(&token), token.clone(), limits).unwrap();
    let address = serving.endpoint().url.trim_start_matches("http://").trim_end_matches("/mcp").to_owned();
    Served { serving: Some(serving), address, token }
}

impl Served {
    /// `tools/list` with the token, closing after: answered 200.
    fn valid(&self) -> String {
        self.post(&format!("Authorization: Bearer {}\r\n", self.token), r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#)
    }

    /// A POST to `/mcp` with these extra header lines and this body.
    fn post(&self, extra: &str, body: &str) -> String {
        format!("POST /mcp HTTP/1.1\r\nHost: {}\r\n{extra}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", self.address, body.len())
    }

    /// A POST without the token, saying a body of `length` bytes comes (it never does).
    fn tokenless(&self, length: &str) -> String {
        format!("POST /mcp HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\nContent-Length: {length}\r\n\r\n", self.address)
    }

    /// A new connection that sent `request`.
    fn send(&self, request: &str) -> TcpStream {
        let mut stream = connect(&self.address);
        stream.write_all(request.as_bytes()).unwrap();
        stream
    }

    /// What `request` gets on a connection of its own, the server closing after it.
    fn answer(&self, request: &str) -> String {
        until_closed(&mut self.send(request), Duration::from_secs(2)).expect("answered and closed")
    }
}

fn connect(address: &str) -> TcpStream {
    let stream = TcpStream::connect(address).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    stream
}

/// Everything the server sends until it closes, if it closes within `wait`.
fn until_closed(stream: &mut TcpStream, wait: Duration) -> io::Result<String> {
    stream.set_read_timeout(Some(wait))?;
    let mut got = Vec::new();
    stream.read_to_end(&mut got)?;
    Ok(String::from_utf8_lossy(&got).into_owned())
}

/// The answer's status line, if it comes within `wait`.
fn first_line(stream: &mut TcpStream, wait: Duration) -> io::Result<String> {
    stream.set_read_timeout(Some(wait))?;
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line)?;
    Ok(line)
}

fn status(answer: &str) -> &str {
    answer.lines().next().unwrap_or_default()
}

/// The server closed the connection: an end, or a reset.
fn closed(got: &io::Result<String>) -> bool {
    match got {
        Ok(_) => true,
        Err(e) => e.kind() == ErrorKind::ConnectionReset,
    }
}

/// `f` holds within `wait`, tried every 50 ms.
fn eventually(wait: Duration, mut f: impl FnMut() -> bool) -> bool {
    let started = Instant::now();
    loop {
        if f() {
            return true;
        }
        if started.elapsed() > wait {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Runs the ignored test `name` of this module alone in a child process (what it does may
/// abort a process), and says whether it passed, with its output.
fn in_a_child(name: &str) -> (bool, String) {
    let module = module_path!().split_once("::").map_or(module_path!(), |(_, rest)| rest);
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([&format!("{module}::{name}"), "--exact", "--ignored", "--nocapture", "--test-threads=1"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let started = Instant::now();
    let finished = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break Some(status);
        }
        if started.elapsed() > Duration::from_secs(20) {
            let _ = child.kill();
            break None;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let output = child.wait_with_output().unwrap();
    let said = format!("{finished:?}\n{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    (finished.is_some_and(|s| s.success()) && said.contains("1 passed"), said)
}

#[test]
fn without_the_token_a_huge_content_length_is_refused_at_once_and_the_app_lives_on() {
    let (passed, said) = in_a_child("a_huge_content_length_without_the_token_in_a_child");
    assert!(passed, "the child died or failed: {said}");
}

#[test]
#[ignore = "run in a child process by without_the_token_a_huge_content_length_is_refused_at_once_and_the_app_lives_on"]
fn a_huge_content_length_without_the_token_in_a_child() {
    let s = served(Limits::DEFAULT);
    let started = Instant::now();
    let answer = until_closed(&mut s.send(&s.tokenless("4611686018427387904")), Duration::from_secs(2)).expect("answered and closed");
    assert!(status(&answer).starts_with("HTTP/1.1 401"), "{answer}");
    assert!(started.elapsed() < Duration::from_secs(1), "{:?}", started.elapsed());
    // What tiny_http did with it happened after its answer: give it the time to.
    std::thread::sleep(Duration::from_millis(300));
    let served = s.answer(&s.valid());
    assert!(status(&served).starts_with("HTTP/1.1 200"), "{served}");
}

#[test]
fn without_the_token_a_content_length_of_u64_max_breaks_nothing() {
    let s = served(Limits::DEFAULT);
    let answer = until_closed(&mut s.send(&s.tokenless(&u64::MAX.to_string())), Duration::from_secs(2)).expect("answered and closed");
    assert!(status(&answer).starts_with("HTTP/1.1 401"), "{answer}");
    std::thread::sleep(Duration::from_millis(300));
    let mut next = s.send(&s.valid());
    let served = until_closed(&mut next, Duration::from_secs(2)).expect("the next request is served");
    assert!(status(&served).starts_with("HTTP/1.1 200"), "{served}");
}

#[test]
fn without_the_token_a_body_that_never_comes_holds_up_no_one() {
    let s = served(Limits::DEFAULT);
    let mut stalled = s.send(&s.tokenless("100000"));
    std::thread::sleep(Duration::from_millis(100));
    let started = Instant::now();
    let mut other = s.send(&s.valid());
    let answer = first_line(&mut other, Duration::from_secs(2)).expect("served meanwhile");
    assert!(answer.starts_with("HTTP/1.1 200") && started.elapsed() < Duration::from_secs(2), "{answer:?} in {:?}", started.elapsed());
    let refused = until_closed(&mut stalled, Duration::from_secs(2)).expect("refused and closed, its body unread");
    assert!(status(&refused).starts_with("HTTP/1.1 401"), "{refused}");
}

#[test]
fn a_body_over_1_mib_gets_413_without_being_read() {
    let s = served(Limits::DEFAULT);
    let head = format!("POST /mcp HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {}\r\nContent-Length: {}\r\n\r\n", s.address, s.token, (1 << 20) + 1);
    let started = Instant::now();
    let answer = until_closed(&mut s.send(&head), Duration::from_secs(2)).expect("refused before its body came");
    assert!(status(&answer).starts_with("HTTP/1.1 413"), "{answer}");
    assert!(started.elapsed() < Duration::from_secs(1), "{:?}", started.elapsed());
}

#[test]
fn a_head_sent_a_byte_at_a_time_is_cut_off_when_the_heads_time_is_up() {
    let s = served(QUICK);
    let mut slow = connect(&s.address);
    let mut writer = slow.try_clone().unwrap();
    let head = format!("POST /mcp HTTP/1.1\r\nHost: {}\r\nX-Slow: {}", s.address, "a".repeat(100));
    let started = Instant::now();
    let dripping = std::thread::spawn(move || {
        for byte in head.bytes() {
            if writer.write_all(&[byte]).is_err() {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    });
    let cut = until_closed(&mut slow, Duration::from_secs(3));
    let after = started.elapsed();
    assert!(closed(&cut), "still open after {after:?}: {cut:?}");
    assert!(after >= QUICK.head && after < QUICK.head * 4, "{after:?}");
    dripping.join().unwrap();
}

#[test]
fn connections_past_the_cap_are_closed_at_once_and_take_no_thread() {
    let s = served(Limits::DEFAULT);
    let held: Vec<TcpStream> = (0..MAX_CONNECTIONS).map(|_| connect(&s.address)).collect();
    let open = s.serving.as_ref().map(Serving::connections).expect("on");
    assert!(eventually(Duration::from_secs(1), || open() == MAX_CONNECTIONS), "{} open", open());
    let started = Instant::now();
    for _ in 0..3 * MAX_CONNECTIONS {
        let got = until_closed(&mut connect(&s.address), Duration::from_secs(2));
        assert!(closed(&got) && got.as_ref().map_or(true, String::is_empty), "{got:?}");
    }
    assert!(started.elapsed() < Duration::from_secs(1), "{:?}", started.elapsed());
    assert_eq!(open(), MAX_CONNECTIONS, "no more connections, so no more threads");
    drop(held);
    let free = eventually(Duration::from_secs(2), || first_line(&mut s.send(&s.valid()), Duration::from_millis(500)).is_ok_and(|l| l.starts_with("HTTP/1.1 200")));
    assert!(free, "their places come free once they're gone");
}

#[test]
fn only_a_loopback_host_with_this_port_is_served() {
    let s = served(Limits::DEFAULT);
    let port = s.address.rsplit_once(':').unwrap().1.to_owned();
    for host in ["evil.example", "evil.example:80", &format!("evil.example:{port}"), "127.0.0.1", "127.0.0.1:1", "localhost"] {
        let request = s.valid().replace(&format!("Host: {}", s.address), &format!("Host: {host}"));
        let answer = s.answer(&request);
        assert!(status(&answer).starts_with("HTTP/1.1 403"), "{host}: {answer}");
    }
    let no_host = s.valid().replace(&format!("Host: {}\r\n", s.address), "");
    assert!(status(&s.answer(&no_host)).starts_with("HTTP/1.1 403"));
    let localhost = s.valid().replace(&format!("Host: {}", s.address), &format!("Host: localhost:{port}"));
    assert!(status(&s.answer(&localhost)).starts_with("HTTP/1.1 200"));
}

#[test]
fn a_request_from_a_web_page_is_refused_even_with_the_token() {
    let s = served(Limits::DEFAULT);
    for origin in ["https://evil.example", "null", &format!("http://{}", s.address)] {
        let request = s.post(&format!("Origin: {origin}\r\nAuthorization: Bearer {}\r\n", s.token), r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#);
        let answer = s.answer(&request);
        assert!(status(&answer).starts_with("HTTP/1.1 403"), "{origin}: {answer}");
    }
}

#[test]
fn a_chunked_body_gets_501_unread() {
    let s = served(Limits::DEFAULT);
    let request = format!("POST /mcp HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {}\r\nTransfer-Encoding: chunked\r\n\r\n", s.address, s.token);
    let answer = s.answer(&request);
    assert!(status(&answer).starts_with("HTTP/1.1 501"), "{answer}");
}

#[test]
fn a_keep_alive_request_is_answered_then_closed_and_the_next_comes_on_a_new_connection() {
    let s = served(Limits::DEFAULT);
    let keep = s.valid().replace("Connection: close", "Connection: keep-alive");
    let first = until_closed(&mut s.send(&keep), Duration::from_secs(2)).expect("answered, then closed");
    assert!(status(&first).starts_with("HTTP/1.1 200") && first.contains("Connection: close\r\n"), "{first}");
    let again = s.answer(&keep);
    assert!(status(&again).starts_with("HTTP/1.1 200"), "{again}");
}

#[test]
fn the_path_and_method_are_matched_as_before() {
    let s = served(Limits::DEFAULT);
    let with = |method: &str, path: &str| format!("{method} {path} HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {}\r\nConnection: close\r\n\r\n", s.address, s.token);
    assert!(status(&s.answer(&with("GET", "/mcp"))).starts_with("HTTP/1.1 405"), "no event stream: a client's GET learns so");
    assert!(status(&s.answer(&with("DELETE", "/mcp"))).starts_with("HTTP/1.1 405"));
    assert!(status(&s.answer(&with("GET", "/other"))).starts_with("HTTP/1.1 404"));
    let query = s.valid().replace("POST /mcp ", "POST /mcp?x=1 ");
    assert!(status(&s.answer(&query)).starts_with("HTTP/1.1 200"), "the query is left out");
    let notification = s.post(&format!("Authorization: Bearer {}\r\n", s.token), r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#);
    assert!(status(&s.answer(&notification)).starts_with("HTTP/1.1 202"));
}

#[test]
fn stopped_its_port_refuses_connections_and_its_threads_end_even_with_clients_stalled() {
    let mut s = served(Limits::DEFAULT);
    let open = s.serving.as_ref().map(Serving::connections).expect("on");
    let _idle = connect(&s.address);
    let mut stalled = s.send(&s.tokenless("100000"));
    let refused = first_line(&mut stalled, Duration::from_secs(2)).expect("refused");
    assert!(refused.starts_with("HTTP/1.1 401"), "{refused:?}");
    s.serving.take().expect("on").stop();
    let after = TcpStream::connect(&s.address);
    assert!(after.as_ref().is_err_and(|e| e.kind() == ErrorKind::ConnectionRefused), "{after:?}");
    assert!(eventually(Duration::from_secs(3), || open() == 0), "{} connections still open", open());
}
