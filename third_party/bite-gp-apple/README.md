# bite-gp-apple

Part of **[bite-gpui](https://bite-gpui.github.io)** — Zed's [GPUI](https://github.com/zed-industries/zed) re-cut into a
modular, acyclic stack of crates: install a tier on its own, or the whole stack behind one facade.

This package is the published build of `gpui_apple`, staged from the `bite_v1.24.x` line of this
project's repository at `1.24.402`.

- **Website and guides** — <https://bite-gpui.github.io>
- **The stack, tier by tier** — <https://bite-gpui.github.io/#layers>
- **Source, and the branches this is built from** — <https://github.com/bite-gpui/bite-gpui>

## Install

```sh
# this crate
cargo add bite-gp-apple

# the whole stack, behind one facade
cargo add bite-gpui

```

## Hello, window

```rust
use gpui::*;

fn main() {
    gpui::application().run(|cx: &mut App| {
        cx.open_window(WindowOptions::default(), |_, cx| {
            cx.new(|_| MyView)
        })
        .unwrap();
    });
}
```

## Swap a seam

Every seam in the stack is a trait an implementation can replace without forking the engine.
[`bite-gp-parley`](https://bite-gpui.github.io/parley) is the published second implementation of `TextSystem`, and swapping it
in is one builder call:

```sh
cargo add bite-gp-parley
```

```rust
use std::cell::RefCell;
use std::rc::Rc;
use gpui::*;
use gpui_parley::ParleyTextSystem;

fn main() {
    let metrics = Rc::new(RefCell::new(PhaseMetrics::default()));

    application()
        .with_text_system(ParleyTextSystem::new())
        .with_frame_pipeline({
            let metrics = metrics.clone();
            move |_window_id| {
                Box::new(
                    StandardImmediatePipeline
                        .max_fps(60)
                        .instrumented(metrics.clone())
                )
            }
        })
        .run(|cx: &mut App| { /* … */ });
}
```

## Headless test — no display server, no GPU

With the `test-support` feature, a window is drawn in-process:

```rust
#[gpui::test]
fn renders_without_a_display(cx: &mut TestAppContext) {
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| cx.new(|_| MyView))
            .unwrap()
    });
    let mut cx = VisualTestContext::from_window(window.into(), cx);

    // The window is drawn in-process: no display server, no GPU.
    let bounds = cx.debug_bounds("submit").unwrap();
    assert_eq!(bounds.size.height, px(32.));
}
```

## Guides

- [Wraps & swaps](https://bite-gpui.github.io/swaps) — the two extension tiers, and the published crates behind them
- [Parley](https://bite-gpui.github.io/parley) — how to swap the text engine, and the measured trade that comes with it
- [Morphorm](https://bite-gpui.github.io/morphorm) — how to swap the layout solver, and its conformance numbers
- [Pass](https://bite-gpui.github.io/pass) — how to wrap the frame pipeline, and the rate and ledger that come with it
- [Demos](https://bite-gpui.github.io/demos) — projects built on the published crates
- [Compare](https://bite-gpui.github.io/compare) — the comparisons that can actually be run

## Provenance and licence

Derived from Zed's gpui: restructured, renamed and published as `bite-gp-apple` under `Apache-2.0`.
`NOTICE` records the modifications the licence asks to be made prominent, and `LICENSE-*` carries
the licence text.

    source:   https://github.com/bite-gpui/bite-gpui
    branch:   bite_v1.24.x  (upstream 1.24.4)
    version:  1.24.402
    commit:   8b288f1a2a06ed463d56228823a2f28a911e8855
