class Gramtty < Formula
  desc "Terminal Telegram client"
  homepage "https://github.com/methamphetaminelab/gramtty"
  version "1.6.0"
  license "GPL-3.0-or-later"

  on_macos do
    on_arm do
      url "https://github.com/methamphetaminelab/gramtty/releases/download/v#{version}/gramtty-#{version}-aarch64-macos.tar.gz"
      sha256 "37871590d5efe31bb3f41611015ff2231aabb471863fd8cd949dc604c3a8c48f"
    end
    on_intel do
      url "https://github.com/methamphetaminelab/gramtty/releases/download/v#{version}/gramtty-#{version}-x86_64-macos.tar.gz"
      sha256 "b882565490e8d4c8e99ec8fa8423ee7edf2e379aafeaa9743e185ca89547fa33"
    end
  end

  on_linux do
    on_intel do
      url "https://github.com/methamphetaminelab/gramtty/releases/download/v#{version}/gramtty-#{version}-x86_64-linux.tar.gz"
      sha256 "2718a1546fd84414f386b6826d1c710a495ab067c9446318177a25de15fd2ccd"
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
