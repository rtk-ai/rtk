class Rtk < Formula
  desc "Rust Token Killer - High-performance CLI proxy to minimize LLM token consumption"
  homepage "https://github.com/pszymkowiak/rtk"
  version "0.7.1"
  license "MIT"

  if OS.mac? && Hardware::CPU.arm?
    url "https://github.com/pszymkowiak/rtk/releases/download/v0.7.1/rtk-aarch64-apple-darwin.tar.gz"
    sha256 "add05b41ed2cb91d152801757a647e2eecbf9115f30bd1d3c77106870b4330fc"
  elsif OS.mac? && Hardware::CPU.intel?
    url "https://github.com/pszymkowiak/rtk/releases/download/v0.7.1/rtk-x86_64-apple-darwin.tar.gz"
    sha256 "51508705a81aa8cfcb8ee2a5711e480712366554c944e230bca39cfbe323c675"
  elsif OS.linux? && Hardware::CPU.arm?
    url "https://github.com/pszymkowiak/rtk/releases/download/v0.7.1/rtk-aarch64-unknown-linux-gnu.tar.gz"
    sha256 "c3041dc44f97a36558b75eebe8d368a62aa7598d6eca2088aaedb038eaaf5b01"
  elsif OS.linux? && Hardware::CPU.intel?
    url "https://github.com/pszymkowiak/rtk/releases/download/v0.7.1/rtk-x86_64-unknown-linux-gnu.tar.gz"
    sha256 "8e158b1710f2a8fb44a5b8d3f09a56c7c661e890b9b09144b4c477d8e808eee8"
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

      📖 Full documentation: https://github.com/pszymkowiak/rtk
    EOS
  end

  test do
    assert_match "rtk #{version}", shell_output("#{bin}/rtk --version")
  end
end
