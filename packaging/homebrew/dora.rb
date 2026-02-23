# dora Homebrew formula
#
# This file lives in the Jacke/homebrew-dora tap repo at Formula/dora.rb
# It is auto-updated by the tui-release.yml GitHub Actions workflow on each
# tui-v* tag push.
#
# To install manually (before the tap is wired up):
#   brew install --formula ./dora.rb
#
# Normal install after tap is published:
#   brew tap Jacke/dora
#   brew install dora

class Dora < Formula
  desc "Beautiful TUI for media downloading (yt-dlp + ffmpeg)"
  homepage "https://github.com/Jacke/doradura"
  version "0.6.0"
  license "MIT"

  on_macos do
    on_arm do
      url "https://github.com/Jacke/doradura/releases/download/tui-v0.6.0/dora-aarch64-apple-darwin.tar.gz"
      sha256 "<sha256-aarch64-apple-darwin>"
    end
    on_intel do
      url "https://github.com/Jacke/doradura/releases/download/tui-v0.6.0/dora-x86_64-apple-darwin.tar.gz"
      sha256 "<sha256-x86_64-apple-darwin>"
    end
  end

  on_linux do
    url "https://github.com/Jacke/doradura/releases/download/tui-v0.6.0/dora-x86_64-unknown-linux-gnu.tar.gz"
    sha256 "<sha256-x86_64-unknown-linux-gnu>"
  end

  depends_on "yt-dlp"
  depends_on "ffmpeg"

  def install
    bin.install "dora"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/dora --version 2>&1", 0)
  end
end
