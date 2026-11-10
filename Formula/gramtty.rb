class Gramtty < Formula
  desc "Terminal Telegram client"
  homepage "https://github.com/methamphetaminelab/gramtty"
  version "1.2.0"
  license "GPL-3.0-or-later"

  on_macos do
    on_arm do
      url "https://github.com/methamphetaminelab/gramtty/releases/download/v#{version}/gramtty-#{version}-aarch64-macos.tar.gz"
      sha256 "d51c153ce5848c8fe1c59d2e2d9e25b8201e394ab50900fb58264bc06c0642a0"
    end
    on_intel do
      url "https://github.com/methamphetaminelab/gramtty/releases/download/v#{version}/gramtty-#{version}-x86_64-macos.tar.gz"
      sha256 "f147edca1393fc278cd6e70e93d369538e1e18aedc0ef776b9064855e411bdea"
    end
  end

  on_linux do
    on_intel do
      url "https://github.com/methamphetaminelab/gramtty/releases/download/v#{version}/gramtty-#{version}-x86_64-linux.tar.gz"
      sha256 "27b68c837fadfed6ff3c3f0cfa36f04c58446bb72db93161a8c963edd7a8e71b"
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
