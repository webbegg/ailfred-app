//! A request's head, read off the socket's bytes: its request line and its headers, nothing
//! more. Strict: lines end in CRLF (a bare LF or CR is 400), no line folding, no
//! `Transfer-Encoding` (501), one numeric `Content-Length` (400), which a POST must have (411),
//! at most one `Host` and one `Authorization` (400: which one counts would be a guess),
//! at most `MAX_HEAD` bytes and `MAX_HEADERS` headers (431).

use super::routes::{Answer, Head};

/// The longest head, in bytes, its blank line too.
pub const MAX_HEAD: usize = 16 << 10;
/// The most headers a head has.
pub const MAX_HEADERS: usize = 64;

/// A request's head, as read.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Request {
    pub method: String,
    /// Without the query.
    pub path: String,
    pub host: Option<String>,
    pub origin: Option<String>,
    pub authorization: Option<String>,
    pub length: Option<u64>,
    /// It said `Expect: 100-continue`: it waits to hear it may send its body.
    pub continues: bool,
}

impl Request {
    /// What the guards read of it.
    pub fn head(&self) -> Head<'_> {
        Head {
            method: &self.method,
            path: &self.path,
            host: self.host.as_deref(),
            origin: self.origin.as_deref(),
            authorization: self.authorization.as_deref(),
            length: self.length,
        }
    }
}

fn bad(reason: &str) -> Answer {
    Answer::error(400, reason)
}

fn too_large() -> Answer {
    Answer::error(431, "the request's head is larger than 16 KiB or has more than 64 headers")
}

/// Where the head in `bytes` (read so far) ends, past its blank line; None if it hasn't yet.
/// A bare LF or CR is 400, and a head over `MAX_HEAD` 431. The first `seen` bytes were looked
/// at already (but their last three, which may start the blank line).
pub fn end(bytes: &[u8], seen: usize) -> Result<Option<usize>, Answer> {
    for (i, &b) in bytes.iter().enumerate().skip(seen.saturating_sub(3)) {
        let bare_lf = b == b'\n' && (i == 0 || bytes[i - 1] != b'\r');
        let bare_cr = b == b'\r' && bytes.get(i + 1).is_some_and(|&next| next != b'\n');
        if bare_lf || bare_cr {
            return Err(bad("lines must end in CRLF"));
        }
        if b == b'\n' && i >= 3 && &bytes[i - 3..=i] == b"\r\n\r\n" {
            return if i < MAX_HEAD { Ok(Some(i + 1)) } else { Err(too_large()) };
        }
    }
    if bytes.len() >= MAX_HEAD { Err(too_large()) } else { Ok(None) }
}

/// The head `bytes` (up to and with its blank line): its request line and the headers the
/// server reads.
pub fn parse(bytes: &[u8]) -> Result<Request, Answer> {
    let text = std::str::from_utf8(bytes).map_err(|_| bad("the head isn't UTF-8"))?;
    let text = text.strip_suffix("\r\n\r\n").ok_or_else(|| bad("the head doesn't end in a blank line"))?;
    let mut lines = text.split("\r\n");
    let mut request = request_line(lines.next().unwrap_or_default())?;
    let (mut hosts, mut lengths, mut authorizations) = (0, 0, 0);
    for (n, line) in lines.enumerate() {
        if n >= MAX_HEADERS {
            return Err(too_large());
        }
        if line.starts_with([' ', '\t']) {
            return Err(bad("folded header lines aren't accepted"));
        }
        let (name, value) = line.split_once(':').ok_or_else(|| bad("a header line has no colon"))?;
        if name.is_empty() || !name.bytes().all(token) {
            return Err(bad("a header's name isn't a token"));
        }
        let value = value.trim_matches([' ', '\t']);
        if value.bytes().any(|b| (b < 0x20 && b != b'\t') || b == 0x7f) {
            return Err(bad("a header's value has a control character"));
        }
        let first = |slot: &mut Option<String>| {
            if slot.is_none() {
                *slot = Some(value.to_owned());
            }
        };
        match name.to_ascii_lowercase().as_str() {
            "host" => {
                hosts += 1;
                first(&mut request.host);
            }
            "origin" => first(&mut request.origin),
            "authorization" => {
                authorizations += 1;
                first(&mut request.authorization);
            }
            "transfer-encoding" => return Err(Answer::error(501, "Transfer-Encoding isn't supported: send a Content-Length")),
            "content-length" => {
                lengths += 1;
                request.length = Some(content_length(value)?);
            }
            "expect" => request.continues = value.eq_ignore_ascii_case("100-continue"),
            _ => {}
        }
    }
    if hosts > 1 {
        return Err(bad("more than one Host header"));
    }
    if authorizations > 1 {
        return Err(bad("more than one Authorization header"));
    }
    if lengths > 1 {
        return Err(bad("more than one Content-Length header"));
    }
    if request.method == "POST" && request.length.is_none() {
        return Err(Answer::error(411, "a POST needs a Content-Length"));
    }
    Ok(request)
}

/// `METHOD /path HTTP/1.x`.
fn request_line(line: &str) -> Result<Request, Answer> {
    let mut parts = line.split(' ');
    let (Some(method), Some(target), Some(version), None) = (parts.next(), parts.next(), parts.next(), parts.next()) else {
        return Err(bad("not a request line"));
    };
    if method.is_empty() || !method.bytes().all(token) || !target.starts_with('/') {
        return Err(bad("not a request line"));
    }
    match version {
        "HTTP/1.1" | "HTTP/1.0" => {}
        v if v.starts_with("HTTP/") => return Err(Answer::error(505, "only HTTP/1.1 is spoken")),
        _ => return Err(bad("not a request line")),
    }
    let path = target.split('?').next().unwrap_or_default();
    Ok(Request { method: method.into(), path: path.into(), ..Request::default() })
}

/// Digits only; past what a u64 holds, the largest (the size guard refuses it).
fn content_length(value: &str) -> Result<u64, Answer> {
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return Err(bad("Content-Length isn't a number"));
    }
    Ok(value.parse().unwrap_or(u64::MAX))
}

/// A byte of an HTTP token (a method, a header's name).
fn token(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b)
}
