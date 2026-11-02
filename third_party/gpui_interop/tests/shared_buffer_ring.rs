#![cfg(all(target_os = "linux", feature = "wgpu-test"))]
//! The shared-buffer ring's guarantee: a slow frame never hands GPUI a pending fence.
//!
//! GPUI polls a surface's acquire fence for 16 ms and drops the surface on timeout.
//! The ring is the producer-side answer: `present` returns a frame only after its
//! fence has signalled (and strips the fence), or the last good frame while a new
//! one is still running. This test submits a deliberately long frame and asserts
//! the invariant on both the first and the in-flight case.
//!
//! ```sh
//! cargo test -p gpui_interop --features wgpu-test --test shared_buffer_ring
//! ```

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    AnyWindowHandle, AppContext as _, Context, IntoElement, Render, Window, div, gpu_canvas,
    prelude::*, px, size,
};
use gpui_interop::{WgpuSharedBufferRing, attach};
use gpui_wgpu::{CosmicTextSystem, WgpuHeadlessRenderer};

/// The ring's slots, large enough that a clear pass takes measurable time.
const EDGE: u32 = 512;
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

struct Producer {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    ring: WgpuSharedBufferRing,
}

struct View {
    producer: Rc<RefCell<Option<Producer>>>,
    built: Rc<Cell<bool>>,
}

impl Render for View {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let producer = self.producer.clone();
        let built = self.built.clone();
        div().size_full().child(
            gpu_canvas(move |gpu| {
                let mut slot = producer.borrow_mut();
                if slot.is_none() {
                    let Some(lent) = gpu.try_device::<WgpuHeadlessRenderer>() else {
                        return;
                    };
                    let Ok(interop) = attach(Some(lent)) else {
                        return;
                    };
                    let Some((device, queue)) = interop.adapter().wgpu() else {
                        return;
                    };
                    let Ok(ring) = WgpuSharedBufferRing::new(&device, EDGE, EDGE, FORMAT) else {
                        return;
                    };
                    *slot = Some(Producer {
                        device,
                        queue,
                        ring,
                    });
                    built.set(true);
                }
                let producer = slot.as_mut().expect("built above");
                if let Some(handle) = producer.ring.present() {
                    gpu.paint_surface(handle);
                }
            })
            .size_full(),
        )
    }
}

/// Submit one frame: `passes` full-target clears, then export it.
fn submit(producer: &mut Producer, passes: u32) {
    let frame = producer.ring.begin().expect("a free slot");
    let mut encoder = producer
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("ring test frame"),
        });
    for _ in 0..passes {
        let _ = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("ring test clear"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: frame.view(),
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: 0.1,
                        g: 0.2,
                        b: 0.3,
                        a: 1.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
    }
    frame
        .submit(&producer.queue, Some(encoder.finish()))
        .expect("submit a frame");
}

/// Poll `present` until it returns a frame or the deadline expires.
fn wait_for_present(ring: &mut WgpuSharedBufferRing) -> Option<gpui_engine::DmaBufHandle> {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if let Some(handle) = ring.present() {
            return Some(handle);
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    None
}

#[test]
fn a_slow_frame_never_hands_gpui_a_pending_fence() {
    if !gpui::current_headless_renderer().is_ok_and(|renderer| renderer.is_some()) {
        eprintln!("skipping: no headless renderer (no Vulkan adapter)");
        return;
    }

    let text_system = Arc::new(CosmicTextSystem::new("fallback"));
    let mut cx = gpui::HeadlessAppContext::with_platform(text_system, Arc::new(()), || {
        gpui::current_headless_renderer()
    });

    let built = Rc::new(Cell::new(false));
    let producer = Rc::new(RefCell::new(None));
    let window = cx
        .open_window(size(px(EDGE as f32), px(EDGE as f32)), |_window, cx| {
            cx.new(|_| View {
                producer: producer.clone(),
                built: built.clone(),
            })
        })
        .expect("a window to composite into");
    let window: AnyWindowHandle = window.into();

    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| {
        let _ = window.draw(cx);
    })
    .expect("draw a frame");

    if !built.get() {
        eprintln!("skipping: the renderer lent no exportable wgpu device");
        return;
    }

    let mut producer = producer
        .borrow_mut()
        .take()
        .expect("the first paint built the ring");

    // A short frame gives the ring a good frame to repeat.
    submit(&mut producer, 1);
    let first = wait_for_present(&mut producer.ring).expect("the first frame becomes ready");
    assert!(
        first.acquire_fence.is_none(),
        "present must strip the acquire fence: GPUI can still poll it otherwise"
    );

    // Now a frame long enough to still be running when GPUI would paint. The ring
    // must repeat the first frame, not hand over a pending fence.
    submit(&mut producer, 2000);
    let during = producer.ring.present();
    if let Some(handle) = &during {
        assert!(
            handle.acquire_fence.is_none(),
            "a frame still in flight must not be presented with its fence"
        );
    }

    assert_eq!(
        producer.ring.pending(),
        1,
        "the slow frame should still be in flight, so present repeated the last good frame"
    );
    let during = during.expect("the last good frame is repeated while the slow frame runs");
    assert!(
        during.acquire_fence.is_none(),
        "the repeated frame must be fence-free"
    );

    let last = producer.ring.last().expect("a last good frame exists");
    assert!(last.acquire_fence.is_none(), "last() is fence-free too");

    // And the slow frame eventually arrives, still fence-free.
    let slow =
        wait_for_present(&mut producer.ring).expect("the slow frame eventually becomes ready");
    assert!(slow.acquire_fence.is_none());
}
