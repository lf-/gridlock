# gridlock

gridlock is a lockfile manager similar to [Niv] and [Nix flakes], but with *no*
dependency on Nix. It provides Nix compatible hashes, but is not inherently
tied to Nix, and is suitable for just keeping track of git hashes if that's
your jam.

[Niv]: https://github.com/nmattia/niv
[Nix flakes]: https://nixos.org/manual/nix/stable/command-ref/new-cli/nix3-flake.html

## Why?

The purpose of `gridlock` is to enable projects to provide a good experience
for Nix users by offering lock files with Nix compatible structural hashes of
the *inside of* archives (thereby being able to use non-IFD fetching) without
requiring all maintainers to have Nix installed.

Nix has two kinds of fetchers for files, one of which is the builtin
`builtins.fetchTree`, `builtins.fetchTarball`, etc, and the other is
`nixpkgs.fetchzip` and friends. The difference is that the builtin fetchers are
instances of [import-from-derivation (IFD)][ifd], whereas the ones in nixpkgs
are fixed-output derivations. The fixed-output derivations are greatly
preferable since they can be parallelized (the fact that the builtins cannot is
a Nix limitation) but they *require* a known hash ahead of time.

[ifd]: https://nixos.wiki/wiki/Import_From_Derivation

## Usage

> **NOTE**: `gridlock` is currently alpha quality software, and is currently
> missing some rather important planned features such as updating a full
> lockfile in one shot, or progrss bars.

```
 » gridlock --lockfile gridlock.json init
 » gridlock --lockfile gridlock.json add tree-sitter/tree-sitter-rust
Adding tree-sitter/tree-sitter-rust at master: f7fb205c424b0962de59b26b931fe484e1262b35
» gridlock --lockfile gridlock.json show
tree-sitter-rust
  Branch: master
  Rev: f7fb205c424b0962de59b26b931fe484e1262b35
  Last updated: 2023-02-16 21:38:35
  Web link: https://github.com/tree-sitter/tree-sitter-rust/tree/f7fb205c424b0962de59b26b931fe484e1262b35
```

## Implementation

I simply reimplemented(tm) the NAR file format, and then generate NAR files
in-memory, then hash them. The efficiency of the NAR encoder is undoubtedly not
the best, but it is currently Ok. Also, on systems with Nix, the archive will
need to be downloaded twice, since it is not imported into the Nix store by
gridlock (FIXME: possibly add this as a feature?).

Another creative design choice in gridlock compared to Niv and Nix flakes is
that gridlock uses `git` to interact with GitHub, and does *not* touch the
GitHub API, due to the very low unauthenticated rate limits of the GitHub API.
I suspect that the `git` protocol operations used by `gridlock` are not rate
limited in the same way.
