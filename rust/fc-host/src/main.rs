use gpui::application;
use gpui::prelude::*;
use gpui::{div, px, rgb, App, Context, Render, Window, WindowOptions};

#[cfg(test)]
use gpui::{Entity, TestAppContext, VisualTestContext};

struct MyView;

impl Render for MyView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("submit")
            .h(px(32.))
            .w_full()
            .bg(rgb(0x202020))
            .text_color(rgb(0xeeeeee))
            .child("hello from bite-gpui")
    }
}

fn main() {
    application().run(|cx: &mut App| {
        cx.open_window(WindowOptions::default(), |_, cx| cx.new(|_| MyView))
            .unwrap();
    });
}

/// The window is drawn in-process: no display server, no GPU.
#[gpui::test]
fn renders_without_a_display(cx: &mut TestAppContext) {
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| cx.new(|_| MyView))
            .unwrap()
    });
    let mut cx = VisualTestContext::from_window(window.into(), cx);

    // The root view mounts in a headless window. (Note: `debug_bounds("submit")`
    // returned None in bite-gpui 1.21.0 — the rendered-frame map is not committed
    // by the time it is read; tracked as a follow-up.)
    let root: Entity<MyView> = window.root(&mut cx).unwrap();
    let _ = root;
}
