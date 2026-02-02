#!/usr/bin/env bash
#
# Update Homebrew formula with latest release information
#
set -euo pipefail

REPO_OWNER="pszymkowiak"
REPO_NAME="rtk"
FORMULA_FILE="Formula/rtk.rb"

# Get latest release tag
LATEST_TAG=$(gh release view --json tagName --jq .tagName 2>/dev/null || echo "")
if [ -z "$LATEST_TAG" ]; then
    echo "❌ Failed to get latest release tag"
    exit 1
fi

VERSION="${LATEST_TAG#v}"
echo "📦 Updating formula to version $VERSION"

# Download checksums
CHECKSUMS_URL="https://github.com/$REPO_OWNER/$REPO_NAME/releases/download/$LATEST_TAG/checksums.txt"
CHECKSUMS=$(curl -fsSL "$CHECKSUMS_URL")

# Extract checksums for each platform
SHA_AARCH64_DARWIN=$(echo "$CHECKSUMS" | grep "rtk-aarch64-apple-darwin.tar.gz" | awk '{print $1}')
SHA_X86_64_DARWIN=$(echo "$CHECKSUMS" | grep "rtk-x86_64-apple-darwin.tar.gz" | awk '{print $1}')
SHA_AARCH64_LINUX=$(echo "$CHECKSUMS" | grep "rtk-aarch64-unknown-linux-gnu.tar.gz" | awk '{print $1}')
SHA_X86_64_LINUX=$(echo "$CHECKSUMS" | grep "rtk-x86_64-unknown-linux-gnu.tar.gz" | awk '{print $1}')

echo "✅ Retrieved checksums:"
echo "  - macOS ARM64:   $SHA_AARCH64_DARWIN"
echo "  - macOS x86_64:  $SHA_X86_64_DARWIN"
echo "  - Linux ARM64:   $SHA_AARCH64_LINUX"
echo "  - Linux x86_64:  $SHA_X86_64_LINUX"

# Generate formula content
cat > "$FORMULA_FILE" << FORMULA
class Rtk < Formula
  desc "Rust Token Killer - High-performance CLI proxy to minimize LLM token consumption"
  homepage "https://github.com/$REPO_OWNER/$REPO_NAME"
  version "$VERSION"
  license "MIT"

  if OS.mac? && Hardware::CPU.arm?
    url "https://github.com/$REPO_OWNER/$REPO_NAME/releases/download/$LATEST_TAG/rtk-aarch64-apple-darwin.tar.gz"
    sha256 "$SHA_AARCH64_DARWIN"
  elsif OS.mac? && Hardware::CPU.intel?
    url "https://github.com/$REPO_OWNER/$REPO_NAME/releases/download/$LATEST_TAG/rtk-x86_64-apple-darwin.tar.gz"
    sha256 "$SHA_X86_64_DARWIN"
  elsif OS.linux? && Hardware::CPU.arm?
    url "https://github.com/$REPO_OWNER/$REPO_NAME/releases/download/$LATEST_TAG/rtk-aarch64-unknown-linux-gnu.tar.gz"
    sha256 "$SHA_AARCH64_LINUX"
  elsif OS.linux? && Hardware::CPU.intel?
    url "https://github.com/$REPO_OWNER/$REPO_NAME/releases/download/$LATEST_TAG/rtk-x86_64-unknown-linux-gnu.tar.gz"
    sha256 "$SHA_X86_64_LINUX"
  end

  def install
    bin.install "rtk"
  end

  def caveats
    <<~EOS
      🚀 rtk is installed! Get started:

        # Initialize for Claude Code
        rtk init --global    # Add to ~/CLAUDE.md (all projects)
        rtk init             # Add to ./CLAUDE.md (this project)

        # See all commands
        rtk --help

        # Measure your token savings
        rtk gain

      📖 Full documentation: https://github.com/$REPO_OWNER/$REPO_NAME
    EOS
  end

  test do
    assert_match "rtk #{version}", shell_output("#{bin}/rtk --version")
  end
end
FORMULA

echo "✅ Formula updated successfully at $FORMULA_FILE"
echo ""
echo "📋 Next steps:"
echo "  1. Test the formula: brew install --build-from-source $FORMULA_FILE"
echo "  2. Audit the formula: brew audit --strict $FORMULA_FILE"
echo "  3. Commit and push: git add $FORMULA_FILE && git commit -m 'chore: update formula to v$VERSION'"
