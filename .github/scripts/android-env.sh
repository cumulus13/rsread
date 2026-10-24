#!/usr/bin/env bash
# Configure cargo to cross-compile for Android with the NDK preinstalled on GitHub runners.
# Usage: android-env.sh <rust-target>   (exports CC/AR/linker through $GITHUB_ENV)
set -euo pipefail

target="${1:?usage: android-env.sh <rust-target>}"
api="${ANDROID_API:-24}" # Termux requires API 24+

ndk="${ANDROID_NDK_LATEST_HOME:-${ANDROID_NDK_HOME:-${ANDROID_NDK_ROOT:-}}}"
if [ -z "$ndk" ] || [ ! -d "$ndk" ]; then
  echo "::error::Android NDK not found (ANDROID_NDK_LATEST_HOME / ANDROID_NDK_HOME unset)"
  exit 1
fi

bin="$ndk/toolchains/llvm/prebuilt/linux-x86_64/bin"
case "$target" in
  armv7-linux-androideabi) prefix="armv7a-linux-androideabi" ;;
  *) prefix="$target" ;;
esac

clang="$bin/${prefix}${api}-clang"
if [ ! -x "$clang" ]; then
  echo "::error::NDK clang not found: $clang"
  exit 1
fi

upper="$(printf '%s' "$target" | tr 'a-z-' 'A-Z_')"
lower="${target//-/_}"

{
  echo "CC_${lower}=${clang}"
  echo "CXX_${lower}=${clang}++"
  echo "AR_${lower}=${bin}/llvm-ar"
  echo "CARGO_TARGET_${upper}_LINKER=${clang}"
} >> "${GITHUB_ENV:-/dev/stdout}"

echo "NDK: $ndk (API $api) -> $clang"
