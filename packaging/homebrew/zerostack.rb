class Zerostack < Formula
  desc "Minimalistic coding agent written in Rust, optimized for memory footprint and performance"
  homepage "https://github.com/gi-dellav/zerostack"
  version "1.5.0"
  license "GPL-3.0-only"

  on_macos do
    if Hardware::CPU.intel?
      url "https://github.com/gi-dellav/zerostack/releases/download/v1.5.0/zerostack-x86_64-apple-darwin.tar.gz"
      sha256 "0c5ce2d6cc251bb6dd782f250a321bbb13084d0e88a9536d9d86d7b04a5779e0"
    else
      url "https://github.com/gi-dellav/zerostack/releases/download/v1.5.0/zerostack-aarch64-apple-darwin.tar.gz"
      sha256 "100c1a7182343d916e126b342e3cc32bf12bf44e0fa17e392d441171b31c3ebc"
    end
  end

  on_linux do
    if Hardware::CPU.intel?
      url "https://github.com/gi-dellav/zerostack/releases/download/v1.5.0/zerostack-x86_64-unknown-linux-musl.tar.gz"
      sha256 "b8b35c4afdc5866ec137e1c15d3dfe47382bc05be17c1360fff1c0382a912aff"
    else
      url "https://github.com/gi-dellav/zerostack/releases/download/v1.5.0/zerostack-aarch64-unknown-linux-musl.tar.gz"
      sha256 "0d81ceef899f2a8be800857dca6165fbf3bee682ba12f0f9ccd291abaca7ec0c"
    end
  end

  def install
    # darwin tarballs contain "zerostack", musl tarballs contain "zerostack-<target>"
    bin.install Dir["zerostack*"].first => "zerostack"
  end

  test do
    assert_match(/^zerostack /, shell_output("#{bin}/zerostack --version"))
  end
end
