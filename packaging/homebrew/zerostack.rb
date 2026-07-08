class Zerostack < Formula
  desc "Minimalistic coding agent written in Rust, optimized for memory footprint and performance"
  homepage "https://github.com/gi-dellav/zerostack"
  version "1.6.1"
  license "GPL-3.0-only"

  on_macos do
    if Hardware::CPU.intel?
      url "https://github.com/gi-dellav/zerostack/releases/download/v1.6.1/zerostack-x86_64-apple-darwin.tar.gz"
      sha256 "168d131dfb2ee639c39d14acdfead135e4051f93090bc1a84fdddca87e04b6d1"
    else
      url "https://github.com/gi-dellav/zerostack/releases/download/v1.6.1/zerostack-aarch64-apple-darwin.tar.gz"
      sha256 "67a9ba08ab30ec5ecb819b71380a7fb68108cb2507e28a8a7ed603cb83816216"
    end
  end

  on_linux do
    if Hardware::CPU.intel?
      url "https://github.com/gi-dellav/zerostack/releases/download/v1.6.1/zerostack-x86_64-unknown-linux-musl.tar.gz"
      sha256 "3203352163f7aefd9443a707accba66ec73025181e5e1fe7de2ed8207fe5077d"
    else
      url "https://github.com/gi-dellav/zerostack/releases/download/v1.6.1/zerostack-aarch64-unknown-linux-musl.tar.gz"
      sha256 "add6ada392c3320a43798b197daf734fd2157bf7fd541b9debbaae98491dda98"
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
