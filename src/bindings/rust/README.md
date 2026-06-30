# Rust Bindings for NIXL

Rust bindings for the NVIDIA Inference Xfer Library (NIXL). These bindings provide a safe and idiomatic Rust interface to the NIXL C++ library.

## Prerequisites

### Install Rust and Cargo using [rustup](https://rustup.rs/):

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

### Install the NIXL library on your system

Refer to the [NIXL README](https://github.com/ai-dynamo/nixl/blob/main/README.md) for instructions on how to install the NIXL library on your system.



## Building

The bindings can be built using Cargo, Rust's package manager:

```bash
# From the src/bindings/rust directory
cargo build
```

### Building with Stubs (No NIXL Library Required)

You can compile the bindings using stub implementations that don't require the actual NIXL library to be installed. This is useful for:
- Development environments where NIXL isn't available
- CI/CD pipelines
- Building documentation

```bash
# Build with stub API
cargo build --features stub-api
```

**Important**: When using stubs, any attempt to actually call NIXL functions at runtime will print an error message and abort the program.
- The stubs are only meant for compilation, not execution.

### Building from Source (No Pre-installed NIXL Required)

The `build-from-source` feature builds the NIXL C++ libraries with meson into
`OUT_DIR` and links against them, so no pre-installed NIXL or `NIXL_PREFIX` is
needed:

```bash
# In-tree, or with NIXL_SOURCE_DIR pointing at a NIXL checkout:
cargo build --features build-from-source
```

The source tree is located, in priority order, from `NIXL_SOURCE_DIR`, then the
enclosing NIXL checkout (when built in-tree), then sources bundled inside the
crate at `vendor/nixl`. The crate published to crates.io bundles the NIXL C++
sources (see "Packaging" below), so a standalone consumer can build from source
without a checkout.

Source *acquisition* itself performs no network access (no git clone), but the
meson build resolves the NIXL C++ dependencies (Abseil, liburing, Taskflow, …)
and, when they are not present as system packages, downloads them as meson
subprojects — so a fully offline build additionally requires those dependencies
to be installed system-wide.

Build-tool requirements: `meson`, `ninja`, a C++20 compiler, the CUDA toolkit,
UCX (for the UCX plugin), and `libclang` (for bindgen). Python/`pybind11` is
**not** required — the from-source build passes `-Dbuild_python=false`.

#### Packaging

The C++ sources are not committed under the crate; `vendor-nixl.sh` copies them
into `vendor/nixl` (gitignored, listed in `Cargo.toml` `include`). Run it before
`cargo package` / `cargo publish` so the published crate carries the sources.

#### Runtime library path for downstream binaries

The libraries are built into the crate's `OUT_DIR`. An rpath is emitted so
`nixl-sys`' own tests and binaries run directly, but `rustc-link-arg` does not
propagate to a **downstream** executable. The build script therefore exports the
install location as `links` metadata; a downstream `build.rs` can apply its own
rpath:

```rust
// downstream build.rs
if let Ok(dir) = std::env::var("DEP_NIXL_LIB_DIR") {
    println!("cargo:rustc-link-arg=-Wl,-rpath,{dir}");
}
```

Without that, set `LD_LIBRARY_PATH` to the built `lib`/`lib64` dir at runtime.

At the meson level, NIXL can now also be built fully static: `nixl`,
`nixl_build`, and `nixl_common` are all `library()` targets, so a
`--default-library=static` build (with `-Dstatic_plugins=<plugins>` to build
backends in) produces static archives. `nixl-sys`' `build-from-source` path
currently links dynamically and does not yet drive a static install.

If both `build-from-source` and `stub-api` are enabled, `build-from-source`
takes precedence.

### Environment Variables

- `NIXL_PREFIX`: Path to the NIXL installation (default: `/opt/nvidia/nvda_nixl`)
- `NIXL_SOURCE_DIR`: Path to a NIXL source checkout for `build-from-source`
- `NIXL_PLUGINS`: Comma-separated plugins to build (`-Denable_plugins`); default builds all
- `NIXL_UCX_PATH`, `NIXL_CUDA_INC_PATH`, `NIXL_CUDA_LIB_PATH`: optional UCX/CUDA paths for `build-from-source`

## Documentation

The crate is documented using Rust's standard documentation system. You can generate and view the documentation with:

```bash
cargo doc
```


## Testing

The bindings include a comprehensive test suite that can be run with:
Note that multithreading is disabled because NIXL might deadlock.

```bash
cargo test -- --test-threads=1
```

**Note**: Tests cannot be run with the `stub-api` feature as they require actual NIXL functionality.