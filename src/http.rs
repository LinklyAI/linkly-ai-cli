//! The one place HTTP clients are built.
//!
//! Every request the CLI sends — MCP to Desktop (local, LAN, or the remote
//! tunnel), the website, the update and skills servers — carries the same
//! `User-Agent: linkly-cli/<version>`, the usual way an HTTP client names
//! itself and its version. The application driving the CLI is a separate
//! header, `X-Linkly-Client` (see `connection::CLIENT_HEADER`).
//!
//! Build clients through [`client_builder`] rather than `reqwest::Client`
//! directly; a bare `reqwest::get` or `Client::new()` sends no User-Agent.

/// Fixed User-Agent for all outgoing requests.
pub const USER_AGENT: &str = concat!("linkly-cli/", env!("CARGO_PKG_VERSION"));

/// A client builder with [`USER_AGENT`] already set; add timeouts as needed.
pub fn client_builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder().user_agent(USER_AGENT)
}

#[cfg(test)]
mod tests {
    use super::{client_builder, USER_AGENT};

    #[test]
    fn the_user_agent_names_the_cli_and_its_version() {
        assert_eq!(
            USER_AGENT,
            format!("linkly-cli/{}", env!("CARGO_PKG_VERSION"))
        );
    }

    #[tokio::test]
    async fn requests_from_a_built_client_carry_the_user_agent() {
        let listener =
            std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind an ephemeral loopback port");
        let addr = listener.local_addr().expect("read the bound address");
        let server = std::thread::spawn(move || {
            use std::io::{Read, Write};
            let (mut stream, _) = listener.accept().expect("accept the client's request");
            let mut request = [0u8; 4096];
            let n = stream.read(&mut request).expect("read the request");
            stream
                .write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n")
                .expect("write the canned response");
            String::from_utf8_lossy(&request[..n]).to_ascii_lowercase()
        });

        client_builder()
            .no_proxy()
            .build()
            .expect("build the client")
            .get(format!("http://{addr}/health"))
            .send()
            .await
            .expect("the canned server answers");

        let request = server.join().expect("the server thread finishes");
        let expected = format!("user-agent: {}\r\n", USER_AGENT.to_ascii_lowercase());
        assert!(request.contains(&expected), "request was: {request}");
    }
}
