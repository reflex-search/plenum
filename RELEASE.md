# Release Process

This document describes how to create and publish releases for Plenum.

## Overview

Plenum uses [cargo-dist](https://axodotdev.github.io/cargo-dist/) to automate the release process. Releases are published to:
- **GitHub Releases**: Binary artifacts for multiple platforms
- **crates.io**: Rust package registry
- **npm**: JavaScript/Node.js package registry

## Prerequisites

Before creating your first release, you need to set up trusted publishing:

### 1. First Manual crates.io Release

crates.io requires the first release to be manual:

```bash
cargo publish
```

After the first release, configure trusted publishing on crates.io:
1. Go to your package settings on crates.io
2. Add GitHub Actions as a trusted publisher
3. Specify:
   - Repository: `reflex-search/plenum`
   - Workflow: `publish-packages.yml`

### 2. npm Trusted Publishing Setup

First, you need to publish the package manually once:

```bash
# Ensure you have npm >= 11.5.1
npm --version

# Login to npm
npm login

# Publish manually for the first time
npm publish --access public
```

After the first manual publish, configure trusted publishing:

1. Go to your package page: `https://www.npmjs.com/package/plenum`
2. Click the **"Settings"** tab
3. Scroll down to the **"Publishing access"** section
4. Click **"Automate publishing with GitHub Actions"** or **"Add trusted publisher"**
5. Fill in the form (**case-sensitive!**):
   - **Repository owner**: `reflex-search`
   - **Repository name**: `plenum`
   - **Workflow name**: `publish-packages.yml` (must include .yml extension)
   - **Environment name**: (leave blank)
6. Click "Add" or "Save"

**Optional but recommended**: After configuring trusted publishing, go back to Publishing access and select **"Require two-factor authentication and disallow tokens"** for enhanced security.

**Note**: Trusted publishing uses OIDC tokens automatically - no npm tokens or secrets needed in your workflow.

## Release Workflow

### Creating a Release

1. **Update Version**:
   ```bash
   # Edit Cargo.toml and update version field
   version = "0.2.0"
   ```

2. **Update Changelog**:
   ```bash
   # Edit CHANGELOG.md
   # Move items from [Unreleased] to new version section
   # Add release date
   ```

3. **Commit Changes**:
   ```bash
   git add Cargo.toml CHANGELOG.md
   git commit -m "chore: release v0.2.0"
   ```

4. **Create and Push Tag**:
   ```bash
   git tag v0.2.0
   git push origin main --tags
   ```

### What Happens Next

Once you push a version tag (e.g., `v0.2.0`), the following happens automatically:

1. **Release Workflow** (`.github/workflows/release.yml`):
   - Triggered by the version tag
   - Builds binaries for all target platforms:
     - Linux (x86_64, aarch64)
     - macOS (x86_64, aarch64)
     - Windows (x86_64)
   - Generates installers:
     - Shell script (`plenum-installer.sh`)
     - PowerShell script (`plenum-installer.ps1`)
     - npm package (`plenum-npm-package.tar.gz`)
   - Creates checksums for all artifacts
   - Creates a GitHub Release with all artifacts

2. **Publish Packages Workflow** (`.github/workflows/publish-packages.yml`):
   - Triggered after successful release workflow
   - Publishes to crates.io using trusted publishing (OIDC)
   - Downloads npm package from GitHub Release
   - Publishes to npm with provenance

## Installation Methods

After release, users can install Plenum via:

### npm (recommended for cross-platform)
```bash
npm install -g plenum
```

### Shell Script (Linux/macOS)
```bash
curl --proto '=https' --tlsv1.2 -LsSf \
  https://github.com/reflex-search/plenum/releases/latest/download/plenum-installer.sh | sh
```

### PowerShell (Windows)
```powershell
irm https://github.com/reflex-search/plenum/releases/latest/download/plenum-installer.ps1 | iex
```

### Cargo
```bash
cargo install plenum
```

### Manual Download
Download platform-specific binaries from [GitHub Releases](https://github.com/reflex-search/plenum/releases).

## Versioning

Plenum follows [Semantic Versioning](https://semver.org/):
- **MAJOR**: Breaking changes
- **MINOR**: New features (backward compatible)
- **PATCH**: Bug fixes (backward compatible)

## Manual Release Trigger

You can manually trigger the publish workflow for a specific tag:

```bash
gh workflow run publish-packages.yml -f tag=v0.2.0
```

## Troubleshooting

### Release Workflow Failed
- Check the GitHub Actions logs
- Ensure all tests pass
- Verify cargo-dist configuration in `dist-workspace.toml`

### crates.io Publishing Failed
- Verify trusted publishing is configured
- Check that version doesn't already exist
- Ensure Cargo.toml metadata is valid

### npm Publishing Failed
- Verify trusted publishing is configured
- Check npm package name isn't taken
- Ensure npm-package artifact was generated in release

## Configuration Files

- **dist-workspace.toml**: cargo-dist configuration
- **.github/workflows/release.yml**: Release automation (auto-generated)
- **.github/workflows/publish-packages.yml**: Package publishing
- **Cargo.toml**: Package metadata and version
- **CHANGELOG.md**: Version history

## Security

All publishing uses **trusted publishing** (OIDC):
- No long-lived tokens required
- Authentication via GitHub Actions identity
- Time-limited credentials (30 minutes)
- Reduced risk of credential leakage
