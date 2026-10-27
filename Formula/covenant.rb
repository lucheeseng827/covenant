# Generated for each release by .github/workflows/release.yml: do not edit it
# by hand. The next release writes it again.
class Covenant < Formula
  desc "Data-contract enforcement runtime: one contract, run at every boundary"
  homepage "https://github.com/lucheeseng827/covenant"
  version "0.2.1"
  license "Apache-2.0"

  on_macos do
    on_arm do
      url "https://github.com/lucheeseng827/covenant/releases/download/v0.2.1/covenant-aarch64-apple-darwin.tar.gz"
      sha256 "0ed1b646cb82a1741cc9af625dfbb747a59629c342c979ef487b7b7abfb3b74c"
    end
    on_intel do
      url "https://github.com/lucheeseng827/covenant/releases/download/v0.2.1/covenant-x86_64-apple-darwin.tar.gz"
      sha256 "30d1a2d38cd23547f7a6f19964938b95ae8ed45ad91e4bd7a3db7b63237bac43"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/lucheeseng827/covenant/releases/download/v0.2.1/covenant-aarch64-unknown-linux-musl.tar.gz"
      sha256 "5a86bdbf37ee861c99a00acb37dc0c27ce0879b4f6471878594fa44680846321"
    end
    on_intel do
      url "https://github.com/lucheeseng827/covenant/releases/download/v0.2.1/covenant-x86_64-unknown-linux-musl.tar.gz"
      sha256 "44035e78a8b5956e19eec3041e0f7f1f758e7ccb733d8bac00a9fe8c9fc41a58"
    end
  end

  def install
    bin.install "covenant"
  end

  test do
    assert_equal "covenant #{version}", shell_output("#{bin}/covenant --version").strip
    system bin/"covenant", "init", testpath/"orders.yaml"
    system bin/"covenant", "validate", testpath/"orders.yaml"
  end
end
