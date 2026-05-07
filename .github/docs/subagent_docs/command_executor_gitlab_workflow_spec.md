# GitLab CI Workflow Spec — Up Project

## Purpose

This spec documents the equivalent GitLab CI pipeline for the GitHub Actions workflow defined in `.github/workflows/ci.yml`. The pipeline mirrors all build, lint, test, and validation steps so that the GitLab mirror at VictoryTek/Up receives the same CI enforcement as the GitHub repository.

---

## Source of Truth

| File | Role |
|------|------|
| `.github/workflows/ci.yml` | GitHub Actions CI (reference) |
| `scripts/preflight.sh` | Local preflight script (same steps) |
| `Cargo.toml` | Project metadata, edition 2021, stable Rust |

---

## Jobs to Replicate

### GitHub Actions: `native-build`

Steps in order:

1. Checkout source
2. Install Rust stable with `clippy` and `rustfmt` components
3. Cache Cargo registry, git, and target/
4. Install system dependencies via `apt-get`
5. `cargo fmt --check`
6. `cargo clippy -- -D warnings`
7. `cargo build --release`
8. `cargo test`
9. `desktop-file-validate data/io.github.up.desktop`
10. `appstreamcli validate --no-net data/io.github.up.metainfo.xml`

### GitHub Actions: `validation`

A summary job that depends on `native-build` and fails if it failed. GitLab CI handles this natively via job dependencies (`needs:`), so no separate job is required — pipeline-level pass/fail is sufficient.

---

## Docker Image

Use `ubuntu:24.04` to match `runs-on: ubuntu-24.04` in the GitHub Actions workflow.

---

## System Dependencies (apt-get)

```
libgtk-4-dev
libadwaita-1-dev
libglib2.0-dev
libcairo2-dev
libpango1.0-dev
pkg-config
cmake
ninja-build
desktop-file-utils
appstream
curl
```

`appstream` provides `appstreamcli`. In GitHub Actions it is conditionally installed; in GitLab CI it is installed unconditionally in `before_script` to keep the pipeline deterministic.

`curl` is needed to install `rustup`.

---

## Rust Toolchain Installation

GitLab CI does not have a `dtolnay/rust-toolchain` action equivalent. Use `rustup` directly:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable --component clippy,rustfmt
source "$HOME/.cargo/env"
```

---

## Caching Strategy

GitLab CI caches are keyed by file hash. Cargo artifacts must reside inside `CI_PROJECT_DIR` to be cacheable.

```yaml
variables:
  CARGO_HOME: "${CI_PROJECT_DIR}/.cargo"

cache:
  key:
    files:
      - Cargo.lock
  paths:
    - .cargo/registry/
    - .cargo/git/
    - target/
```

This is equivalent to `Swatinem/rust-cache@v2` in GitHub Actions.

---

## Environment Variables

| Variable | Value | Reason |
|----------|-------|--------|
| `CARGO_TERM_COLOR` | `always` | Matches GitHub Actions global env |
| `CARGO_HOME` | `${CI_PROJECT_DIR}/.cargo` | Required for GitLab CI cache to work |

---

## Pipeline Trigger Rules

Use `rules:` (not deprecated `only:`/`except:`):

- Run on pushes to `main` branch
- Run on merge request events

```yaml
rules:
  - if: '$CI_COMMIT_BRANCH == "main"'
  - if: '$CI_PIPELINE_SOURCE == "merge_request_event"'
```

---

## Job Configuration

| Setting | Value |
|---------|-------|
| Job name | `native-build` |
| Image | `ubuntu:24.04` |
| `interruptible` | `true` |

---

## Output Files

| File | Purpose |
|------|---------|
| `.gitlab-ci.yml` | GitLab CI pipeline configuration |
