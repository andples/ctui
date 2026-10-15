#!/usr/bin/env bash
# Bump ctui's version, tag + release it on GitHub, wait for the release
# workflow to attach prebuilt binaries, and point the homebrew-ctui tap
# formula at them.
#
# Usage:
#   scripts/release.sh patch   # 0.1.0 -> 0.1.1
#   scripts/release.sh minor   # 0.1.0 -> 0.2.0
#   scripts/release.sh major   # 0.1.0 -> 1.0.0
#
# Requires: cargo, git, gh (authenticated).

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TAP_DIR="${TAP_DIR:-$ROOT/../homebrew-ctui}"
CARGO_TOML="$ROOT/Cargo.toml"
FORMULA="$TAP_DIR/Formula/ctui.rb"
GITHUB_REPO="andples/ctui"

usage() {
    echo "Usage: $0 <patch|minor|major> [release notes]" >&2
    echo "  patch   bump X.Y.Z -> X.Y.(Z+1)" >&2
    echo "  minor   bump X.Y.Z -> X.(Y+1).0   (the \".1\" bump)" >&2
    echo "  major   bump X.Y.Z -> (X+1).0.0   (the \"1.0\" bump)" >&2
    echo "  [release notes]  optional text used as the GitHub release body;" >&2
    echo "                   omit it to auto-generate notes from commits" >&2
    exit 1
}

[ $# -ge 1 ] && [ $# -le 2 ] || usage
BUMP="$1"
NOTES="${2:-}"
case "$BUMP" in
    patch|minor|major) ;;
    *) usage ;;
esac

command -v gh >/dev/null || { echo "error: gh CLI is required" >&2; exit 1; }

if [ -n "$(git -C "$ROOT" status --porcelain)" ]; then
    echo "error: $ROOT has uncommitted changes, aborting" >&2
    exit 1
fi
if [ ! -d "$TAP_DIR" ]; then
    echo "error: tap repo not found at $TAP_DIR (set TAP_DIR to override)" >&2
    exit 1
fi
if [ -n "$(git -C "$TAP_DIR" status --porcelain)" ]; then
    echo "error: $TAP_DIR has uncommitted changes, aborting" >&2
    exit 1
fi

check_up_to_date() {
    local dir="$1" label="$2"
    git -C "$dir" fetch origin -q
    local branch behind
    branch="$(git -C "$dir" rev-parse --abbrev-ref HEAD)"
    behind="$(git -C "$dir" rev-list --count "HEAD..origin/$branch" 2>/dev/null || echo 0)"
    if [ "$behind" != "0" ]; then
        echo "error: $label ($dir) is $behind commit(s) behind origin/$branch" >&2
        echo "       run: git -C \"$dir\" pull --rebase origin $branch" >&2
        exit 1
    fi
}

echo "==> checking $ROOT and $TAP_DIR are up to date with origin"
check_up_to_date "$ROOT" "ctui"
check_up_to_date "$TAP_DIR" "homebrew-ctui"

CURRENT_VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' "$CARGO_TOML" | head -1)"
[ -n "$CURRENT_VERSION" ] || { echo "error: could not read version from $CARGO_TOML" >&2; exit 1; }

IFS='.' read -r MAJOR MINOR PATCH <<< "$CURRENT_VERSION"
if [ "$BUMP" = "patch" ]; then
    NEW_VERSION="$MAJOR.$MINOR.$((PATCH + 1))"
elif [ "$BUMP" = "minor" ]; then
    NEW_VERSION="$MAJOR.$((MINOR + 1)).0"
else
    NEW_VERSION="$((MAJOR + 1)).0.0"
fi
TAG="v$NEW_VERSION"

echo "About to release:"
echo "  repo:        $GITHUB_REPO"
echo "  version:     $CURRENT_VERSION -> $NEW_VERSION"
echo "  tag:         $TAG"
echo "  tap dir:     $TAP_DIR"
if [ -n "$NOTES" ]; then
    echo "  notes:       $NOTES"
else
    echo "  notes:       (auto-generated from commits)"
fi
read -r -p "Proceed? [y/N] " CONFIRM
case "$CONFIRM" in
    y|Y|yes|YES) ;;
    *) echo "aborted" >&2; exit 1 ;;
esac

echo "==> updating Cargo.toml"
sed -i "0,/^version = \".*\"/s//version = \"$NEW_VERSION\"/" "$CARGO_TOML"

echo "==> updating Cargo.lock"
(cd "$ROOT" && cargo check -q)

echo "==> running tests"
(cd "$ROOT" && cargo test -q)

echo "==> committing version bump"
git -C "$ROOT" add Cargo.toml Cargo.lock
git -C "$ROOT" commit -m "Bump version to $NEW_VERSION"

echo "==> tagging $TAG"
git -C "$ROOT" tag -a "$TAG" -m "$TAG"

echo "==> pushing branch and tag"
git -C "$ROOT" push origin HEAD
git -C "$ROOT" push origin "$TAG"

echo "==> creating GitHub release"
if [ -n "$NOTES" ]; then
    gh release create "$TAG" \
        --repo "$GITHUB_REPO" \
        --title "$TAG" \
        --notes "$NOTES"
else
    gh release create "$TAG" \
        --repo "$GITHUB_REPO" \
        --title "$TAG" \
        --generate-notes
fi

# The tag push started .github/workflows/release.yml; wait for it to
# attach a binary per target.
TARGETS="aarch64-apple-darwin x86_64-apple-darwin aarch64-unknown-linux-musl x86_64-unknown-linux-musl"
echo "==> waiting for the release workflow to build binaries"
RUN_ID=""
for _ in $(seq 30); do
    RUN_ID="$(gh run list --repo "$GITHUB_REPO" --workflow release.yml --commit "$(git -C "$ROOT" rev-parse "$TAG^{commit}")" \
        --json databaseId -q '.[0].databaseId' 2>/dev/null || true)"
    [ -n "$RUN_ID" ] && break
    sleep 5
done
[ -n "$RUN_ID" ] || { echo "error: no release workflow run for $TAG" >&2; exit 1; }
gh run watch "$RUN_ID" --repo "$GITHUB_REPO" --exit-status >/dev/null || {
    echo "error: release workflow failed: https://github.com/$GITHUB_REPO/actions/runs/$RUN_ID" >&2
    exit 1
}

echo "==> reading checksums"
SUMS_DIR="$(mktemp -d)"
trap 'rm -rf "$SUMS_DIR"' EXIT
gh release download "$TAG" --repo "$GITHUB_REPO" --pattern '*.sha256' --dir "$SUMS_DIR"
sha_for() {
    local f="$SUMS_DIR/ctui-$1.tar.gz.sha256"
    [ -s "$f" ] || { echo "error: missing checksum for $1" >&2; exit 1; }
    cut -d' ' -f1 "$f"
}
for t in $TARGETS; do
    echo "    $t: $(sha_for "$t")"
done

BASE="https://github.com/$GITHUB_REPO/releases/download/$TAG"
echo "==> writing formula at $FORMULA"
cat > "$FORMULA" <<FORMULA_EOF
class Ctui < Formula
  desc "Minimal terminal calendar and task list (TUI and CLI)"
  homepage "https://github.com/$GITHUB_REPO"
  version "$NEW_VERSION"
  license "MIT"

  # Prebuilt by .github/workflows/release.yml in $GITHUB_REPO; written by
  # scripts/release.sh, don't edit by hand.
  on_macos do
    on_arm do
      url "$BASE/ctui-aarch64-apple-darwin.tar.gz"
      sha256 "$(sha_for aarch64-apple-darwin)"
    end
    on_intel do
      url "$BASE/ctui-x86_64-apple-darwin.tar.gz"
      sha256 "$(sha_for x86_64-apple-darwin)"
    end
  end

  on_linux do
    on_arm do
      url "$BASE/ctui-aarch64-unknown-linux-musl.tar.gz"
      sha256 "$(sha_for aarch64-unknown-linux-musl)"
    end
    on_intel do
      url "$BASE/ctui-x86_64-unknown-linux-musl.tar.gz"
      sha256 "$(sha_for x86_64-unknown-linux-musl)"
    end
  end

  head do
    url "https://github.com/$GITHUB_REPO.git", branch: "master"
    depends_on "rust" => :build
  end

  def install
    if build.head?
      system "cargo", "install", *std_cargo_args
    else
      bin.install "ctui"
    end
    # The Monkeytype themes (GPL-3.0, separate files); ctui finds them in
    # share/ctui/themes next to its own prefix.
    pkgshare.install "themes"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/ctui --version")
  end
end
FORMULA_EOF

echo "==> committing and pushing tap update"
git -C "$TAP_DIR" add Formula/ctui.rb
git -C "$TAP_DIR" commit -m "ctui $NEW_VERSION"
check_up_to_date "$TAP_DIR" "homebrew-ctui"
git -C "$TAP_DIR" push origin HEAD

echo "==> done: $TAG released and homebrew-ctui updated"
echo "    crates.io is separate: cargo publish (needs the owner's go-ahead)"
