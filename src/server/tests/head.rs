//! A request's head, read strictly off its bytes.

use super::super::head::{MAX_HEAD, MAX_HEADERS, Request, end, parse};

/// What `head` (its blank line added) parses to, or the status it's refused with.
fn parsed(head: &str) -> Result<Request, u16> {
    parse(format!("{head}\r\n\r\n").as_bytes()).map_err(|a| a.status)
}

fn status(head: &str) -> u16 {
    parsed(head).expect_err("taken")
}

#[test]
fn the_head_ends_at_its_blank_line_and_what_follows_is_the_body() {
    let bytes = b"GET / HTTP/1.1\r\nHost: a\r\n\r\nbody";
    assert_eq!(end(&bytes[..10], 0).unwrap(), None, "not yet");
    assert_eq!(end(bytes, 10).unwrap(), Some(bytes.len() - 4), "found past what was looked at");
}

#[test]
fn a_bare_lf_or_cr_is_400_at_once() {
    for bytes in [&b"GET / HTTP/1.1\nHost: a"[..], b"GET / HTTP/1.1\r\nHost: a\n\n", b"GET / HTTP/1.1\rHost: a", b"\n"] {
        assert_eq!(end(bytes, 0).unwrap_err().status, 400, "{:?}", String::from_utf8_lossy(bytes));
    }
}

#[test]
fn a_head_of_16_kib_without_its_end_or_with_too_many_headers_is_431() {
    assert_eq!(end(&vec![b'a'; MAX_HEAD], 0).unwrap_err().status, 431);
    let mut long = format!("GET / HTTP/1.1\r\nX: {}", "a".repeat(MAX_HEAD)).into_bytes();
    long.truncate(MAX_HEAD - 4);
    long.extend(b"\r\n\r\n");
    assert_eq!(end(&long, 0).unwrap(), Some(MAX_HEAD), "exactly 16 KiB fits");
    let many: String = (0..=MAX_HEADERS).map(|n| format!("\r\nX-{n}: a")).collect();
    assert_eq!(status(&format!("GET / HTTP/1.1{many}")), 431);
    let enough: String = (1..MAX_HEADERS).map(|n| format!("\r\nX-{n}: a")).collect();
    assert!(parsed(&format!("GET / HTTP/1.1\r\nHost: a{enough}")).is_ok());
}

#[test]
fn header_names_are_read_whatever_their_case_and_values_trimmed() {
    let r = parsed("POST /v1/commands?x=1 HTTP/1.1\r\nhOsT: 127.0.0.1:1\r\nAUTHORIZATION:  Bearer t \r\norigin: null\r\ncontent-LENGTH: 12\r\nExpect: 100-Continue").unwrap();
    assert_eq!((r.method.as_str(), r.path.as_str()), ("POST", "/v1/commands"), "without the query");
    assert_eq!((r.host.as_deref(), r.authorization.as_deref(), r.origin.as_deref()), (Some("127.0.0.1:1"), Some("Bearer t"), Some("null")));
    assert_eq!((r.length, r.continues), (Some(12), true));
}

#[test]
fn folded_lines_and_malformed_lines_are_400() {
    for head in [
        "GET / HTTP/1.1\r\nHost: a\r\n b",
        "GET / HTTP/1.1\r\nHost: a\r\n\tb",
        "GET / HTTP/1.1\r\nHost a",
        "GET / HTTP/1.1\r\nHost : a",
        "GET / HTTP/1.1\r\n: a",
        "GET / HTTP/1.1\r\nHost: a\x01",
        "GET /  HTTP/1.1",
        "GET / HTTP/1.1 x",
        "GET example.com HTTP/1.1",
        "GET /",
        "",
        "GET / HTTP/1.1\r\nHost: a\r\nHost: b",
    ] {
        assert_eq!(status(head), 400, "{head:?}");
    }
    assert_eq!(status("GET / HTTP/2.0"), 505);
}

#[test]
fn transfer_encoding_is_501_and_content_length_must_be_one_number() {
    for te in ["chunked", "identity", "gzip, chunked"] {
        assert_eq!(status(&format!("POST /v1/commands HTTP/1.1\r\nTransfer-Encoding: {te}\r\nContent-Length: 1")), 501, "{te}");
    }
    for cl in ["", "12a", "-1", "+1", "1, 1", "0x10", "1 2"] {
        assert_eq!(status(&format!("POST /v1/commands HTTP/1.1\r\nContent-Length: {cl}")), 400, "{cl:?}");
    }
    assert_eq!(status("POST /v1/commands HTTP/1.1\r\nContent-Length: 1\r\nContent-Length: 1"), 400, "said twice, even the same");
    assert_eq!(status("POST /v1/commands HTTP/1.1\r\nHost: a"), 411);
    let huge = parsed("POST /v1/commands HTTP/1.1\r\nContent-Length: 99999999999999999999999").unwrap();
    assert_eq!(huge.length, Some(u64::MAX), "a number past u64 is too large, not malformed");
    assert_eq!(parsed("GET /v1/events HTTP/1.1").unwrap().length, None, "a GET needs none");
}
