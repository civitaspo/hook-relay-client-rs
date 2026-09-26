//! Messages exchanged with a hook-relay server over WebSocket.
//!
//! The shapes follow `src/types.ts` in <https://github.com/itkq/hook-relay>.
//! Every message is a JSON object with a `kind` field.

use std::collections::BTreeMap;

use hmac::{Hmac, KeyInit, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

/// A message the server sends to the client.
#[derive(Debug, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ServerMessage {
    Challenge {
        nonce: String,
    },
    ChallengeResult {
        success: bool,
    },
    Http(HttpRequest),
    Error {
        #[serde(rename = "messageId")]
        message_id: String,
        error: String,
    },
}

/// An HTTP request the server received and relays to the client.
#[derive(Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HttpRequest {
    pub message_id: String,
    pub headers: Headers,
    /// The request body, base64-encoded.
    pub raw_body: String,
    pub method: String,
    pub path: String,
    #[serde(default)]
    pub query_params: BTreeMap<String, String>,
}

/// A message the client sends to the server.
#[derive(Debug, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ClientMessage {
    #[serde(rename_all = "camelCase")]
    ChallengeResponse { client_id: String, hmac: String },
    #[serde(rename_all = "camelCase")]
    Http {
        client_id: String,
        message_id: String,
        headers: Headers,
        status: u16,
        /// The response body, base64-encoded.
        body: String,
    },
}

/// HTTP headers as Node.js represents them: lower-case names mapped to a
/// string, or to a list of strings for headers such as `set-cookie`.
pub type Headers = BTreeMap<String, HeaderValue>;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(untagged)]
pub enum HeaderValue {
    One(String),
    Many(Vec<String>),
}

impl HeaderValue {
    pub fn values(&self) -> &[String] {
        match self {
            HeaderValue::One(v) => std::slice::from_ref(v),
            HeaderValue::Many(vs) => vs,
        }
    }
}

/// The hex-encoded HMAC-SHA256 of the challenge nonce keyed by the passphrase.
pub fn challenge_hmac(passphrase: &str, nonce: &str) -> String {
    let mut mac =
        Hmac::<Sha256>::new_from_slice(passphrase.as_bytes()).expect("HMAC accepts any key length");
    mac.update(nonce.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn challenge_hmac_matches_node_crypto() {
        // node -e "console.log(require('crypto').createHmac('sha256','passphrase').update('nonce').digest('hex'))"
        assert_eq!(
            challenge_hmac("passphrase", "nonce"),
            "b210ef7072ebcc8080c358a73c27abbdf2a997f693c81fc771c486cf395e6522"
        );
    }

    #[test]
    fn parses_server_messages() {
        let challenge: ServerMessage =
            serde_json::from_value(json!({"kind": "challenge", "nonce": "abc"})).unwrap();
        assert_eq!(
            challenge,
            ServerMessage::Challenge {
                nonce: "abc".into()
            }
        );

        let http: ServerMessage = serde_json::from_value(json!({
            "kind": "http",
            "messageId": "1-abc",
            "headers": {"content-type": "application/json", "x-many": ["a", "b"]},
            "rawBody": "e30=",
            "method": "POST",
            "path": "/slack/events",
        }))
        .unwrap();
        let ServerMessage::Http(req) = http else {
            panic!("not an http message: {http:?}");
        };
        assert_eq!(req.message_id, "1-abc");
        assert_eq!(req.headers["x-many"].values(), ["a", "b"]);
        assert!(req.query_params.is_empty());
    }

    #[test]
    fn serializes_client_messages() {
        let msg = ClientMessage::ChallengeResponse {
            client_id: "id".into(),
            hmac: "ff".into(),
        };
        assert_eq!(
            serde_json::to_value(msg).unwrap(),
            json!({"kind": "challenge-response", "clientId": "id", "hmac": "ff"})
        );
    }
}
