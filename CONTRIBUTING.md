# Contributing to Eskiz

Thank you for helping. This file explains how to build, what a good change looks like, and the sign-off rule.

## Build and test

You need [rustup](https://rustup.rs); the pinned Rust version installs automatically. Before you open a pull request, run:

```
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo build --release -p eskiz-app --locked
./target/release/eskiz --selftest
```

CI runs the same steps on Linux, macOS and Windows with GitHub-hosted runners. Please add a test for a bug you fix or a feature you add. `INTEGRATION.md` explains how the crates fit together, `BENCHMARKS.md` how to measure performance (`crates/eskiz-bench`), and `docs/guide/` is the in-app user guide, which is compiled into the binary.

## Sign-off (DCO)

Every commit must carry a `Signed-off-by:` line, which certifies the [Developer Certificate of Origin](https://developercertificate.org/): that you wrote the change or have the right to submit it under the project license. Add it with `git commit -s`. A pull request with unsigned commits will be asked to fix them.

**AI-assisted commits.** You may use AI tools. A tool cannot sign off. The human who submits the change must read it, understand it, take responsibility for it, and add their own `Signed-off-by:` line. Do not add a sign-off on behalf of someone else, and do not submit code you could not explain in review.

## Originality rules

- Eskiz is an original painting app. Its icons, brushes, textures and artwork are drawn in code.
- Write contributions from public documentation and your own work. Do not submit code copied from another application, or code derived by decompiling or otherwise reverse engineering software.
- Do not use another app's name, logo, brushes, textures or other assets in code, artwork or documentation.
- `.procreate` is supported as a file format for compatibility. The name refers only to that format.
- Dependencies must have licenses allowed by `deny.toml` (`cargo deny check licenses`).

## Pull requests

Keep a pull request focused. Describe what changed and why, and how you tested it. For larger features, open an issue first so we can agree on the approach. Be kind: see the [Code of Conduct](CODE_OF_CONDUCT.md).

## License

By contributing you agree that your contribution is licensed under the project's licenses, MIT OR Apache-2.0, as described in the README.
