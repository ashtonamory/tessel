# Contributing to Tessel

Thanks for helping. Tessel is a Linux desktop app written in Rust; see the
[README](README.md) for the system packages you need and
[docs/SPEC.md](docs/SPEC.md) for the design.

## Build and run

```sh
cargo build --workspace
cargo run --release -p app -- [FILE.tessel.json]
```

## Before you open a PR

CI runs these; run them first:

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
```

GPU tests (render output, the canvas texture, agent screenshots) skip when no
wgpu adapter is found. Add tests with your change.

## Branches and pull requests

- Branch from `main` and never commit to it directly.
- Name the branch with a prefix: `feat/`, `fix/`, `chore/` or `docs/`, for
  example `fix/rotated-ungroup`.
- Open a pull request against `main`. Keep it focused, and describe what
  changed and how you tested it.
- Pull requests are squash-merged, so the PR title becomes the commit message.
  Write it as a short summary of the change.

## Security issues

Do not file those as issues or PRs. See [SECURITY.md](SECURITY.md).

## License

By contributing you agree that your work is dual licensed under
MIT OR Apache-2.0, as described in the README.
