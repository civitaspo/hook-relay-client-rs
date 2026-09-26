# Repository Guidelines

## Project scope

hook-relay-client-rs is a Rust implementation of the [hook-relay](https://github.com/itkq/hook-relay) client. It connects to a hook-relay server over WebSocket, answers the server's HMAC challenge, forwards each relayed HTTP request to a local endpoint, and sends the response back. It must stay wire-compatible with the upstream hook-relay server; the upstream TypeScript client is the reference for behavior and command-line flags.

## Contribution rules

- Write commits, pull request titles and bodies, documentation, comments, and workflow messages in English only.
- Use Conventional Commits for pull request titles: `feat`, `fix`, `docs`, `refactor`, `test`, `ci`, `build`, `chore`, `perf`, or `revert`. Use `!` for breaking changes.
- Never push directly to `main`. Open a pull request and squash-merge it after the required checks and review pass.
- Sign commits. Do not amend or rewrite commits that have already been merged.
- Keep changes focused. Do not add implementation code to setup-only changes.
- Use the MIT License for this repository.

## Local tooling

Install the pinned tools with:

```bash
mise install --locked
```

Run this check before opening a pull request:

```bash
mise run lint
```

## GitHub Actions and credentials

- Pin every GitHub Action to an immutable commit SHA and keep `persist-credentials: false` on checkout steps.
- Keep workflow permissions at the least-privilege level.
- Do not use `secrets: inherit`; pass each secret explicitly.
- Use Securefix for automated workflow fixes and signed machine commits.
- Keep GPG keys, machine-user tokens, server app keys, and other strong credentials only in `civitaspo/securefix-server`.
- See [docs/securefix.md](docs/securefix.md) for the client/server setup.
