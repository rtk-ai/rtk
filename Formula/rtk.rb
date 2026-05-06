# typed: false
# frozen_string_literal: true

# Homebrew formula for rtk - Rust Token Killer (Homeserve mirror)
# To install:
#   brew tap homeservefr/rtk https://github.com/HomeserveFR/rtk.git
#   brew install homeservefr/rtk/rtk
class Rtk < Formula
  desc "High-performance CLI proxy to minimize LLM token consumption"
  homepage "https://github.com/HomeserveFR/rtk"
  version "0.39.1"
  license "MIT"

  on_macos do
    on_intel do
      url "https://github.com/HomeserveFR/rtk/releases/download/v#{version}/rtk-x86_64-apple-darwin.tar.gz"
      sha256 "adc983c516e092d85faa0ad36ab79f4788fe011dd13aba3e24f49bd49ba9fabd"
    end

    on_arm do
      url "https://github.com/HomeserveFR/rtk/releases/download/v#{version}/rtk-aarch64-apple-darwin.tar.gz"
      sha256 "2c9499f09068a2596c95a8f084a614afdcc573ced83f873f3f11e13c98badcca"
    end
  end

  on_linux do
    on_intel do
      url "https://github.com/HomeserveFR/rtk/releases/download/v#{version}/rtk-x86_64-unknown-linux-musl.tar.gz"
      sha256 "9dd8a53afb80a6d55a5706f77cf859e382b70f3ba960a5cbf46ff20c0c51c9f5"
    end
  end

  def install
    bin.install "rtk"
  end

  test do
    assert_match "rtk #{version}", shell_output("#{bin}/rtk --version")
  end
end
