# Working notes

Context that is not derivable from the code or the history. It lives in the
repository rather than in any one machine's notes because this is developed on
both Linux and macOS and the repository is the only thing every session sees.

This is the only notes file of its kind here. Do not add a second one beside
it under another name; everything that reads such a file reads `AGENTS.md`.

## Extraction in progress

This crate is being lifted out of STAR/AMP a module at a time, in an order that
leaves STAR/AMP compiling and passing its tests after every step. The leaf
modules, the directory rule, file logging and the whole theme engine have
landed, and STAR/AMP now runs through all of them; the terminal graphics, the
docked chrome, the keymap and the layout engine have not. Two of the rules
below -- probing before raw mode, and the `[patch]` recipe -- describe
machinery that arrives with the graphics module and with the move from a path
dependency to a tag. They are written down now because they are the reasons
those pieces are shaped the way they are, not because they are already load
bearing. `CHANGELOG.md` says what is actually here.

## Two consumers, both of them known

Every public item in this crate exists because STAR/AMP or STAR/CORD needed
it, and most of them are called from both. There is no third caller and no
published API to keep compatible with a stranger, which is the one real
advantage this arrangement has: **before changing a signature, read both call
sites.** They are checked out beside this repository:

```sh
grep -rn "starkit::" ../staramp/src ../starcord/src
```

A change that is awkward at one of them and impossible at the other is not a
refactor, it is a new function. Adding one is cheaper than bending a shared
one until neither application likes it.

This is also why the crate is not documented as a general framework. Its
README says so out loud, and a feature request with no consumer in either
application is a fork, not an issue.

## Working on it from an application

Once the extraction is finished both applications will depend on a tag rather
than on this directory, and a local change will not be visible to them until it
is released. To try one before it is:

```toml
# ../staramp/.cargo/config.toml -- never committed
[patch."https://github.com/bstar/starkit"]
starkit = { path = "../starkit" }
```

`.cargo/config.toml` is deliberately not in either application's `.gitignore`
as an invitation: it is untracked, it is per-checkout, and a committed one
would point CI at a path that does not exist. Delete it when the change is
tagged and the application has moved to the new tag.

## Probe before raw mode

`Graphics::probe` writes capability queries to the terminal and reads the
replies back off stdin. It has to run **before** `term::init` enables raw mode
and the alternate screen, and the late replies that arrive after it returns are
discarded by `drain_stdin`. Probing afterwards reads the user's keystrokes as
capability replies and reports whatever they typed as the terminal's answer.

The ordering is not expressible in the type system across a crate boundary, so
`term::init` records that it ran and `probe` debug-asserts against it: a debug
build panics, and a release build degrades to `Graphics::disabled()` with a
warning rather than corrupting the session.

## Themes are shared assets

The sixteen theme files here are read by both applications. The core resolver
deserialises the tables it knows -- `meta`, `base16`, `app`, `chrome`, `panel`,
`row`, `status` -- and keeps everything else in `ThemeFile::extra`, so a file
may carry `[vis]` for STAR/AMP's analyzer and `[chat]` for STAR/CORD's message
list at the same time and neither one sees the other's table as an error.

That is a property to preserve. A resolver that rejects unknown tables, or a
schema that flattens the app tables into the core struct, makes every theme
file the property of one application.

Resolution is pinned at both ends: `testdata/golden/` here, and STAR/AMP's own
`testdata/theme-golden/`, record what each built-in resolves to role by role. A
derivation change that is deliberate is a diff of colours in those directories.
One that is not is a failing test.

## `-A dead_code`, and why

CI runs `cargo clippy --all-targets --all-features -- -D warnings -A
dead_code`. `dead_code` is the one lint that is allowed, because a library
module lands here complete and tested one work package before the application
that will call it, and `--all-features` means the build always contains code
that only one of the two consumers ever reaches. Denying it would mean either
`#[allow]` scattered through the crate or writing the caller first. Every other
lint is an error.

## Building

Through the flake, always, on every machine:

```sh
nix develop -c cargo test --all-features
nix develop -c cargo test --no-default-features
```

Both ends of the feature matrix, because a feature only one application turns
on is exactly the one that rots. `nix flake check` builds the package, which
runs the tests with every feature on.
