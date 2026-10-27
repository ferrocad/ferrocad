# Research notes

The spikes, assessments and upstream investigations that shaped FerroCAD. These are
historical: they record what was measured and what was decided at the time. When a
result becomes a stable design, the decision is written in
[`../../developer/`](../../developer/README.md) and this note stays linked, not copied.

- [geometry-and-topology.md](geometry-and-topology.md) — topological sorting vs
  topological naming, and the seam for a future geometry engine.
- [occt-history-spike.md](occt-history-spike.md) — the first OCCT history spike: what
  FreeCAD depends on and what it implies for the trait.
- [coin-bridge-reuse-assessment.md](coin-bridge-reuse-assessment.md) — the evidence
  behind the Coin3D and scene-graph decisions.
- [gpu-interop-assessment.md](gpu-interop-assessment.md) — zero-copy GPU presentation:
  the dma-buf hand-off, the fallback and the platform matrix.
- [upstream-dmabuf-issue.md](upstream-dmabuf-issue.md) — the single-dma-buf race filed
  as `bite-gpui/gpui_interop#1` and the residual symptom.
- [python-ui-research.md](python-ui-research.md) — Python scripting and a Python-built
  UI: the research behind the shell.
- [grid-overlay-bringup.md](grid-overlay-bringup.md) — a third-party Coin/GLSL addon as
  a shader/extraction fixture.
- [simplecad-bringup.md](simplecad-bringup.md) — SimpleCAD as the canvas-first proving
  ground: the measured tree and S0–S5.

The complete folder list is in [`../../README.md`](../../README.md); the file-by-file
map is [`../../INDEX.md`](../../INDEX.md).
