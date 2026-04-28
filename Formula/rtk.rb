# typed: false
# frozen_string_literal: true

# Homebrew formula for rtk - Rust Token Killer (Homeserve mirror, macOS only)
# To install:
#   brew tap homeservefr/rtk https://github.com/HomeserveFR/rtk.git
#   brew install homeservefr/rtk/rtk
class Rtk < Formula
  desc "High-performance CLI proxy to minimize LLM token consumption"
  homepage "https://github.com/HomeserveFR/rtk"
  version "0.38.0"
  license "MIT"

  on_macos do
    on_intel do
      url "https://github.com/HomeserveFR/rtk/releases/download/v#{version}/rtk-x86_64-apple-darwin.tar.gz"
      sha256 "b628d2878912571994975ff5cd001fd30f5c796252e571ded6a588e043edc06a"
    end

    on_arm do
      url "https://github.com/HomeserveFR/rtk/releases/download/v#{version}/rtk-aarch64-apple-darwin.tar.gz"
      sha256 "21dd4c34d345737ef22770f1391243b3d4267e801e40ad2b75218c5c9e330054"
    end
  end

  def install
    bin.install "rtk"
  end

  test do
    assert_match "rtk #{version}", shell_output("#{bin}/rtk --version")
  end
end
