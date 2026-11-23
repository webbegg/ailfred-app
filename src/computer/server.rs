//! Serves `Helper` on `127.0.0.1:<random port>/mcp` with a fresh random token per launch. The
//! token lives only in memory and in what Ailfred hands the model it starts.

use super::mcp::{Helper, HttpReply, HttpRequest};
use std::io::Read;
use std::sync::Arc;
use std::thread;

/// Where the tools are served, and the token that opens them.
#[derive(Clone, PartialEq, Eq)]
pub struct Endpoint {
    pub url: String,
    pub token: String,
}

impl std::fmt::Debug for Endpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Endpoint").field("url", &self.url).field("token", &"…").finish()
    }
}

/// 32 random bytes from the system, hex encoded.
pub fn new_token() -> std::io::Result<String> {
    let mut bytes = [0u8; 32];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// Binds a random loopback port and serves each request on its own thread (actions are
/// serialized inside `Helper`). Serves until the app quits.
pub fn start(helper: Arc<Helper>, token: String) -> std::io::Result<Endpoint> {
    let server = tiny_http::Server::http("127.0.0.1:0").map_err(std::io::Error::other)?;
    let port = server.server_addr().to_ip().map(|a| a.port()).ok_or_else(|| std::io::Error::other("no port"))?;
    thread::Builder::new().name("computer-mcp".into()).spawn(move || {
        for request in server.incoming_requests() {
            let helper = helper.clone();
            let _ = thread::Builder::new().name("computer-call".into()).spawn(move || serve(&helper, request));
        }
    })?;
    Ok(Endpoint { url: format!("http://127.0.0.1:{port}/mcp"), token })
}

/// Requests larger than this are refused: tool calls are small.
const MAX_BODY: u64 = 1 << 20;

fn serve(helper: &Helper, mut request: tiny_http::Request) {
    let authorization = request.headers().iter().find(|h| h.field.equiv("Authorization")).map(|h| h.value.as_str().to_string());
    let mut body = String::new();
    let read = request.as_reader().take(MAX_BODY).read_to_string(&mut body);
    let method = request.method().as_str().to_string();
    let path = request.url().split('?').next().unwrap_or_default().to_string();
    let reply = match read {
        Ok(_) => helper.handle(&HttpRequest { method: &method, path: &path, authorization: authorization.as_deref(), body: &body }),
        Err(_) => HttpReply { status: 400, body: None },
    };
    let is_json = reply.body.is_some();
    let text = reply.body.map(|b| b.to_string()).unwrap_or_default();
    let mut response = tiny_http::Response::from_string(text).with_status_code(reply.status);
    if is_json {
        response.add_header(tiny_http::Header::from_bytes("Content-Type", "application/json").expect("valid header"));
    }
    if reply.status == 401 {
        response.add_header(tiny_http::Header::from_bytes("WWW-Authenticate", "Bearer").expect("valid header"));
    }
    let _ = request.respond(response);
}

#[cfg(test)]
mod tests {
    use super::super::control::Control;
    use super::super::fake::{FakeClock, FakeDesktop, FakeDialogs};
    use super::*;
    use std::io::Write;
    use std::net::TcpStream;

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

    #[test]
    fn the_loopback_server_checks_the_token_and_answers_json() {
        let token = new_token().unwrap();
        let helper = Helper::new(
            Arc::new(FakeDesktop::default()),
            Arc::new(FakeDialogs::new(false, false)),
            Arc::new(FakeClock::new(0)),
            Arc::new(Control::new(|_| {})),
            token.clone(),
            1,
        );
        let endpoint = start(Arc::new(helper), token.clone()).unwrap();
        assert!(endpoint.url.starts_with("http://127.0.0.1:") && endpoint.url.ends_with("/mcp"));

        let body = r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#;
        let request = |auth: &str| {
            format!(
                "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\n{auth}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
        };
        let denied = exchange(&endpoint.url, &request(""));
        assert!(denied.starts_with("HTTP/1.1 401"), "{denied}");
        let allowed = exchange(&endpoint.url, &request(&format!("Authorization: Bearer {token}\r\n")));
        assert!(allowed.starts_with("HTTP/1.1 200"), "{allowed}");
        assert!(allowed.to_ascii_lowercase().contains("content-type: application/json"), "{allowed}");
        assert!(allowed.contains("\"screenshot\""), "{allowed}");
    }
}
