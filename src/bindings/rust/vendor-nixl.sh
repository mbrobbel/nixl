#!/usr/bin/env bash
# SPDX-FileCopyrightText: Copyright (c) 2025-2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
# SPDX-License-Identifier: Apache-2.0
#
# Populate vendor/nixl with the NIXL C++ sources so the published nixl-sys crate
# can build with `--features build-from-source` without a separate checkout
# (build.rs resolves vendor/nixl ahead of the in-tree walk-up).
#
# Run before `cargo package` / `cargo publish`. vendor/ is gitignored and listed
# in Cargo.toml `include`, so it ships in the crate without being committed.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/../../.." && pwd)"
dest="$here/vendor/nixl"

rm -rf "$dest"
mkdir -p "$dest/subprojects"

# Top-level meson inputs.
cp "$repo/meson.build" "$repo/meson_options.txt" "$repo/nixl.pc.in" "$dest/"

# C++ source tree, excluding the Rust crate directory entirely: its nested
# Cargo.toml would make cargo treat it as a separate package and drop the whole
# subtree from `cargo package`. The unconditional nixl_capi target only needs
# rust/wrapper.{cpp,h}, which are copied back in below.
( cd "$repo" && tar --exclude='src/bindings/rust' -cf - src ) | ( cd "$dest" && tar -xf - )
mkdir -p "$dest/src/bindings/rust"
cp "$repo/src/bindings/rust/wrapper.cpp" "$repo/src/bindings/rust/wrapper.h" \
   "$dest/src/bindings/rust/"

# Subproject definitions (wrap files + packagefiles). Extracted subproject
# sources and packagecache are not vendored; an offline build additionally
# needs those dependencies present as system packages (see README).
cp "$repo"/subprojects/*.wrap "$dest/subprojects/" 2>/dev/null || true
[ -d "$repo/subprojects/packagefiles" ] && cp -r "$repo/subprojects/packagefiles" "$dest/subprojects/"

echo "Vendored NIXL sources into $dest ($(du -sh "$dest" | cut -f1))"
