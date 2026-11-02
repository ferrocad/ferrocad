//! The macOS module: an `IOSurface` producer's surface, adopted into wgpu.
//!
//! **Not yet implemented.** This backend will accept an `IOSurface` from a foreign producer by building
//! an `MTLTexture` over it with `objc2-metal` and adopting that texture into wgpu, with an
//! `MTLSharedEvent` ordering the two queues. The matching half is trivial on this platform: wgpu's
//! adapter *is* the `MetalRenderer`'s `MTLDevice`, so there is nothing to match.
