class Ultty < Formula
  desc "GPU-accelerated terminal emulator with native TUI protocol"
  homepage "https://github.com/Trav55555/ultty"
  version "VERSION"
  license "MIT OR Apache-2.0"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/Trav55555/ultty/releases/download/v#{version}/ultty-v#{version}-aarch64-apple-darwin.tar.gz"
      sha256 "SHA256_ARM64"
    else
      url "https://github.com/Trav55555/ultty/releases/download/v#{version}/ultty-v#{version}-x86_64-apple-darwin.tar.gz"
      sha256 "SHA256_X86_64"
    end
  end

  on_linux do
    url "https://github.com/Trav55555/ultty/releases/download/v#{version}/ultty-v#{version}-x86_64-unknown-linux-gnu.tar.gz"
    sha256 "SHA256_LINUX"

    depends_on "wayland"
    depends_on "libxkbcommon"
  end

  def install
    bin.install "ultty"
    bin.install "ulttyctl"
    bin.install "ultty-session"

    (share/"terminfo").mkpath
    system "tic", "-x", "-o", share/"terminfo", "dist/terminfo/ultty.terminfo"
    (share/"ultty").install "dist/shell-integration"

    if OS.linux?
      (share/"applications").install "dist/ultty.desktop"
      (share/"icons").install "dist/icons/hicolor"
    end
  end

  test do
    assert_match "ultty", shell_output("#{bin}/ultty --version")
    assert_match "ulttyctl", shell_output("#{bin}/ulttyctl --version")
    assert_match "ultty-session", shell_output("#{bin}/ultty-session --version")
  end
end
