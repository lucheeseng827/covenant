#!/usr/bin/env bash
# Write the Homebrew formula for one release to stdout, as the release workflow
# does before it tests and commits it as Formula/covenant.rb:
#
#   formula.sh <version> <download URL> <dir holding the archives>
#
# The formula installs the release's own binaries: macOS arm64 and x86_64, and
# Linux arm64 and x86_64 (static, musl). Each is pinned to the SHA-256 of its
# archive (package.sh names them) in <dir>. The download URL is the release's
# (https://github.com/<owner>/<repo>/releases/download/v<version>), or a
# file:// URL of <dir> to install a build that is not released.
set -euo pipefail

die() {
    echo "formula.sh: $*" >&2
    exit 2
}

[ $# -eq 3 ] || die "usage: formula.sh <version> <download URL> <dir>"
version=$1
url=${2%/}
dir=$3

# The versions the release workflow's plan accepts, and no other.
[[ $version =~ ^[0-9]+\.[0-9]+\.[0-9]+([-+][0-9A-Za-z.+-]+)?$ ]] || die "not a version: $version"
# The URL goes into Ruby strings: nothing in it may end one or interpolate.
[[ $url =~ ^(https|file)://[A-Za-z0-9._~/%:+-]+$ ]] || die "not a download URL: $url"

sha256() {
    local archive="$dir/covenant-$1.tar.gz"
    [ -f "$archive" ] || die "no archive at $archive"
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$archive" | cut -d' ' -f1
    else
        shasum -a 256 "$archive" | cut -d' ' -f1
    fi
}

mac_arm=$(sha256 aarch64-apple-darwin)
mac_intel=$(sha256 x86_64-apple-darwin)
linux_arm=$(sha256 aarch64-unknown-linux-musl)
linux_intel=$(sha256 x86_64-unknown-linux-musl)

cat <<EOF
# Generated for each release by .github/workflows/release.yml: do not edit it
# by hand. The next release writes it again.
class Covenant < Formula
  desc "Data-contract enforcement runtime: one contract, run at every boundary"
  homepage "https://github.com/lucheeseng827/covenant"
  version "$version"
  license "Apache-2.0"

  on_macos do
    on_arm do
      url "$url/covenant-aarch64-apple-darwin.tar.gz"
      sha256 "$mac_arm"
    end
    on_intel do
      url "$url/covenant-x86_64-apple-darwin.tar.gz"
      sha256 "$mac_intel"
    end
  end

  on_linux do
    on_arm do
      url "$url/covenant-aarch64-unknown-linux-musl.tar.gz"
      sha256 "$linux_arm"
    end
    on_intel do
      url "$url/covenant-x86_64-unknown-linux-musl.tar.gz"
      sha256 "$linux_intel"
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
EOF
