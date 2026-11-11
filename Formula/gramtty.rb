class Gramtty < Formula
  desc "Terminal Telegram client"
  homepage "https://github.com/methamphetaminelab/gramtty"
  version "1.4.0"
  license "GPL-3.0-or-later"

  on_macos do
    on_arm do
      url "https://github.com/methamphetaminelab/gramtty/releases/download/v#{version}/gramtty-#{version}-aarch64-macos.tar.gz"
      sha256 "ccac42db92bd7afd248db287ea9bd2acb17a4b2b9a3c717538efd9178cd561df"
    end
    on_intel do
      url "https://github.com/methamphetaminelab/gramtty/releases/download/v#{version}/gramtty-#{version}-x86_64-macos.tar.gz"
      sha256 "a52ddf85caefbf23a3548dcde97328dfd964fb95bee99184a81b39b574e7bbbf"
    end
  end

  on_linux do
    on_intel do
      url "https://github.com/methamphetaminelab/gramtty/releases/download/v#{version}/gramtty-#{version}-x86_64-linux.tar.gz"
      sha256 "2e96adc295a049e5a9d58d420f10d293a2fe7a8c70f66b6731582e9255a4c268"
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
