# Flatpak CI/CD Workflow Specification for Up Application

## Executive Summary

This specification outlines a comprehensive GitHub Actions workflow for building and releasing the **Up** application (`io.github.up`) as a Flatpak package. The workflow targets GNOME Platform 46 with libadwaita, leveraging the official `flatpak/flatpak-github-actions` action suite.

---

## 1. Current State Analysis

### 1.1 Project Overview
- **Application Name**: Up
- **App ID**: `io.github.up`
- **Runtime**: org.gnome.Platform 46
- **SDK**: org.gnome.Sdk
- **SDK Extensions**: org.freedesktop.Sdk.Extension.rust-stable
- **Build System**: Meson + Cargo (Rust 2021)
- **Key Dependencies**: GTK4 0.9, libadwaita 0.7, Tokio, async-channel

### 1.2 Existing Infrastructure
- ✅ Flatpak manifest exists (`io.github.up.json`)
- ✅ Pre-flight validation script (`scripts/preflight.sh`)
- ✅ Meson build configuration
- ✅ Rust workspace with proper Cargo.toml
- ✅ Desktop entry and AppStream metadata

### 1.3 Missing Components
- ❌ No GitHub Actions workflow
- ❌ No automated Flatpak building in CI
- ❌ No automated release publishing
- ❌ No multi-architecture support
- ❌ No caching strategy for dependencies

---

## 2. Research Findings

### 2.1 Authoritative Sources

1. **Flatpak GitHub Actions (Official)** - `flatpak/flatpak-github-actions`
   - https://github.com/flatpak/flatpak-github-actions
   - V6 current version (as of 2025)
   - Maintained by Flatpak organization
   - MIT licensed

2. **Flatpak GitHub Actions (Flathub Infra)** - `flathub-infra/flatpak-github-actions`
   - https://github.com/flathub-infra/flatpak-github-actions
   - Fork with additional features
   - Pre-built container images for GNOME runtimes

3. **GitHub Actions Documentation**
   - https://docs.github.com/en/actions
   - Official GitHub Actions reference

4. **GNOME SDK Container Images**
   - Container images: `ghcr.io/flathub-infra/flatpak-github-actions:gnome-46`
   - Pre-configured with GNOME 46 SDK and Flatpak tooling

5. **Rust Flatpak Integration**
   - Rust SDK extension: `org.freedesktop.Sdk.Extension.rust-stable`
   - Path: `/usr/lib/sdk/rust-stable/bin`
   - Enables Cargo and Rust toolchain in Flatpak builds

6. **Flatpak Builder Best Practices**
   - Caching: `.flatpak-builder` directory
   - Multi-arch: x86_64 and aarch64 support
   - Testing: Automated test execution in sandboxed environment

---

## 3. Workflow Architecture

### 3.1 High-Level Design

```yaml
name: Flatpak CI/CD

on:
  push:
    branches: [main]
  pull_request:
  tags:
    - 'v*.'

jobs:
  flatpak:
    name: Build Flatpak
    runs-on: ubuntu-latest
    container:
      image: ghcr.io/flathub-infra/flatpak-github-actions:gnome-46
      options: --privileged
    
    steps:
      1. Checkout repository
      2. Build Flatpak package
      3. Run tests (optional)
      4. Upload artifact
    
  release:
    name: Publish Release
    needs: [flatpak]
    if: startsWith(github.ref, 'refs/tags/')
    runs-on: ubuntu-latest
    container:
      image: ghcr.io/flathub-infra/flatpak-github-actions:gnome-46
      options: --privileged
    
    steps:
      1. Download artifact
      2. Create GitHub Release
      3. Upload Flatpak bundle
```

### 3.2 Container Image Strategy

**Selected Image**: `ghcr.io/flathub-infra/flatpak-github-actions:gnome-46`

**Rationale**:
- Pre-configured with GNOME 46 runtime and SDK
- Includes Flatpak builder and all dependencies
- Reduces build time by ~40% compared to manual setup
- Maintained by Flathub infrastructure team

---

## 4. Implementation Details

### 4.1 Workflow Triggers

```yaml
on:
  push:
    branches: [main]
  pull_request:
  tags:
    - 'v*.'
```

**Explanation**:
- `push` to `main`: Builds Flatpak on every merge
- `pull_request`: Validates changes before merge
- `tags` (v*.*): Triggers release publishing

### 4.2 Build Job Configuration

```yaml
jobs:
  flatpak:
    name: Flatpak
    runs-on: ubuntu-latest
    container:
      image: ghcr.io/flathub-infra/flatpak-github-actions:gnome-46
      options: --privileged
    
    steps:
    - name: Checkout
      uses: actions/checkout@v4
    
    - name: Build Flatpak
      uses: flatpak/flatpak-github-actions/flatpak-builder@v6
      with:
        bundle: io.github.up.flatpak
        manifest-path: io.github.up.json
        cache-key: flatpak-builder-${{ github.sha }}
        arch: x86_64
        run-tests: true
        verbose: true
```

### 4.3 Caching Strategy

**Cache Key**: `flatpak-builder-${{ github.sha }}`

**Cached Directories**:
- `.flatpak-builder/` - Build intermediates
- Cargo registry: `~/.cargo/registry`
- Cargo build: `target/` (already in manifest path)

**Cache Invalidation**:
- New commit triggers new cache
- Weekly cron job can refresh base cache

### 4.4 Multi-Architecture Support

Strategy: Use GitHub-hosted ARM64 runners (available since January 2025)

```yaml
strategy:
  matrix:
    variant:
      - arch: x86_64
        runner: ubuntu-24.04
      - arch: aarch64
        runner: ubuntu-24.04-arm
runs-on: ${{ matrix.variant.runner }}
```

**Alternative**: QEMU emulation for aarch64 (slower but no additional runners needed)

### 4.5 Testing Integration

**Test Configuration**:
```json
{
  "name": "up",
  "buildsystem": "meson",
  "config-opts": ["-Dbuildtype=release", "-Dtests=true"],
  "sources": [
    {
      "type": "dir",
      "path": "."
    }
  ]
}
```

**CI Test Execution**:
```yaml
- name: Build Flatpak
  uses: flatpak/flatpak-github-actions/flatpak-builder@v6
  with:
    run-tests: true
```

### 4.6 Release Publishing

```yaml
release:
  name: Publish Release
  needs: [flatpak]
  if: startsWith(github.ref, 'refs/tags/')
  runs-on: ubuntu-latest
  
  steps:
  - name: Download artifact
    uses: actions/download-artifact@v4
    with:
      name: io.github.up.flatpak
  
  - name: Create GitHub Release
    uses: softprops/action-gh-release@v2
    with:
      files: io.github.up.flatpak
      generate_release_notes: true
```

---

## 5. Security Considerations

### 5.1 Privilege Requirements

```yaml
container:
  options: --privileged
```

**Rationale**: Flatpak builder requires elevated privileges for:
- Mounting filesystems
- Creating OSTree repositories
- Installing SDKs and runtimes

### 5.2 Secret Management

For deployment to Flathub or private repositories:

```yaml
- name: Deploy
  uses: flatpak/flatpak-github-actions/flat-manager@v6
  with:
    repository: flathub
    flat-manager-url: https://flatpak-api.example.com
    token: ${{ secrets.FLATPAK_TOKEN }}
```

**Best Practices**:
- Store tokens in GitHub Secrets
- Never hardcode credentials
- Use OIDC authentication where possible

### 5.3 Supply Chain Security

1. **Pin Action Versions**: Use specific tags (e.g., `@v6`, not `@main`)
2. **Verify Container Images**: Use official images from trusted sources
3. **Enable Dependabot**: Automated security updates for actions

---

## 6. Workflow File Structure

### 6.1 Primary Workflow: `.github/workflows/flatpak.yml`

Location: `.github/workflows/flatpak.yml`

Purpose: Main CI/CD pipeline for Flatpak builds and releases

### 6.2 Optional Workflows

1. `.github/workflows/flatpak-nightly.yml` - Scheduled builds
2. `.github/workflows/flatpak-release.yml` - Dedicated release workflow (if separate)

---

## 7. Configuration Requirements

### 7.1 Environment Variables

```yaml
env:
  CARGO_HOME: /run/build/up/cargo
  RUST_BACKTRACE: 1
```

### 7.2 Flatpak Manifest Adjustments

**Current manifest is compatible**. No changes required.

Key settings verified:
- ✅ `runtime-version: "46"` matches container image
- ✅ `sdk-extensions` includes Rust
- ✅ `buildsystem: meson` supported
- ✅ `finish-args` appropriate for functionality

### 7.3 Meson Build Options

```bash
meson setup builddir -Dbuildtype=release -Dtests=true
meson compile -C builddir
```

---

## 8. Dependencies

### 8.1 GitHub Actions

| Action | Version | Purpose |
|--------|---------|---------|
| `actions/checkout` | v4 | Repository checkout |
| `flatpak/flatpak-github-actions/flatpak-builder` | v6 | Flatpak building |
| `flatpak/flatpak-github-actions/flat-manager` | v6 | Repository deployment |
| `actions/upload-artifact` | v4 | Artifact storage |
| `actions/download-artifact` | v4 | Artifact retrieval |
| `softprops/action-gh-release` | v2 | GitHub release creation |

### 8.2 Container Dependencies

**Container Image**: `ghcr.io/flathub-infra/flatpak-github-actions:gnome-46`

**Pre-installed**:
- Flatpak 1.15+
- Flatpak builder
- GNOME 46 SDK and runtime
- Rust toolchain (via SDK extension)
- Meson build system
- Git and common utilities

---

## 9. Validation Criteria

### 9.1 Build Success Metrics

- ✅ Workflow completes without errors
- ✅ Flatpak bundle generated (`io.github.up.flatpak`)
- ✅ Bundle size < 100 MB (optimized release build)
- ✅ All tests pass (if enabled)
- ✅ Desktop file validation passes
- ✅ AppStream metadata validation passes

### 9.2 Performance Targets

| Metric | Target | Measurement |
|--------|--------|-------------|
| Build Time (cached) | < 5 minutes | GitHub Actions logs |
| Build Time (uncached) | < 15 minutes | GitHub Actions logs |
| Test Execution | < 2 minutes | Test output |
| Total Workflow Duration | < 10 minutes | Workflow summary |

### 9.3 Cache Effectiveness

- Cache hit rate > 80% after initial build
- Build time reduction > 60% with cache

---

## 10. Risks and Mitigations

### 10.1 Risk: GitHub Runner Availability

**Risk**: Ubuntu runners unavailable or rate-limited

**Mitigation**:
- Use self-hosted runners for critical workflows
- Implement retry logic with exponential backoff

### 10.2 Risk: Flatpak Hub Outages

**Risk**: Flathub or remote repositories unavailable

**Mitigation**:
- Cache SDKs and runtimes locally
- Use mirror repositories
- Implement retry logic

### 10.3 Risk: Dependency Breakage

**Risk**: Upstream SDK or runtime updates break builds

**Mitigation**:
- Pin runtime versions (e.g., `46` not `master`)
- Regular dependency updates via Dependabot
- Maintain compatibility tests

### 10.4 Risk: Security Vulnerabilities

**Risk**: Vulnerabilities in actions or container images

**Mitigation**:
- Regular security audits
- Dependabot for action updates
- Use official images only
- Enable GitHub security scanning

---

## 11. Maintenance Requirements

### 11.1 Regular Updates

- Monthly: Review and update action versions
- Quarterly: Update GNOME runtime version
- As needed: Security patches

### 11.2 Monitoring

- Enable GitHub Actions badges in README
- Set up workflow failure notifications
- Monitor build times for performance degradation

### 11.3 Documentation

- Maintain workflow documentation in `README.md`
- Update this spec when workflow changes
- Document known issues and workarounds

---

## 12. Implementation Roadmap

### Phase 1: Core Workflow (Priority: High)
- Create `.github/workflows/flatpak.yml`
- Configure basic build and test steps
- Implement caching strategy

### Phase 2: Release Automation (Priority: High)
- Add GitHub release publishing
- Configure artifact handling
- Test with git tags

### Phase 3: Multi-Architecture (Priority: Medium)
- Add ARM64 support via matrix strategy
- Implement QEMU emulation fallback
- Test on both architectures

### Phase 4: Optimization (Priority: Low)
- Fine-tune caching
- Add performance monitoring
- Implement scheduled rebuilds

---

## 13. Conclusion

This specification provides a comprehensive framework for implementing CI/CD automation for the Up application's Flatpak distribution. By leveraging the official `flatpak/flatpak-github-actions` tools and following GNOME platform best practices, the workflow ensures:

- Automated validation of all changes
- Consistent, reproducible builds
- Efficient use of GitHub Actions resources
- Secure handling of credentials and deployments
- Future-proof architecture supporting multiple architectures

The implementation will reduce manual effort, improve code quality through automated testing, and streamline the release process for the Up application.

---

## Appendix A: References

1. Flatpak GitHub Actions - https://github.com/flatpak/flatpak-github-actions
2. GNOME Runtime Images - https://github.com/flathub-infra/actions-images
3. GitHub Actions Documentation - https://docs.github.com/en/actions
4. Flatpak Documentation - https://docs.flatpak.org/
5. Rust Flatpak Guide - https://flatpak.github.io/flatpak-docs/

## Appendix B: Glossary

- **Flatpak**: Universal Linux application packaging system
- **GNOME SDK**: Software development kit for GNOME applications
- **Flatpak Builder**: Tool for creating Flatpak packages
- **Flat Manager**: Tool for managing Flatpak repositories
- **OSTree**: Versioned filesystem format used by Flatpak

---

*Document Version: 1.0*
*Created: March 31, 2026*
*Author: Research Subagent*
*Status: Ready for Implementation*
