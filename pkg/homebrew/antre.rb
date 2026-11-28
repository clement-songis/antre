class Antre < Formula
  desc "Personal AI gateway inspired by OpenClaw"
  homepage "https://www.moltis.org/"
  license "MIT"
  version "0.1.0"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/antre-ai/antre/releases/download/v#{version}/antre-#{version}-aarch64-apple-darwin.tar.gz"
    else
      url "https://github.com/antre-ai/antre/releases/download/v#{version}/antre-#{version}-x86_64-apple-darwin.tar.gz"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/antre-ai/antre/releases/download/v#{version}/antre-#{version}-aarch64-unknown-linux-gnu.tar.gz"
    else
      url "https://github.com/antre-ai/antre/releases/download/v#{version}/antre-#{version}-x86_64-unknown-linux-gnu.tar.gz"
    end
  end

  def install
    bin.install "antre"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/antre --version")
  end
end
