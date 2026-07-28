# Contributing

Thank you for helping improve Skill BASIC.

## Before opening an issue

- Search existing issues first.
- Include a minimal BASIC program that reproduces the problem.
- Include the expected result, actual result, operating system, and Rust
  version.
- Report security vulnerabilities privately as described in `SECURITY.md`.

## Pull requests

Keep changes focused and add tests for behavior changes. Before submitting:

```console
cargo fmt --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo audit
```

By contributing, you agree that your contribution is licensed under the
Apache License 2.0.

## AI backend contributions

Ollama is the first backend in version 0.1, not a permanent provider
requirement. Proposals and focused contributions for provider-neutral
interfaces or additional AI services are welcome. Please open an issue before a
large backend change so configuration, privacy, testing, and compatibility can
be discussed first.
