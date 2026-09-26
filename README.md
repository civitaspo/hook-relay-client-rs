# hook-relay-client-rs

[![CI](https://github.com/civitaspo/hook-relay-client-rs/actions/workflows/pull_request.yml/badge.svg)](https://github.com/civitaspo/hook-relay-client-rs/actions/workflows/pull_request.yml)

hook-relay-client-rs is a Rust implementation of the [hook-relay](https://github.com/itkq/hook-relay) client. It connects to a hook-relay server over WebSocket and forwards the webhooks the server relays to a local endpoint, such as a development server.

## Development

```bash
mise install --locked
mise run lint
```

## License

hook-relay-client-rs is licensed under the MIT License. See [LICENSE](LICENSE).
