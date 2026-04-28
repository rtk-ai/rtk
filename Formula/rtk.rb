# typed: false
# frozen_string_literal: true

# Homebrew formula for rtk - Rust Token Killer (Homeserve mirror, macOS only)
# To install:
#   brew tap homeservefr/rtk https://github.com/HomeserveFR/rtk.git
#   brew install homeservefr/rtk/rtk
class Rtk < Formula
  desc "High-performance CLI proxy to minimize LLM token consumption"
  homepage "https://github.com/HomeserveFR/rtk"
  version "0.38.1"
  license "MIT"

  on_macos do
    on_intel do
      url "https://github.com/HomeserveFR/rtk/releases/download/v#{version}/rtk-x86_64-apple-darwin.tar.gz"
      sha256 "b198307d727432d8355a127c4997e0a9440280b2f3c3704e4135604fc01f54b3"
    end

    on_arm do
      url "https://github.com/HomeserveFR/rtk/releases/download/v#{version}/rtk-aarch64-apple-darwin.tar.gz"
      sha256 "73064058e3b287e0fa7f9d8d05cb7894b559bed6efe44d76863957c0d7a31b51"
    end
  end

  def install
    bin.install "rtk"
  end

  test do
    assert_match "rtk #{version}", shell_output("#{bin}/rtk --version")
  end
end
