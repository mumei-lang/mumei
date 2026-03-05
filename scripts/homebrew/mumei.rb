# =============================================================
# Homebrew Formula for Mumei
# =============================================================
# Usage:
#   brew tap mumei-lang/mumei https://github.com/mumei-lang/mumei
#   brew install mumei-lang/mumei/mumei
#
# Or directly:
#   brew install mumei-lang/mumei/mumei
#
# NOTE: This formula is a template. To publish:
#   1. Create a separate repo: mumei-lang/homebrew-mumei
#   2. Copy this file as Formula/mumei.rb
#   3. Update the url/sha256 for each release version
#   4. Users can then: brew tap mumei-lang/mumei && brew install mumei

class Mumei < Formula
  desc "Mathematical Proof-Driven Programming Language — formally verified with Z3"
  homepage "https://github.com/mumei-lang/mumei"
  license "MIT"
  version "0.2.0"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/mumei-lang/mumei/releases/download/v#{version}/mumei-aarch64-apple-darwin.tar.gz"
      # sha256 "UPDATE_WITH_ACTUAL_SHA256"
    else
      url "https://github.com/mumei-lang/mumei/releases/download/v#{version}/mumei-x86_64-apple-darwin.tar.gz"
      # sha256 "UPDATE_WITH_ACTUAL_SHA256"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/mumei-lang/mumei/releases/download/v#{version}/mumei-aarch64-unknown-linux-gnu.tar.gz"
      # sha256 "UPDATE_WITH_ACTUAL_SHA256"
    else
      url "https://github.com/mumei-lang/mumei/releases/download/v#{version}/mumei-x86_64-unknown-linux-gnu.tar.gz"
      # sha256 "UPDATE_WITH_ACTUAL_SHA256"
    end
  end

  depends_on "z3"
  depends_on "llvm@17"

  def install
    bin.install "mumei"
    (share/"mumei/std").install Dir["std/*"]

    # Set MUMEI_STD_PATH so mumei can find the standard library
    env_script = <<~EOS
      export MUMEI_STD_PATH="#{share}/mumei/std"
    EOS
    (etc/"mumei").mkpath
    (etc/"mumei/env.sh").write env_script
  end

  def caveats
    <<~EOS
      The Mumei standard library has been installed to:
        #{share}/mumei/std

      To use it, add the following to your shell profile:
        export MUMEI_STD_PATH="#{share}/mumei/std"

      Or source the environment file:
        source #{etc}/mumei/env.sh
    EOS
  end

  test do
    # Basic smoke test: mumei --version should succeed
    assert_match version.to_s, shell_output("#{bin}/mumei --version")
  end
end
