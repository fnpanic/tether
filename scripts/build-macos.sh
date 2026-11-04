#!/usr/bin/env bash
# Unsigned universal (Apple silicon + Intel) macOS build of Tether and TetherQS. See docs/macos.md.
#
#   TETHER_SERVER=… TETHER_PUB_KEY=… TETHER_ADMIN_PUB_KEY=… TETHER_UPDATER_PUB_KEY=… scripts/build-macos.sh
#
# Optional: OUT (artifact folder, default dist/macos), VCPKG_ROOT (default .tools/vcpkg),
# TAURI_SIGNING_PRIVATE_KEY (+ _PASSWORD) to write .sig files (Tether Manage signs on publish anyway),
# APPLE_SIGNING_IDENTITY (default "-" = ad-hoc), MACOSX_DEPLOYMENT_TARGET (default 11.0),
# MAC_ARCH=arm64 (Apple silicon only: half the build time, no Intel slice, no Rosetta/sudo),
# SKIP_QS=1 (technician app only).
# Safe to run again: every step skips what is already in place.
set -euo pipefail
cd "$(dirname "$0")/.."
if [ "${MAC_ARCH:-universal}" = arm64 ]; then
  TRIPLETS="arm64-osx" RUST_TARGETS="aarch64-apple-darwin" TARGET=aarch64-apple-darwin ARCH_NAME=aarch64
else
  TRIPLETS="arm64-osx x64-osx" RUST_TARGETS="aarch64-apple-darwin x86_64-apple-darwin"
  TARGET=universal-apple-darwin ARCH_NAME=universal
fi
ROOT=$PWD
TOOLS=$ROOT/.tools
OUT=${OUT:-$ROOT/dist/macos}
VCPKG_COMMIT=9e593bb18ea69cc5095e012465dcd675a822ed0d # same as .github/workflows/desktop.yml and vendor/rustdesk/vcpkg.json
NASM_VERSION=2.16.03 # aom/libvpx/ffmpeg need NASM 2.x for x86_64; Homebrew ships 3.x
export MACOSX_DEPLOYMENT_TARGET=${MACOSX_DEPLOYMENT_TARGET:-11.0} # = bundle.macOS.minimumSystemVersion

die() { printf '\nerror: %s\n' "$*" >&2; exit 1; }
step() { printf '\n==> %s\n' "$*"; }

[ "$(uname -s)" = Darwin ] || die "this script builds on macOS only."

# ---------------------------------------------------------------- inputs
missing=()
for v in TETHER_SERVER TETHER_PUB_KEY TETHER_ADMIN_PUB_KEY TETHER_UPDATER_PUB_KEY; do
  [ -n "${!v:-}" ] || missing+=("$v")
done
if [ ${#missing[@]} -gt 0 ]; then
  cat >&2 <<EOF
error: missing ${missing[*]}.
Release builds compile these into the apps. For the production relay:

  export TETHER_SERVER=tether-desk.duckdns.org
  export TETHER_PUB_KEY=fe8A7xTX72RJlJypn8aBxG3NZHHxygCLPQqCWxbeev8=
  export TETHER_ADMIN_PUB_KEY=LdVr9FeDLNbgO6T8PPHddPsicLPr5Hkly1MXqmaLwKI=
  export TETHER_UPDATER_PUB_KEY=dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDQzREZBRjA4Q0Y5RDlERDIKUldUU25aM1BDSy9mUTJqSlBadmQxUWRsc1pxUSthOVUvQTk3M3Z3dzZBV0JnVXFxcThQMXBXeFEK

(other relays: server/install.sh output and Tether Manage -> Keys), then run this script again.
EOF
  exit 1
fi
export TETHER_SERVER TETHER_PUB_KEY TETHER_ADMIN_PUB_KEY TETHER_UPDATER_PUB_KEY

# ---------------------------------------------------------------- prerequisites
step "Xcode Command Line Tools"
if ! xcode-select -p >/dev/null 2>&1 || ! xcrun --find clang >/dev/null 2>&1; then
  xcode-select --install 2>/dev/null || true
  die "the Xcode Command Line Tools are missing. A macOS dialog to install them should have opened (else run: xcode-select --install). Finish it, then run this script again."
fi

step "Homebrew packages"
command -v brew >/dev/null 2>&1 || [ ! -x /opt/homebrew/bin/brew ] || eval "$(/opt/homebrew/bin/brew shellenv)"
command -v brew >/dev/null 2>&1 || [ ! -x /usr/local/bin/brew ] || eval "$(/usr/local/bin/brew shellenv)"
# shellcheck disable=SC2016 # printed literally for the user to paste
command -v brew >/dev/null 2>&1 || die 'Homebrew is missing. Install it from https://brew.sh:
  /bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"
then run this script again.'
# llvm: libclang for bindgen; cmake/ninja/pkg-config: vcpkg ports; node: the UI build.
for f in llvm cmake ninja pkg-config node; do
  brew list --formula "$f" >/dev/null 2>&1 || brew install "$f"
done
LIBCLANG_PATH="$(brew --prefix llvm)/lib"
export LIBCLANG_PATH
node_major=$(node -p 'process.versions.node.split(".")[0]')
[ "$node_major" -ge 22 ] || die "Node 22 or newer is needed (found $(node -v)): brew upgrade node"

step "NASM $NASM_VERSION"
mkdir -p "$TOOLS/bin"
export PATH="$TOOLS/bin:$PATH"
if ! nasm -v 2>/dev/null | grep -q "version 2\."; then
  # Built from source: a native binary on both architectures, no Rosetta needed.
  src=$TOOLS/nasm-$NASM_VERSION
  [ -d "$src" ] || curl -fsSL "https://www.nasm.us/pub/nasm/releasebuilds/$NASM_VERSION/nasm-$NASM_VERSION.tar.xz" | tar -xJ -C "$TOOLS"
  (cd "$src" && ./configure -q && make -j"$(sysctl -n hw.ncpu)" nasm ndisasm >/dev/null)
  cp "$src/nasm" "$src/ndisasm" "$TOOLS/bin/"
fi
nasm -v

step "Rust (aarch64 + x86_64)"
# shellcheck source=/dev/null
if [ -f "$HOME/.cargo/env" ]; then . "$HOME/.cargo/env"; fi
if ! command -v rustup >/dev/null 2>&1; then
  curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal
  # shellcheck source=/dev/null
  . "$HOME/.cargo/env"
fi
rustup target add $RUST_TARGETS

step "vcpkg"
export VCPKG_ROOT=${VCPKG_ROOT:-$TOOLS/vcpkg}
if [ ! -x "$VCPKG_ROOT/vcpkg" ]; then
  [ -d "$VCPKG_ROOT/.git" ] || git clone -q https://github.com/microsoft/vcpkg "$VCPKG_ROOT"
  git -C "$VCPKG_ROOT" checkout -q "$VCPKG_COMMIT"
  "$VCPKG_ROOT/bootstrap-vcpkg.sh" -disableMetrics
fi

# ---------------------------------------------------------------- sources
step "Pinned sources and patches"
bash scripts/bootstrap.sh

step "Native libraries for $TRIPLETS"
# The stock osx triplets plus a deployment target, so the static libs run on every macOS the app
# supports (the stock ones target the build Mac's version).
triplets=$TOOLS/vcpkg-triplets
mkdir -p "$triplets"
for t in arm64:arm64-osx x86_64:x64-osx; do
  arch=${t%%:*} name=${t#*:}
  cat >"$triplets/$name.cmake" <<EOF
set(VCPKG_TARGET_ARCHITECTURE ${arch/x86_64/x64})
set(VCPKG_CRT_LINKAGE dynamic)
set(VCPKG_LIBRARY_LINKAGE static)
set(VCPKG_CMAKE_SYSTEM_NAME Darwin)
set(VCPKG_OSX_ARCHITECTURES $arch)
set(VCPKG_OSX_DEPLOYMENT_TARGET $MACOSX_DEPLOYMENT_TARGET)
EOF
done
# ffmpeg is a *host* dependency in the core's manifest, so vcpkg only builds it for the host
# triplet. Build each triplet as its own host in its own install root, then merge into
# $VCPKG_ROOT/installed/<triplet>, where the core's build scripts look. On Apple silicon the x64
# host tools (pkgconf, …) run under Rosetta.
if [[ $TRIPLETS == *x64-osx* ]] && [ "$(uname -m)" = arm64 ] && ! arch -x86_64 /usr/bin/true 2>/dev/null; then
  sudo /usr/sbin/softwareupdate --install-rosetta --agree-to-license
fi
for t in $TRIPLETS; do
  (cd vendor/rustdesk && "$VCPKG_ROOT/vcpkg" install --triplet "$t" --host-triplet "$t" \
    --overlay-triplets="$triplets" --x-install-root="$VCPKG_ROOT/installed-$t")
  mkdir -p "$VCPKG_ROOT/installed"
  rm -rf "$VCPKG_ROOT/installed/$t"
  cp -R "$VCPKG_ROOT/installed-$t/$t" "$VCPKG_ROOT/installed/$t"
done
for t in $TRIPLETS; do
  for lib in libvpx libyuv libopus libaom libavcodec libavutil; do
    [ -f "$VCPKG_ROOT/installed/$t/lib/$lib.a" ] || die "vcpkg did not produce $t/lib/$lib.a (see $VCPKG_ROOT/buildtrees/*/*.log)"
  done
done

# ---------------------------------------------------------------- build
mkdir -p "$OUT"
export APPLE_SIGNING_IDENTITY=${APPLE_SIGNING_IDENTITY:--} # "-": ad-hoc, required for Apple silicon
version() { node -p "require('./src-tauri/tauri.conf.json').version"; }
sign() { # file: optional .sig next to it, the format Tether Manage also writes
  if [ -n "${TAURI_SIGNING_PRIVATE_KEY:-}" ]; then
    (cd "$ROOT/apps/desktop" && npx tauri signer sign "$1" >/dev/null)
  fi
}

step "Tether technician app ($ARCH_NAME .app, .dmg, updater .app.tar.gz)"
cd "$ROOT/apps/desktop"
npm ci
npx tauri build --target $TARGET --bundles app,dmg
V=$(version)
B="src-tauri/target/$TARGET/release/bundle"
cp "$B/dmg/Tether_${V}_${ARCH_NAME}.dmg" "$OUT/"
# Tauri's updater format: a gzipped tar with the .app at the top.
rm -f "$OUT/Tether_${V}_${ARCH_NAME}.app.tar.gz.sig"
COPYFILE_DISABLE=1 tar -czf "$OUT/Tether_${V}_${ARCH_NAME}.app.tar.gz" -C "$B/macos" Tether.app
sign "$OUT/Tether_${V}_${ARCH_NAME}.app.tar.gz"

if [ -z "${SKIP_QS:-}" ]; then
step "TetherQS (portable $ARCH_NAME .app, zipped)"
cd "$ROOT/apps/quick"
npm ci
# Windows ships a bare exe (bundling off); macOS needs the .app bundle.
# Thin LTO like the technician app: fat LTO fails on macOS ("__CLASS_MudaMenuItem: symbol multiply
# defined", then "failed to load bitcode of module muda"). Windows keeps the Cargo.toml setting.
CARGO_PROFILE_RELEASE_LTO=thin npx tauri build --target $TARGET --bundles app --config '{"bundle":{"active":true}}'
QV=$(version)
rm -f "$OUT/TetherQS-${QV}-${ARCH_NAME}.zip" "$OUT/TetherQS-${QV}-${ARCH_NAME}.zip.sig"
ditto -c -k --sequesterRsrc --keepParent src-tauri/target/$TARGET/release/bundle/macos/TetherQS.app "$OUT/TetherQS-${QV}-${ARCH_NAME}.zip"
sign "$OUT/TetherQS-${QV}-${ARCH_NAME}.zip"
fi

step "Done. Artifacts:"
for f in "$OUT"/Tether_"$V"_${ARCH_NAME}.* "$OUT"/TetherQS-"${QV:-}"-${ARCH_NAME}.*; do
  if [ -e "$f" ]; then echo "  $f"; fi
done
echo
echo "Publish with Tether Manage -> Releases (platform macOS); see docs/macos.md."
