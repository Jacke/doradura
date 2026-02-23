class Dora < Formula
  desc "Beautiful TUI for media downloading (yt-dlp + ffmpeg)"
  homepage "https://github.com/Jacke/doradura"
  version "0.6.0"
  license "MIT"

  on_macos do
    on_arm do
      url "https://github.com/Jacke/doradura/releases/download/tui-v0.6.0/dora-aarch64-apple-darwin.tar.gz"
      sha256 "4449a40385821cc0e4e33d40a3655a10d58d59eace79498fa32269a937b17c63"
    end
    on_intel do
      url "https://github.com/Jacke/doradura/releases/download/tui-v0.6.0/dora-x86_64-apple-darwin.tar.gz"
      sha256 "7221fdcd2b7d2cedc38871b09aa6ea27fe6b7f165e06cbd8555bd5343e94f21b"
    end
  end

  on_linux do
    url "https://github.com/Jacke/doradura/releases/download/tui-v0.6.0/dora-x86_64-unknown-linux-gnu.tar.gz"
    sha256 "7393cf67349610c5b849bf1dad3078250063e6b0514ed64a08bcf6d97b770c08"
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
