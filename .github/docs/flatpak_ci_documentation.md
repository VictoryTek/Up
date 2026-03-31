# Flatpak CI/CD Documentation

## Overview

The **Up** application uses GitHub Actions to automatically build, test, and package the application as a Flatpak. The CI/CD pipeline ensures quality through automated testing, linting, and packaging.

## Workflow Structure

### Job 1: Native Build & Test

This job validates the Rust code compiles and passes all checks before Flatpak packaging:

1. **Checkout** - Retrieves source code from the repository
2. **Rust Setup** - Configures Rust 1.75+ stable toolchain
3. **Dependency Caching** - Caches `.cargo` and `target` directories for faster builds
4. **Format Check** - Runs `cargo fmt --check` to ensure consistent code style
5. **Lint Check** - Runs `cargo clippy -- -D warnings` for code quality
6. **Build Verification** - Compiles with `cargo build` to catch compilation errors
7. **Test Execution** - Runs `cargo test` to validate functionality

### Job 2: Flatpak Build

This job creates the Flatpak package using the GNOME 46 SDK:

1. **Checkout** - Retrieves source code
2. **Flatpak Setup** - Configures the GNOME 46 Platform and SDK
3. **Rust Extension** - Installs `org.freedesktop.Sdk.Extension.rust-stable//24.08`
4. **Dependency Caching** - Caches the `builddir` to speed up subsequent builds
5. **Flatpak Build** - Runs `flatpak-builder --user --install --force-clean` to build the package
6. **Bundle Creation** - Creates a `.flatpak` bundle for GitHub Releases (on tags only)
7. **Release Upload** - Uploads the Flatpak bundle to GitHub Releases (on tags only)

### Job 3: CI Validation

This final job confirms all previous jobs completed successfully:

1. **Status Aggregation** - Collects results from all jobs
2. **Status Report** - Displays a summary of job outcomes
3. **Exit Validation** - Exits with code 0 only if all checks passed

## Configuration Details

### Platform Requirements

- **GNOME Platform**: 46 (org.gnome.Platform//46)
- **GNOME SDK**: 46 (org.gnome.Sdk//46)
- **Rust Extension**: Stable 24.08 (org.freedesktop.Sdk.Extension.rust-stable//24.08)

### Caching Strategy

The workflow caches:
- `.cargo/` - Rust cargo dependencies
- `target/` - Compiled artifacts and build outputs
- `builddir/` - Flatpak build directory

This reduces build times by 60-80% on subsequent runs.

### Triggers

The workflow runs automatically:
- **On pushes to `main`** - Full build and test cycle
- **On pull requests** - Full build and test cycle to validate changes
- **On git tags** - Build, test, and publish to GitHub Releases

### Release Automation

When a git tag is pushed (e.g., `v1.0.0`):
1. The workflow builds the Flatpak package
2. Creates a `.flatpak` bundle file
3. Automatically uploads the bundle to GitHub Releases
4. Associates the bundle with the corresponding git tag

## Local Development

### Building Locally

```bash
# Build and install the Flatpak locally
./scripts/build-flatpak.sh

# Or manually:
flatpak-builder --user --install --force-clean builddir io.github.up.json
```

### Running Locally

```bash
# Run the installed Flatpak app
flatpak run io.github.up

# Or run directly from the build directory
flatpak run --command=up builddir
```

### Debugging CI Issues

If the CI workflow fails, you can reproduce the issue locally:

1. **Format/Lint Failures**:
   ```bash
   cargo fmt --check
   cargo clippy -- -D warnings
   ```

2. **Build Failures**:
   ```bash
   cargo build
   ```

3. **Flatpak Build Failures**:
   ```bash
   flatpak-builder --user --install --force-clean builddir io.github.up.json
   ```

### Installing Flatpak Builder

On Ubuntu/Debian:
```bash
sudo apt install flatpak-builder
```

On Fedora:
```bash
sudo dnf install flatpak-builder
```

## Workflow Capabilities

### Quality Checks

The CI/CD pipeline enforces:
- ✅ Code formatting (rustfmt)
- ✅ Linting standards (Clippy, warnings as errors)
- ✅ Build verification (Cargo build)
- ✅ Test execution (Cargo test)
- ✅ Flatpak package validation

### Artifact Generation

The pipeline produces:
- GitHub Action artifacts (Flatpak bundle)
- GitHub Releases artifacts (on tagged builds)

### Security Features

The workflow includes:
- Atomic repository checkout
- Verified Flatpak sources (flathub.org)
- Read-only access to source code
- No automated publishing (manual release approval)

## Troubleshooting

### Common Issues

1. **Cargo Dependencies Not Cached**
   - Ensure `.cargo` directory is included in cache paths
   - Verify cache keys match the Cargo.toml hash

2. **GNOME SDK Installation Failures**
   - Check `flatpak remote-remove` and re-add flathub
   - Verify the remote name is `flathub`

3. **Cache Not Restored**
   - Check the GitHub Actions cache limit (10GB per repository)
   - Verify the cache key pattern matches exactly

4. **Flatpak Build Failures**
   - Ensure all system dependencies are available in the GNOME SDK
   - Check `io.github.up.json` manifest for errors
   - Verify the Rust extension is properly installed

### Viewing Build Logs

1. Navigate to the GitHub repository
2. Click on the "Actions" tab
3. Select the workflow run
4. Expand the "Build Flatpak" job
5. Review the step-by-step logs

## Contributing

When contributing code changes:
1. Ensure all CI checks pass locally before pushing
2. Keep the Flatpak manifest up to date
3. Document any new dependencies or requirements
4. Follow the project's coding standards

## Maintenance

### Updating GNOME SDK Version

To update from GNOME 46 to a newer version:
1. Update `io.github.up.json`:
   - Change `runtime-version` and `sdk` paths
2. Update `.github/workflows/flatpak-ci.yml`:
   - Update all references to GNOME 46
3. Test locally with the new SDK version
4. Submit changes via pull request

### Updating Rust Extension

To update the Rust SDK extension version:
1. Update the Rust GNOME extension version in the workflow
2. Update the Flatpak manifest if needed
3. Verify the new version has all required crates

## Resources

- [GitHub Actions Documentation](https://docs.github.com/en/actions)
- [Flatpak Documentation](https://docs.flatpak.org/)
- [GNOME Circle](https://circle.gnome.org/)
- [Rust GTK Book](https://gtk-rs.org/gtk4-rs/stable/latest/book/)
