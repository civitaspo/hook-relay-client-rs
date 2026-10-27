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
- Keep the command-line flags, environment variables, and wire format compatible with the upstream client. Document every intended difference in the README.

## Local tooling

Install the pinned tools with:

```bash
mise install --locked
```

Run these checks before opening a pull request:

```bash
mise run lint
mise run test
mise run build
```

## GitHub Actions and credentials

- Pin every GitHub Action to an immutable commit SHA and keep `persist-credentials: false` on checkout steps.
- Keep workflow permissions at the least-privilege level.
- Pass each secret explicitly. The three managed request callers and shared
  release publisher inherit the Client App key; keep their lint exceptions
  scoped to those four jobs.
- Use `actions/client/securefix` for automated fixes and signed machine commits.
- Keep Server and Approver App keys only in the `csm-ci` environment of
  `civitaspo/csm-ci-server`. This client holds only the Client App key.
- See [docs/securefix.md](docs/securefix.md) for the client/server setup.

## Release

Releases are prepared by **csm-ci-release-pr** on `release/next`. It updates `.release-version`, `CHANGELOG.md`, and the root versions in `Cargo.toml` and `Cargo.lock` (the csm-ci-server `version_files`) to the same version. The owner requests `/approve` and `/merge` on the current release PR head. The server builds and verifies all four raw binaries before publishing the annotated tag and immutable release with sidecar checksums and provenance. Do not edit `CHANGELOG.md` or `.release-version` on feature pull requests. See [docs/releasing.md](docs/releasing.md).
