//! One WebSocket connection to a hook-relay server and the requests it relays.
//!
//! The behavior follows `src/client/websocket.ts` in
//! <https://github.com/itkq/hook-relay>.

use std::time::Duration;

use anyhow::{Context, bail};
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use futures_util::{SinkExt, StreamExt};
use reqwest::header::{HeaderMap, HeaderName, SET_COOKIE};
use tokio::sync::{mpsc, watch};
use tokio_tungstenite::tungstenite::{
    Message,
    protocol::{CloseFrame, frame::coding::CloseCode},
};
use tracing::{debug, error, info, warn};
use url::Url;

use crate::protocol::{
    ClientMessage, HeaderValue, Headers, HttpRequest, ServerMessage, challenge_hmac,
};

const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(20);
const CLOSE_TIMEOUT: Duration = Duration::from_secs(1);

/// Headers that apply to one connection and are not forwarded either way.
const HOP_BY_HOP_HEADERS: &[&str] = &[
    "connection",
    "keep-alive",
    "proxy-connection",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
];

pub struct Relay {
    pub client_id: String,
    pub server_endpoint: Url,
    pub forward_endpoint: Url,
    pub challenge_passphrase: String,
    pub path: Option<String>,
    pub filter_body_regex: Option<String>,
    pub http: reqwest::Client,
}

/// How a connection ended when it did not end with a fatal error.
#[derive(Debug, PartialEq)]
pub enum Outcome {
    Reconnect,
    Shutdown,
}

impl Relay {
    fn connect_url(&self) -> Url {
        let mut url = self.server_endpoint.clone();
        {
            let mut query = url.query_pairs_mut();
            query.append_pair("clientId", &self.client_id);
            if let Some(path) = &self.path {
                query.append_pair("path", path);
            }
            if let Some(regex) = &self.filter_body_regex {
                query.append_pair("filterBodyRegex", regex);
            }
        }
        url
    }

    /// Runs one connection until the server closes it, it fails, or
    /// `shutdown` turns true. Returns an error when the server rejects the
    /// client in a way that reconnecting cannot fix.
    pub async fn connect(&self, shutdown: &mut watch::Receiver<bool>) -> anyhow::Result<Outcome> {
        let (mut ws, _) = match tokio_tungstenite::connect_async(self.connect_url().as_str()).await
        {
            Ok(conn) => conn,
            Err(e) => {
                error!("Could not connect to {}: {e}", self.server_endpoint);
                return Ok(Outcome::Reconnect);
            }
        };
        match &self.path {
            Some(path) => info!(
                "Connected to server {} as {} ({path})",
                self.server_endpoint, self.client_id
            ),
            None => info!(
                "Connected to server {} as {}",
                self.server_endpoint, self.client_id
            ),
        }

        let (tx, mut rx) = mpsc::unbounded_channel::<ClientMessage>();
        let mut heartbeat = tokio::time::interval_at(
            tokio::time::Instant::now() + HEARTBEAT_INTERVAL,
            HEARTBEAT_INTERVAL,
        );

        loop {
            tokio::select! {
                () = stopped(shutdown) => {
                    info!("Closing WebSocket connection gracefully...");
                    let frame = CloseFrame { code: CloseCode::Normal, reason: "Client shutdown".into() };
                    let closed = tokio::time::timeout(CLOSE_TIMEOUT, async {
                        let _ = ws.close(Some(frame)).await;
                        while let Some(Ok(_)) = ws.next().await {}
                    })
                    .await;
                    match closed {
                        Ok(()) => info!("WebSocket closed gracefully"),
                        Err(_) => warn!("Graceful shutdown timeout reached, terminating WebSocket"),
                    }
                    return Ok(Outcome::Shutdown);
                }
                Some(msg) = rx.recv() => {
                    let text = serde_json::to_string(&msg).expect("client messages serialize");
                    if let Err(e) = ws.send(Message::text(text)).await {
                        error!("WebSocket error: {e}");
                        return Ok(Outcome::Reconnect);
                    }
                }
                _ = heartbeat.tick() => {
                    debug!("Sending heartbeat to server");
                    if let Err(e) = ws.send(Message::Ping(Default::default())).await {
                        error!("WebSocket error: {e}");
                        return Ok(Outcome::Reconnect);
                    }
                }
                frame = ws.next() => match frame {
                    Some(Ok(Message::Text(text))) => self.handle(text.as_str(), &tx),
                    Some(Ok(Message::Ping(_))) => debug!("Received ping from server"),
                    Some(Ok(Message::Close(frame))) => {
                        // Send the close reply that tungstenite queued.
                        let _ = ws.flush().await;
                        return closed_by_server(frame.map(|f| u16::from(f.code)));
                    }
                    Some(Ok(_)) => {}
                    Some(Err(e)) => {
                        error!("WebSocket error: {e}");
                        return Ok(Outcome::Reconnect);
                    }
                    None => return closed_by_server(None),
                },
            }
        }
    }

    fn handle(&self, text: &str, tx: &mpsc::UnboundedSender<ClientMessage>) {
        debug!("Received message: {text}");
        let msg = match serde_json::from_str::<ServerMessage>(text) {
            Ok(msg) => msg,
            Err(e) => {
                warn!("Ignored a message this client does not understand: {e}");
                return;
            }
        };
        match msg {
            ServerMessage::Challenge { nonce } => {
                let _ = tx.send(ClientMessage::ChallengeResponse {
                    client_id: self.client_id.clone(),
                    hmac: challenge_hmac(&self.challenge_passphrase, &nonce),
                });
                debug!("Sent challenge response");
            }
            ServerMessage::ChallengeResult { success: true } => info!("Authentication successful"),
            ServerMessage::ChallengeResult { success: false } => {
                error!("Authentication failed (challenge response mismatch)")
            }
            ServerMessage::Http(req) => {
                let http = self.http.clone();
                let forward_endpoint = self.forward_endpoint.clone();
                let client_id = self.client_id.clone();
                let tx = tx.clone();
                tokio::spawn(async move {
                    let (method, path, message_id) =
                        (req.method.clone(), req.path.clone(), req.message_id.clone());
                    match forward(&http, &forward_endpoint, req).await {
                        Ok((status, headers, body)) => {
                            let _ = tx.send(ClientMessage::Http {
                                client_id,
                                message_id: message_id.clone(),
                                headers,
                                status,
                                body,
                            });
                            info!(
                                "{method} {path} -> {status} sent to server (message:{message_id})"
                            );
                        }
                        // Stay silent towards the server, as the upstream
                        // client does: another client may still answer.
                        Err(e) => error!("Error forwarding request (message:{message_id}): {e:#}"),
                    }
                });
            }
            ServerMessage::Error { message_id, error } => {
                error!("Error: {error} for {message_id}")
            }
        }
    }
}

/// Resolves once `stop` is true. A dropped sender counts as a stop.
pub async fn stopped(stop: &mut watch::Receiver<bool>) {
    // Drop the returned guard here: holding it across an await is not Send.
    let _ = stop.wait_for(|stop| *stop).await;
}

/// Maps the close code the server sent to what the client does next.
fn closed_by_server(code: Option<u16>) -> anyhow::Result<Outcome> {
    let reason = match code {
        Some(4001) => "Authentication failed",
        Some(4002) => "Unknown authenticator",
        Some(4004) => "No client ID provided",
        Some(4005) => "Unsafe regex pattern",
        Some(1000) => {
            info!("Disconnected from the server");
            return Ok(Outcome::Reconnect);
        }
        None => {
            warn!("Connection closed");
            return Ok(Outcome::Reconnect);
        }
        Some(code) => {
            info!("Disconnected from the server with code: {code}");
            return Ok(Outcome::Reconnect);
        }
    };
    bail!(
        "{reason} (close code {}), not reconnecting",
        code.unwrap_or_default()
    )
}

/// Sends the relayed request to the forward endpoint and returns the status,
/// headers, and base64-encoded body of its response.
async fn forward(
    http: &reqwest::Client,
    forward_endpoint: &Url,
    req: HttpRequest,
) -> anyhow::Result<(u16, Headers, String)> {
    let url = forward_url(forward_endpoint, &req);
    let method = reqwest::Method::from_bytes(req.method.as_bytes())
        .with_context(|| format!("invalid method {:?}", req.method))?;
    let body = BASE64
        .decode(&req.raw_body)
        .context("rawBody is not base64")?;

    let mut headers = HeaderMap::new();
    for (name, value) in &req.headers {
        if name == "content-length" || HOP_BY_HOP_HEADERS.contains(&name.as_str()) {
            continue;
        }
        let Ok(name) = HeaderName::from_bytes(name.as_bytes()) else {
            continue;
        };
        for v in value.values() {
            if let Ok(v) = v.parse() {
                headers.append(name.clone(), v);
            }
        }
    }

    let resp = http
        .request(method, url)
        .headers(headers)
        .body(body)
        .send()
        .await?;
    debug!("response status: {}", resp.status());
    debug!("response headers: {:?}", resp.headers());
    let status = resp.status().as_u16();
    let headers = node_headers(resp.headers());
    let body = resp.bytes().await?;
    Ok((status, headers, BASE64.encode(body)))
}

/// The forward endpoint's origin with the relayed path and query.
///
/// The path replaces the endpoint's path, as `new URL(path, endpoint)` does in
/// the upstream client, but it is set as a path so that a relayed path such as
/// `//other-host/` cannot point the request at another host.
fn forward_url(forward_endpoint: &Url, req: &HttpRequest) -> Url {
    let mut url = forward_endpoint.clone();
    url.set_path(&req.path);
    url.set_query(None);
    url.set_fragment(None);
    if !req.query_params.is_empty() {
        url.query_pairs_mut().extend_pairs(&req.query_params);
    }
    url
}

/// Response headers in the shape Node.js gives them: `set-cookie` as a list,
/// other repeated headers joined with `, `.
fn node_headers(map: &HeaderMap) -> Headers {
    let mut out = Headers::new();
    for name in map.keys() {
        if HOP_BY_HOP_HEADERS.contains(&name.as_str()) {
            continue;
        }
        let values: Vec<String> = map
            .get_all(name)
            .iter()
            .map(|v| String::from_utf8_lossy(v.as_bytes()).into_owned())
            .collect();
        let value = if name == SET_COOKIE {
            HeaderValue::Many(values)
        } else {
            HeaderValue::One(values.join(", "))
        };
        out.insert(name.as_str().to_owned(), value);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn request(path: &str, query: &[(&str, &str)]) -> HttpRequest {
        HttpRequest {
            message_id: "1".into(),
            headers: Headers::new(),
            raw_body: String::new(),
            method: "GET".into(),
            path: path.into(),
            query_params: query
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect::<BTreeMap<_, _>>(),
        }
    }

    #[test]
    fn forward_url_replaces_path_and_query() {
        let base = Url::parse("http://localhost:9000/base?x=1").unwrap();
        assert_eq!(
            forward_url(&base, &request("/slack/events", &[])).as_str(),
            "http://localhost:9000/slack/events"
        );
        assert_eq!(
            forward_url(&base, &request("/auth/redirect", &[("code", "a b")])).as_str(),
            "http://localhost:9000/auth/redirect?code=a+b"
        );
    }

    #[test]
    fn forward_url_keeps_the_host() {
        let base = Url::parse("http://localhost:9000").unwrap();
        let url = forward_url(&base, &request("//evil.example/x", &[]));
        assert_eq!(url.host_str(), Some("localhost"));
    }

    #[test]
    fn closes_that_stop_the_client() {
        for code in [4001, 4002, 4004, 4005] {
            assert!(closed_by_server(Some(code)).is_err(), "{code}");
        }
        for code in [Some(1000), Some(1001), Some(4003), None] {
            assert_eq!(closed_by_server(code).unwrap(), Outcome::Reconnect);
        }
    }

    mod scenarios {
        use super::super::*;
        use axum::{Router, body::Bytes, http::HeaderMap as AxumHeaders, routing::post};
        use serde_json::{Value, json};
        use tokio::net::TcpListener;
        use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};

        const PASSPHRASE: &str = "passphrase";

        /// A forward endpoint that echoes what it received.
        async fn forward_endpoint() -> Url {
            let app = Router::new().route(
                "/slack/events",
                post(|headers: AxumHeaders, body: Bytes| async move {
                    let seen = json!({
                        "host": headers["host"].to_str().unwrap(),
                        "x-many": headers.get_all("x-many").iter().map(|v| v.to_str().unwrap()).collect::<Vec<_>>(),
                        "body": String::from_utf8(body.to_vec()).unwrap(),
                    });
                    (
                        axum::http::StatusCode::CREATED,
                        axum::response::AppendHeaders([("set-cookie", "a=1"), ("set-cookie", "b=2")]),
                        seen.to_string(),
                    )
                }),
            );
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            Url::parse(&format!("http://{addr}")).unwrap()
        }

        fn relay(server: &Url, forward: Url) -> Relay {
            Relay {
                client_id: "client-1".into(),
                server_endpoint: server.clone(),
                forward_endpoint: forward,
                challenge_passphrase: PASSPHRASE.into(),
                path: Some("/slack/events".into()),
                filter_body_regex: None,
                http: reqwest::Client::new(),
            }
        }

        type ServerWs = tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>;

        /// Accepts one client, checks its query and challenge response.
        // The callback's error type is fixed by tungstenite.
        #[allow(clippy::result_large_err)]
        async fn accept_authenticated(listener: &TcpListener) -> ServerWs {
            let (stream, _) = listener.accept().await.unwrap();
            let mut query = String::new();
            let mut ws =
                tokio_tungstenite::accept_hdr_async(stream, |req: &Request, resp: Response| {
                    query = req.uri().query().unwrap_or_default().to_string();
                    Ok(resp)
                })
                .await
                .unwrap();
            assert_eq!(query, "clientId=client-1&path=%2Fslack%2Fevents");

            send(&mut ws, json!({"kind": "challenge", "nonce": "nonce"})).await;
            let resp = recv(&mut ws).await;
            assert_eq!(
                resp,
                json!({"kind": "challenge-response", "clientId": "client-1", "hmac": challenge_hmac(PASSPHRASE, "nonce")})
            );
            send(
                &mut ws,
                json!({"kind": "challenge-result", "clientId": "client-1", "success": true}),
            )
            .await;
            ws
        }

        async fn send(ws: &mut ServerWs, msg: Value) {
            ws.send(Message::text(msg.to_string())).await.unwrap();
        }

        async fn recv(ws: &mut ServerWs) -> Value {
            loop {
                match ws.next().await.unwrap().unwrap() {
                    Message::Text(text) => return serde_json::from_str(text.as_str()).unwrap(),
                    Message::Close(frame) => panic!("closed: {frame:?}"),
                    _ => {}
                }
            }
        }

        async fn server() -> (TcpListener, Url) {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = Url::parse(&format!("ws://{}", listener.local_addr().unwrap())).unwrap();
            (listener, url)
        }

        #[tokio::test]
        async fn forwards_a_request_and_stops_on_authentication_failure() {
            let (listener, url) = server().await;
            let relay = relay(&url, forward_endpoint().await);
            let (_stop_tx, mut stop) = watch::channel(false);
            let client = tokio::spawn(async move { relay.connect(&mut stop).await });

            let mut ws = accept_authenticated(&listener).await;
            send(
                &mut ws,
                json!({
                    "kind": "http",
                    "messageId": "1-abc",
                    "headers": {
                        "host": "relay.example.com",
                        "content-length": "5",
                        "connection": "close",
                        "x-many": ["a", "b"],
                    },
                    "rawBody": BASE64.encode("hello"),
                    "method": "POST",
                    "path": "/slack/events",
                }),
            )
            .await;
            let resp = recv(&mut ws).await;
            assert_eq!(resp["kind"], "http");
            assert_eq!(resp["clientId"], "client-1");
            assert_eq!(resp["messageId"], "1-abc");
            assert_eq!(resp["status"], 201);
            assert_eq!(resp["headers"]["set-cookie"], json!(["a=1", "b=2"]));
            let body = BASE64.decode(resp["body"].as_str().unwrap()).unwrap();
            let seen: Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(
                seen,
                json!({"host": "relay.example.com", "x-many": ["a", "b"], "body": "hello"})
            );

            ws.close(Some(CloseFrame {
                code: CloseCode::from(4001),
                reason: "Authentication failed".into(),
            }))
            .await
            .unwrap();
            let err = client.await.unwrap().unwrap_err();
            assert!(err.to_string().contains("close code 4001"), "{err}");
        }

        #[tokio::test]
        async fn closes_normally_on_shutdown() {
            let (listener, url) = server().await;
            let relay = relay(&url, forward_endpoint().await);
            let (stop_tx, mut stop) = watch::channel(false);
            let client = tokio::spawn(async move { relay.connect(&mut stop).await });

            let mut ws = accept_authenticated(&listener).await;
            stop_tx.send_replace(true);
            match ws.next().await.unwrap().unwrap() {
                Message::Close(Some(frame)) => {
                    assert_eq!(frame.code, CloseCode::Normal);
                    assert_eq!(frame.reason.as_str(), "Client shutdown");
                }
                other => panic!("expected a close frame, got {other:?}"),
            }
            let _ = ws.flush().await;
            assert_eq!(client.await.unwrap().unwrap(), Outcome::Shutdown);
        }

        #[tokio::test]
        async fn reconnects_when_the_server_is_unreachable() {
            let (listener, url) = server().await;
            drop(listener);
            let relay = relay(&url, Url::parse("http://127.0.0.1:1").unwrap());
            let (_stop_tx, mut stop) = watch::channel(false);
            assert_eq!(relay.connect(&mut stop).await.unwrap(), Outcome::Reconnect);
        }
    }
}
