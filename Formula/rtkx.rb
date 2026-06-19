# typed: false
# frozen_string_literal: true

# Homebrew formula for rtkx - context compression CLI for the AXON stack (fork of rtk)
# To install: brew tap sammyjdev/rtkx && brew install rtkx
class Rtkx < Formula
  desc "Context compression CLI for the AXON stack (fork of rtk)"
  homepage "https://github.com/sammyjdev/rtkx"
  version "0.1.0"
  license "Apache-2.0"

  on_macos do
    on_intel do
      url "https://github.com/sammyjdev/rtkx/releases/download/v#{version}/rtkx-x86_64-apple-darwin.tar.gz"
      sha256 "PLACEHOLDER_SHA256_INTEL"
    end

    on_arm do
      url "https://github.com/sammyjdev/rtkx/releases/download/v#{version}/rtkx-aarch64-apple-darwin.tar.gz"
      sha256 "PLACEHOLDER_SHA256_ARM"
    end
  end

  on_linux do
    on_intel do
      url "https://github.com/sammyjdev/rtkx/releases/download/v#{version}/rtkx-x86_64-unknown-linux-musl.tar.gz"
      sha256 "PLACEHOLDER_SHA256_LINUX_INTEL"
    end

    on_arm do
      url "https://github.com/sammyjdev/rtkx/releases/download/v#{version}/rtkx-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "PLACEHOLDER_SHA256_LINUX_ARM"
    end
  end

  def install
    bin.install "rtkx"
  end

  test do
    assert_match "rtkx #{version}", shell_output("#{bin}/rtkx --version")
  end
end
