# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).
On 0.x, an API change is a minor bump and a fix is a patch.

## [Unreleased]

### Added

- **The crate itself, and one copy of ratatui for both applications.**
  `ratatui`, `crossterm` and, under the `image` feature, `ratatui-image` and
  `image` are re-exported from the crate root. A widget implemented against a
  second copy of ratatui does not satisfy a signature expecting the first, and
  the compiler reports that as a mismatch between two versions carrying the
  same number. Taking all four from here makes that impossible rather than
  unlikely.
