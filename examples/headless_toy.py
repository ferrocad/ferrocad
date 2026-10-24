"""A headless Toy demo for `ferrocad_toy --script`.

Run it with the kernel-free edition::

    VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json \
      cargo run -p ferrocad_toy -- \
        --script examples/headless_toy.py --render target/demo/toy.png --size 1200x800

The bootstrap already made one slab at the origin; this adds two more, each
through the workbench's normal creation path (one named transaction per slab).
The display provider applies each object's `Placement`, so the three are laid out
side by side without any kernel.
"""

import ferrocad_toy

ferrocad_toy.makeSlab(40.0, 40.0, 20.0, x=-60.0, name="Left")
ferrocad_toy.makeSlab(40.0, 40.0, 20.0, x=60.0, name="Right")
