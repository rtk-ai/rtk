# typed: false
# frozen_string_literal: true

# Homebrew formula for rtk - Rust Token Killer
# To install: brew tap rtk-ai/tap && brew install rtk
class Rtk < Formula
  desc "Rust Token Killer - High-performance CLI proxy to minimize LLM token consumption"
  homepage "https://www.rtk-ai.app"
  version "0.37.2"
  license "MIT"

  on_macos do
    on_intel do
      url "https://github.com/rtk-ai/rtk/releases/download/v#{version}/rtk-x86_64-apple-darwin.tar.gz"
      sha256 "4052e7740a87e121f671a2de269b3f015dcc58b6171d6bedb300da7599cb4d94"
    end

    on_arm do
      url "https://github.com/rtk-ai/rtk/releases/download/v#{version}/rtk-aarch64-apple-darwin.tar.gz"
      sha256 "99e20a59847dedbb64032a3f7985f2fe959fcb9674d8eaf940fc58a189e27eca"
    end
  end

  on_linux do
    on_intel do
      url "https://github.com/rtk-ai/rtk/releases/download/v#{version}/rtk-x86_64-unknown-linux-musl.tar.gz"
      sha256 "3dfb7a05636a68687ba1c5aa696fa8d5fcb494447ded86d9eb8b88b7100a37c6"
    end

    on_arm do
      url "https://github.com/rtk-ai/rtk/releases/download/v#{version}/rtk-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "1d8d7fcca6cb05e1867c08bb4e5aa5f107c037c607131e511b726ae33ac35a47"
    end
  end

  def install
    bin.install "rtk"
  end

  def caveats
    <<~EOS
      rtk is installed! Get started:

        rtk init -g          # Global hook-first setup (recommended)
        rtk init             # Add to ./CLAUDE.md (this project only)
        rtk --help           # See all commands
        rtk gain             # Measure your token savings

      Full documentation: https://www.rtk-ai.app
    EOS
  end

  test do
    system "#{bin}/rtk", "--version"
  end
end
