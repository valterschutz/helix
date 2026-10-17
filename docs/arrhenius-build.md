# Arrhenius binary builds

Arrhenius uses x86-64 Linux with glibc 2.34, without Nix. Do not copy the
Home Manager executable there: its interpreter and libraries live in `/nix/store`.
The development shell includes `cargo-zigbuild` and `zig` for compatible builds:

```bash
nix develop
HELIX_DISABLE_AUTO_GRAMMAR_BUILD=1 cargo zigbuild --release \
  --target x86_64-unknown-linux-gnu.2.34 -p helix-term
```

The executable is `target/x86_64-unknown-linux-gnu/release/hx` (or under
`CARGO_TARGET_DIR` when set). Keep generated copies outside git, for example
`~/synced/misc/helix-arrhenius/<commit>/hx`.

Install it atomically as `~/programs/helix-fork/hx` on Arrhenius, retaining the
previous executable as a backup and verifying the transferred SHA-256 before
renaming. Copy the matching `runtime/` alongside it. Existing remote grammars
can be retained only when `languages.toml` grammar revisions are unchanged;
do not replace them with potentially incompatible Nix-built shared libraries.

The 2026-10-17 deployment used commit `f6b6add2327c2b85fafa1ddaf3c5507fe6453912`.
Compared with the previous `02a5ebb4` build, grammar revisions were unchanged;
only Python indentation queries changed in the runtime queries. The binary and
non-grammar runtime files were transferred, with the old grammar libraries kept.
The transferred binary's SHA-256 was:

```
1680972bbb41b79a5b47722edea3fd3cc7bc599646546fbedce15f090656ed81
```

`hx --version`, dynamic library resolution, and `hx --health python` were verified
on Arrhenius. This documents deployment tooling only, not an additional deviation
in editor behaviour.
