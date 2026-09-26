# hook-relay-client-rs

[![CI](https://github.com/civitaspo/hook-relay-client-rs/actions/workflows/pull_request.yml/badge.svg)](https://github.com/civitaspo/hook-relay-client-rs/actions/workflows/pull_request.yml)

hook-relay-client-rs is a Rust implementation of the [hook-relay](https://github.com/itkq/hook-relay) client. It connects to a hook-relay server over WebSocket and forwards the webhooks the server relays to a local endpoint, such as a development server.

It builds a single binary, `hook-relay-client`, with the same flags, environment variables, and wire format as the upstream Node.js client, so it works with an unmodified hook-relay server.

## Usage

```
$ hook-relay-client --help
Receives webhooks relayed by a hook-relay server over WebSocket and forwards them to another endpoint

Usage: hook-relay-client [OPTIONS] --server-endpoint <SERVER_ENDPOINT> --forward-endpoint <FORWARD_ENDPOINT> --challenge-passphrase <CHALLENGE_PASSPHRASE>

Options:
      --server-endpoint <SERVER_ENDPOINT>
          Server endpoint URL, such as wss://relay.example.com
      --forward-endpoint <FORWARD_ENDPOINT>
          Forward endpoint URL, such as http://localhost:9000
      --path <PATH>
          Receive only webhooks for this path (default: all paths)
      --log-level <LOG_LEVEL>
          Log level: error, warn, info, debug, or trace [env: LOG_LEVEL=] [default: info]
      --filter-body-regex <FILTER_BODY_REGEX>
          Receive only webhooks whose body matches this JavaScript regular expression; the server evaluates it and rejects unsafe patterns
      --reconnect-interval-ms <RECONNECT_INTERVAL_MS>
          Reconnect interval in milliseconds [default: 1000]
      --port <PORT>
          Port of the local HTTP server that reports the client ID [env: PORT=] [default: 3001]
      --challenge-passphrase <CHALLENGE_PASSPHRASE>
          Passphrase for the challenge response [env: CHALLENGE_PASSPHRASE]
  -h, --help
          Print help
  -V, --version
          Print version
```

Forward the Slack events that a hook-relay server receives at `/hook/slack/events` to a local app:

```bash
export CHALLENGE_PASSPHRASE='...'
hook-relay-client \
  --server-endpoint wss://relay.example.com \
  --forward-endpoint http://localhost:9000 \
  --path /slack/events
```

Register a one-shot OAuth callback for this client:

```bash
client_id=$(curl -SsfL localhost:3001 | jq -r .clientId)
curl -H 'content-type: application/json' \
  -XPOST https://relay.example.com/callback/oneshot/register \
  -d "{\"path\":\"/auth/redirect\",\"clientId\":\"$client_id\"}"
```

## Behavior

- The client answers the server's challenge with the HMAC-SHA256 of the nonce keyed by the passphrase, and sends a WebSocket ping every 20 seconds.
- Each relayed request goes to the forward endpoint's origin with the relayed path and query. Redirects are not followed; the response status, headers, and body go back to the server unchanged.
- Request and response headers are forwarded as received, including `host`, except hop-by-hop headers (`connection`, `keep-alive`, `transfer-encoding`, ...) and the request's `content-length`, which the client recomputes.
- If the forward endpoint cannot be reached, the client logs the error and does not answer, so another connected client can still answer the request.
- The client reconnects when the connection drops. It exits with status 1 when the server rejects it with close code 4001, 4002, 4004, or 4005, for example for a wrong passphrase or an unsafe `--filter-body-regex`.
- On the first SIGINT or SIGTERM, the client closes the WebSocket with code 1000 and waits up to one second. A second signal exits at once.
- Logs are human-readable on a terminal and JSON lines otherwise.

### Differences from the upstream client

- A relayed path is always set as the path of the forward endpoint, so a path such as `//other-host/` cannot send the request to another host.
- Hop-by-hop headers are not forwarded.
- `--filter-body-regex` is not checked locally. The server rejects unsafe patterns with close code 4005, and the client exits.

## Development

```bash
mise install --locked
mise run lint
mise run test
mise run build
```

## License

hook-relay-client-rs is licensed under the MIT License. See [LICENSE](LICENSE).
