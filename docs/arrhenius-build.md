# Arrhenius binary builds

Arrhenius uses x86-64 Linux with glibc 2.34, without Nix. Do not copy the
Home Manager executable there: its interpreter and libraries live in `/nix/store`.
The development shell includes `cargo-zigbuild` and `zig` for compatible builds:

```bash
nix develop -c sh -c 'PATH="$(dirname "$(command -v rustc)"):$PATH" \
  RUSTFLAGS="--cfg tokio_unstable" HELIX_DISABLE_AUTO_GRAMMAR_BUILD=1 \
  cargo zigbuild --release --target x86_64-unknown-linux-gnu.2.34 -p helix-term'
```

Both overrides are needed. The shell's `RUSTFLAGS` adds lld linker arguments
(`--no-rosegment`) that zig rejects, plus `target-cpu=native`, which is not
portable. The shell's `cargo` is wrapped by `cargo-auditable`, whose
`--undefined` linker argument zig also rejects, so the toolchain's unwrapped
`cargo` (next to `rustc`) goes first on `PATH`.

The executable is `target/x86_64-unknown-linux-gnu/release/hx` (or under
`CARGO_TARGET_DIR` when set). Keep generated copies outside git, for example
`~/synced/misc/helix-arrhenius/<commit>/hx`.

Install it atomically as `~/programs/helix-fork/hx` on Arrhenius, retaining the
previous executable as a backup and verifying the transferred SHA-256 before
renaming. Copy the matching `runtime/` alongside it. Existing remote grammars
can be retained only when `languages.toml` grammar revisions are unchanged;
do not replace them with potentially incompatible Nix-built shared libraries.

The latest deployment (2026-10-17) used commit
`05a23302afd440bdd6ce0b7dc922ee8379932a87`. Grammar revisions and `runtime/`
were unchanged since the previous `f6b6add2` build, so only the binary was
transferred; the old one is kept as `hx.backup-f6b6add2`. The transferred
binary's SHA-256 was:

```
afcb237831df10b9a885122834758b82c23028388c5b5c1718a4cc0f71cd4eca
```

`hx --version`, dynamic library resolution, `hx --health rust`, and Enter
between `{}` with auto-pairs disabled were verified on Arrhenius. This documents deployment tooling only, not an additional deviation
in editor behaviour.
