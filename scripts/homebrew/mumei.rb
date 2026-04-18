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
# NOTE: This formula is a template. The release workflow automatically
# generates the actual Formula with correct sha256 values and creates
# a PR to mumei-lang/homebrew-mumei.
#
# To set up the Homebrew Tap:
#   1. Create a new repo: mumei-lang/homebrew-mumei
#   2. Create a GitHub secret HOMEBREW_TAP_TOKEN with repo write access
#   3. The release.yml workflow will auto-create PRs to update the Formula
#   4. Users can then: brew install mumei-lang/mumei/mumei

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

    # SI-5 Phase 3-C: install std/ proof-certificate bundle when present
    has_proof_bundle = File.exist?("std-proof-bundle.json")
    if has_proof_bundle
      (share/"mumei").install "std-proof-bundle.json"
    end

    # Set MUMEI_STD_PATH so mumei can find the standard library.
    # Only export MUMEI_PROOF_BUNDLE when the bundle was actually installed
    # (musl / Alpine targets don't ship one).
    env_lines = ["export MUMEI_STD_PATH=\"#{share}/mumei/std\""]
    if has_proof_bundle
      env_lines << "export MUMEI_PROOF_BUNDLE=\"#{share}/mumei/std-proof-bundle.json\""
    end
    env_script = env_lines.join("\n") + "\n"
    (etc/"mumei").mkpath
    (etc/"mumei/env.sh").write env_script
  end

  def caveats
    s = <<~EOS
      The Mumei standard library has been installed to:
        #{share}/mumei/std

      To use it, add the following to your shell profile:
        export MUMEI_STD_PATH="#{share}/mumei/std"

      Or source the environment file:
        source #{etc}/mumei/env.sh
    EOS
    if File.exist?("#{share}/mumei/std-proof-bundle.json")
      s += <<~EOS

        The std/ proof-certificate bundle (SI-5 Phase 3-C) is at:
          #{share}/mumei/std-proof-bundle.json
      EOS
    end
    s
  end

  test do
    # Basic smoke test: mumei --version should succeed
    assert_match version.to_s, shell_output("#{bin}/mumei --version")
  end
end
