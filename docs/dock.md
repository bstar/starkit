# Responsive panes

New applications use `starkit::panes`. `dock`, documented below, remains available
for existing split-tree callers. Pane layout works with the minimal feature set;
geometry and interaction do not require graphics, FFmpeg or an audio device.

```rust
use starkit::panes::{Node, Size, Overflow, Wrap};
use starkit::ratatui::layout::Rect;

let tree = Node::column(vec![
    Node::pane("browser").height(Size::fill(8)).priority(100),
    Node::row(vec![
        Node::pane("preview").width(Size::fill(30))
            .height(Size::bounded(4, 8, 16).grow(1)),
        Node::pane("activity").width(Size::fill(30))
            .height(Size::bounded(4, 8, 16).grow(1))
            .overflow(Overflow::CollapseThenHide)
            .compact(Size::fill(12), Size::fixed(2)),
    ]).gap(1).wrap(Wrap::AtMinimum),
]);
let frame = tree.layout(Rect::new(0, 0, 100, 40))?;
let preview = frame.rect_of("preview");
let clicked = frame.hit(10, 12);
# Ok::<(), starkit::panes::LayoutError>(())
```

Use the **same solved frame** for painting, pointer hit tests and focus. Rectangles
never overlap or extend outside the supplied area. Maximum sizes may leave unused
background; clear the root background when painting. Every pane remains in
`frame.panes`, including hidden panes with no rectangle, so agents can inspect
geometry and the reason for each fallback through serde without a renderer.

Lengths are cells for a cell presentation or physical pixels for a native one.
Use a consistent unit throughout a tree. Glyph shaping and media placement remain
with their existing backends; changing a pane allocation does not resize glyphs.

`Size::bounded(min, preferred, max)` supplies a preference without growth;
`.grow(weight)` shares space left after preferences. Higher-priority siblings
receive preferred sizes first. On overflow, lower-priority siblings yield first;
equal priorities yield from the end. Only fallback actions that relieve the
shortage are applied. `Keep` content can be clipped when even its minimum cannot
fit: `frame.fits` is false and affected panes explicitly report `clipped`.

Rows and columns can wrap at their children's minimum extents before collapsing
or hiding them. Container floors are explicit; containers adapt their descendants
inside the allocation. A compact header belongs to a pane with an ID: groups can
hide but cannot collapse into an anonymous header. Duplicate IDs, trees deeper
than 64 levels and trees larger than 4096 nodes return `LayoutError`.

User hiding, user collapse, viewport rules and insufficient space have distinct
reasons. Solving never mutates the tree, so growing a window restores automatic
fallbacks without reopening a pane the user closed or overwriting a manual size.

`Focus` remembers the requested pane across responsive hiding. A user focus step
replaces that intent; `FocusPolicy::Direct` allows explicit focus without a Tab
stop, and `Never` excludes a subtree. Modal scopes trap traversal and background
focus requests until popped, then restore the previous scope or visible pane.
`ResizeCapture` retains the press origin, bounds and unit scale across repaints;
the application cancels it on viewport, modal or presentation changes.

The runnable example supports interactive resize/hide/focus, headless JSON
inspection, and a solver benchmark:

```sh
nix develop -c cargo run --example responsive-panes --no-default-features
nix develop -c cargo run --example responsive-panes --no-default-features -- --inspect 50x18
nix develop -c cargo run --release --example responsive-panes --no-default-features -- --bench
```

# The dock

`starkit::dock` lays panels out. It is a tree of splits and leaves, solved
once per frame, and its answer is the only source of geometry in the
application: drawing reads it, and so does mouse handling.

That rule is the point of the module. Both STAR/AMP and STAR/CORD used to
build a list of ratatui `Constraint`s in draw order and keep a parallel struct
of rects so the mouse could ask where things had ended up. Two pieces of code
that have to agree, one of which is only exercised by a pointer, is where a
layout bug lives. Here there is one:

```rust
let solved = self.dock.layout(area);   // once, at the top of draw
self.last = solved;                    // and handle_mouse reads this
```

## The tree

```rust
Dock::new(Node::column(Size::Fill, vec![
    Node::row(Size::Fill, vec![
        Node::leaf(Guilds, Size::Fixed(8), 8),
        Node::column(Size::Fixed(26), vec![
            Node::leaf(Channels, Size::Flex(3), 3),
            Node::leaf(Dms,      Size::Flex(2), 3),
        ]),
        Node::column(Size::Flex(1), vec![
            Node::leaf(Chat,     Size::Flex(1), 4),
            Node::leaf(Composer, Size::Fixed(3), 3),
        ]),
        Node::leaf(Members, Size::Fixed(24), 18),
    ]),
    Node::leaf(Status, Size::Fixed(1), 1),
]))
```

A leaf is a panel. A split divides its area along one axis and hands the
pieces to its children. `Axis::Vertical` stacks them; `Axis::Horizontal` puts
them side by side.

## Sizes

Every node says how much of its parent it takes, measured along the parent's
axis.

| Size | In a file | Means |
| --- | --- | --- |
| `Fixed(n)` | `"24"` | exactly `n` cells |
| `Flex(w)` | `"3fr"` | a share of what the fixed children left, by weight |
| `Fill` | `"fill"` | the rest; identical to `Flex(1)` |

Fixed children are satisfied first. What is left is divided among the flexible
ones in proportion to their weights, and any rounding remainder goes to the
last of them. Space nobody asked for — every child fixed, in a larger area —
goes to the last child, because the rects have to tile. Put a `Fill` where you
want the slack instead.

`min` is the smallest a leaf is useful at, in cells, along the same axis as
its size. A split's minimum is the sum of its children's along its own axis
and the largest of them across it. A leaf that needs both a minimum width and
a minimum height needs the tree to say so: give it a `min` for the axis its
parent splits on, and let the other come from an ancestor.

## What comes back

```rust
pub struct Layout<P> {
    pub placed: Vec<Placed<P>>,   // id and rect, in drawing order
    pub too_small: Vec<P>,        // the ones the area could not take
}
```

`placed` tiles the area exactly. No gaps, no overlaps, whatever the area is —
which is the property the proptests hold on to, because a gap is a stale cell
that never gets redrawn and an overlap is two panels writing the same cell in
an order that depends on the tree.

`too_small` names a panel that is either absent from `placed` — dropped so its
siblings could keep their minimums — or present with a rect smaller than its
`min`, because there was no sibling left with room to give. Both mean the same
thing to the caller: this panel cannot be drawn properly at this size.

Children are dropped from the end. That is arbitrary and deliberately so: the
tree has no way to rank panels. An application that cares which panel survives
a narrow terminal hides one itself first, which also lets it tell a panel the
user closed from one the width closed.

## Seams

The seam between two adjacent siblings is the one cell of border at the
**leading edge of the second one** — not a row of its own. The panel already
draws a box there. Giving the seam a cell would either move every panel over
by one or make the rects stop tiling, and both are worse than sharing a cell
with a border that is already drawn.

```rust
if let Some(i) = dock.seam_at(x, y) { self.drag = Some(Drag::Seam(i)); }
// .. while the button is held
dock.drag_seam(i, dx);
```

`drag_seam` takes space from one neighbour and gives it to the other. Neither
goes below its minimum, and dragging back by the same amount puts the layout
back exactly where it was — which is why it rewrites the split's weights
rather than nudging them. Every flexible child is given the weight of the
extent it has now, so the proportions nobody dragged come out of the next
solve unchanged, cell for cell. A fixed panel stays fixed, at the size the
pointer left it: widening the terminal afterwards does not undo the drag.

It acts on the geometry of the last `layout`, which is the frame the pointer
is in. Call `layout` first.

## Hiding and focus

`hide`, `show` and `toggle` take a panel out of the solve without touching any
sizes, so showing it again restores the layout the user had rather than one
the solver invented. A split whose children are all hidden is itself hidden,
so closing the last panel in a column closes the column.

`focus_next` and `focus_prev` walk `visible_order()` — the visible leaves,
depth first, which is the order they are drawn in — and wrap. `focus_set`
opens a panel that was closed, because `alt+3` should reach the third panel
whether or not it is on screen. Hiding the focused panel moves focus to the
first visible one.

## In a config file

`to_config` and `from_config` convert to and from `DockConfig<P>`, which is
plain serde and round-trips through TOML.

| Key | Where | Means |
| --- | --- | --- |
| `hidden` | top level | panels that are closed, as an array of ids |
| `focus` | top level | the focused panel, omitted when there is none |
| `root` | top level | the tree |
| `panel` | a node | the panel id; a node has this or `axis`, never both |
| `axis` | a node | `"vertical"` or `"horizontal"` |
| `size` | a node | `"24"`, `"3fr"` or `"fill"` |
| `min` | a node | cells, omitted when zero |
| `children` | a node | the nodes under a split |

```toml
hidden = ["members"]
focus = "chat"

[root]
axis = "vertical"
size = "fill"

[[root.children]]
panel = "chat"
size = "1fr"
min = 4

[[root.children]]
panel = "status"
size = "1"
min = 1
```

A `hidden` entry naming a panel that is not in the tree is kept rather than
dropped, so a file written by a newer version survives a downgrade. A node
that names both `panel` and `axis`, or neither, and a split with no children,
are errors rather than guesses.
