# Homebrew Tap for rtk

This is the Homebrew tap for [rtk](https://github.com/pszymkowiak/rtk) - the Rust Token Killer.

## Installation

```bash
brew tap pszymkowiak/rtk
brew install rtk
```

## Updating the Formula

To update the formula when a new version is released:

```bash
# Run the update script
./scripts/update-brew-formula.sh

# Verify the formula
brew style Formula/rtk.rb

# Test locally (optional)
brew install --build-from-source Formula/rtk.rb
rtk --version
```

## Manual Formula Updates

If you need to manually update the formula:

1. Get the latest release tag from GitHub
2. Download the checksums from the release
3. Update `Formula/rtk.rb` with:
   - New version number
   - New download URLs for each platform
   - New SHA256 checksums for each platform

## Tap Structure

This repository serves as a Homebrew tap. When users run `brew tap pszymkowiak/rtk`, Homebrew adds this repository as a source for formulae. The formula itself is located at `Formula/rtk.rb`.

## Formula Details

The rtk formula:
- Supports macOS (Intel and Apple Silicon) and Linux (x86_64 and ARM64)
- Installs pre-built binaries from GitHub releases
- Includes version checks in the test block
- Shows helpful caveats after installation

## Resources

- [Homebrew Documentation](https://docs.brew.sh/)
- [Formula Cookbook](https://docs.brew.sh/Formula-Cookbook)
- [rtk Repository](https://github.com/pszymkowiak/rtk)
