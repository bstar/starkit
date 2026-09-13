# Contributing

This is the library half of two applications, `staramp` and `starcord`. It has
no users of its own, so the most useful thing to do before writing code is to
say which of those two needs the change and what it looks like at the call
site.

## Getting it to build

The one-command path:

```sh
nix develop
cargo test --all-features
```

Without Nix you need a Rust toolchain, 1.90 or newer, and nothing else. There
is no C in this tree: no `pkg-config`, no `bindgen`, no system libraries. That
is worth keeping — the applications have those problems already and this crate
should not add to them.

## What CI will run

```sh
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings -A dead_code
cargo test --all-features
cargo test --no-default-features
cargo deny check
```

Both ends of the feature matrix, every time. A feature that only one
application turns on is exactly the one that breaks unnoticed, and
`--no-default-features` is the build that links no image decoder at all.

`dead_code` is allowed because a module usually lands here complete and tested
one work package before the application that will call it. Every other lint is
an error.

## Features

`default = ["image"]`, then `gif`, `wsz` and `net`. A feature exists to keep a
dependency out of an application that has no use for it, not to make the crate
configurable. Before adding one, check that something real is excluded by it:
a feature that gates two hundred bytes of code and no dependency is a way to
break the build on a machine nobody tests on.

Anything behind a feature still has to compile and test with it on, which is
what the `--all-features` runs above are for.

## Changing a signature

Two consumers, both checked out beside this repository, and no third. Read both
before changing a shared item — `grep -rn "starkit::" ../staramp/src
../starcord/src` — and land the consumer fix in the same window as the
release. `AGENTS.md` has the `[patch]` recipe for trying a change from an
application before it is tagged.

## Versioning

Semver on 0.x: an API change is a minor bump, a fix is a patch. Tag `vX.Y.Z`.
A breaking change carries its `CHANGELOG.md` entry under **Changed** and says
what the call site has to do, because the person reading it is the same person
who wrote the call site and they have forgotten.

## Tests

In-module `#[cfg(test)]`, beside what they test. Proptest for anything that
parses input somebody else wrote — a config file, a theme, a skin, a byte off
the wire — because a hand-written case only covers the malformed input its
author thought of.

## Commit messages

Present tense, plain prose, no conventional-commits prefix. What the commit
does and, where it is not obvious, why. The existing log is the style guide.

## Comments

The code explains decisions rather than mechanics, and usually says what was
measured. If you change something a comment justifies, change the comment. If
you move code a comment justifies, move the comment with it. If you leave a
comment saying something is a certain way for a reason, make sure the reason is
true.
