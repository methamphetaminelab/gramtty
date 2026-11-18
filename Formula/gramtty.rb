class Gramtty < Formula
  desc "Terminal Telegram client"
  homepage "https://github.com/methamphetaminelab/gramtty"
  version "1.9.0"
  license "GPL-3.0-or-later"

  on_macos do
    on_arm do
      url "https://github.com/methamphetaminelab/gramtty/releases/download/v#{version}/gramtty-#{version}-aarch64-macos.tar.gz"
      sha256 "ba343dd5b62d8d1b28f58c5a90c36978074e8a17498e4ebc60903e9b3e4711ea"
    end
    on_intel do
      url "https://github.com/methamphetaminelab/gramtty/releases/download/v#{version}/gramtty-#{version}-x86_64-macos.tar.gz"
      sha256 "9620bbabc7a7c2a5ffacc2912801b8b0290041c023e7dd5aad189278f2120fe8"
    end
  end

  on_linux do
    on_intel do
      url "https://github.com/methamphetaminelab/gramtty/releases/download/v#{version}/gramtty-#{version}-x86_64-linux.tar.gz"
      sha256 "c4f89972efc3d0eb4820581a4bdaf1f612c159ac4df83482920f671a82e68c10"
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
