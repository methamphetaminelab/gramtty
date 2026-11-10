class Gramtty < Formula
  desc "Terminal Telegram client"
  homepage "https://github.com/methamphetaminelab/gramtty"
  version "1.1.0"
  license "GPL-3.0-or-later"

  on_macos do
    on_arm do
      url "https://github.com/methamphetaminelab/gramtty/releases/download/v#{version}/gramtty-#{version}-aarch64-macos.tar.gz"
      sha256 "abe6ea86f9d09ea427f23d912e216c110a88645bed1b49510e1bba0a1c834bb5"
    end
    on_intel do
      url "https://github.com/methamphetaminelab/gramtty/releases/download/v#{version}/gramtty-#{version}-x86_64-macos.tar.gz"
      sha256 "bed4d8e78e47585713ae7d6ea31c3cf0ac2ec48bd90aac0a3373d6954d028785"
    end
  end

  on_linux do
    on_intel do
      url "https://github.com/methamphetaminelab/gramtty/releases/download/v#{version}/gramtty-#{version}-x86_64-linux.tar.gz"
      sha256 "9b4f50d7cca6d895a3458a28af50b8e04e45d7d79edcee82cb5bf7abd8e00a2e"
    end
  end

  def install
    bin.install "gramtty"
  end

  def caveats
    <<~EOS
      Optional runtime tools, looked up on PATH:
        ffmpeg  video and sticker previews, inline animations, upload thumbnails
        mpv     media playback (libmpv for the in-viewer player)
      Install them with: brew install ffmpeg mpv
    EOS
  end

  test do
    assert_match "usage:", shell_output("#{bin}/gramtty --brew-test 2>&1", 1)
  end
end
