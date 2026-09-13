# Documentation

Longer pieces that do not fit in a doc comment, for the parts of this crate
where the shape of the API is not the interesting question.

- `themes.md` — the theme file format, the derivation chain, and how an
  application adds tables of its own. *(with the theme modules)*
- `graphics.md` — capability probing, the protocols, cache keys, and the rules
  about clipping that differ per protocol. *(with the graphics module)*
- `dock.md` — the layout tree, sizing, seams, and the one-geometry-call-per-
  frame rule that draw and mouse handling both depend on. *(with the dock
  module)*

Anything smaller than those belongs in the module it describes. A doc comment
is read by the person changing the code; a file in here is read by the person
deciding whether to.
