# Rust Migration And Release

> Last verified: 2026-06-16. Binary installation is release-gated: verify the
> current GitHub Releases assets before telling users to download binaries.

## Migration Path

LTO is moving from the Python CLI to Rust v2 in three steps.

1. **Source-build Rust path**
   ```bash
   cargo build --release --bin lto-rs
   cargo run -- self-test
   cargo run -- check --run-id <run-id> --json
   ```

2. **Wrapper default path**
   ```bash
   bash scripts/install.sh
   lto self-test
   lto recap --run-id <run-id>
   ```

3. **Explicit legacy fallback**
   ```bash
   lto --use-python self-test
   LTO_USE_PYTHON=1 lto check --run-id <run-id> --json
   ```

Python remains a compatibility fallback during the transition. Do not delete Python modules or tests just because a Rust command exists; first prove the Rust path owns the same behavior and keep a rollback path until the release assets and downstream host integrations are verified.

## Platform Policy

- Current supported release targets: Linux `x86_64-unknown-linux-musl`, macOS Apple Silicon `aarch64-apple-darwin`, macOS Intel `x86_64-apple-darwin`.
- Windows native release and runner support are paused. The built-in runner protocol is shell-script based (`scripts/delegate/runners/*.sh`, `healthcheck.sh`), so Windows support needs a separate native design and test pass.

## Binary Availability

Do not assume binaries exist from tags alone. Verify current GitHub Releases
before telling users to download:

```bash
curl -fsSL https://api.github.com/repos/bendusy/long-task-orchestration/releases
git ls-remote --tags origin
```

For the first Rust-default binary release (`v0.4.0`) and later, the expected
assets are:

- `lto-rs-x86_64-unknown-linux-musl.tar.gz`
- `lto-rs-x86_64-unknown-linux-musl.tar.gz.sha256`
- `lto-rs-aarch64-apple-darwin.tar.gz`
- `lto-rs-aarch64-apple-darwin.tar.gz.sha256`
- `lto-rs-x86_64-apple-darwin.tar.gz`
- `lto-rs-x86_64-apple-darwin.tar.gz.sha256`

Install from a release asset only after checking the hash:

```bash
curl -LO https://github.com/bendusy/long-task-orchestration/releases/latest/download/lto-rs-aarch64-apple-darwin.tar.gz
curl -LO https://github.com/bendusy/long-task-orchestration/releases/latest/download/lto-rs-aarch64-apple-darwin.tar.gz.sha256
shasum -a 256 -c lto-rs-aarch64-apple-darwin.tar.gz.sha256
tar -xzf lto-rs-aarch64-apple-darwin.tar.gz
./lto-rs self-test
```

## Release Workflow

Release is a host-owned gate, not a runner side effect.

1. Close functional blockers and record LTO evidence.
2. Run local gates:
   ```bash
   cargo fmt --all --check
   cargo check --locked --all-targets
   cargo clippy --locked --all-targets -- -D warnings
   cargo test --locked --all-targets
   python3 scripts/smoke_test.py
   git diff --check
   ```
3. Preview release metadata:
   ```bash
   cargo run -- release --part minor --date 2026-06-16 --dry-run
   ```
4. Update `VERSION` and `CHANGELOG.md`, commit, and create a `v*` tag using the host-owned plan from `lto release`.
5. Push the branch and tag.
6. Confirm GitHub Actions `rust-v2` passes and `release-binaries` uploads the `.tar.gz` and `.sha256` assets.
   The workflow must verify the packaged binary before upload and then download the uploaded GitHub Release asset, verify its checksum, unpack it, and run `lto-rs self-test`.
7. Download one asset independently, verify the checksum, unpack, and run `./lto-rs self-test` before announcing binaries.

## Development Gate

Before Rust takeover or cleanup work, write these four evidence items into run-state or task evidence:

- `architecture_alignment`: layer, boundary, and reused pattern.
- `first_principles`: real constraint, user value, or root cause.
- `simplification_dedupe`: deleted, merged, or reused code path before new code.
- `value_measurement`: baseline, metric, pass line, verification command, and post-change result.

Optimization without baseline and retest data is not a completed optimization.
