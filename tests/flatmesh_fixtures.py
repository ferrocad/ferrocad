"""Geometry helpers shared by the ``flatmesh`` tests.

The base tests (``test_flatmesh``) and the workbenches tests
(``test_flatmesh_part``) both measure the unwrapper's output, so the four small
measures live here rather than in one of them. The runner's contract
(``contracts/extension-sets.json``) only classifies ``test_*.py`` files, so a
helper module is not a suite.
"""

from __future__ import annotations


def flat_triangle_area(unwrapper):
    """The area of the flat triangles, so a folding overlap would double-count."""
    total = 0.0
    for a, b, c in unwrapper.tris:
        ax, ay = unwrapper.ze_nodes[a][:2]
        bx, by = unwrapper.ze_nodes[b][:2]
        cx, cy = unwrapper.ze_nodes[c][:2]
        total += abs((bx - ax) * (cy - ay) - (by - ay) * (cx - ax)) / 2.0
    return total


def loop_area(loop):
    return abs(
        sum(
            loop[i][0] * loop[i + 1][1] - loop[i + 1][0] * loop[i][1]
            for i in range(len(loop) - 1)
        )
    ) / 2.0


def boundary_edges(unwrapper):
    """Undirected triangle edges that appear exactly once, by node index."""
    counts = {}
    for triangle in unwrapper.tris:
        for k in range(3):
            a, b = triangle[k], triangle[(k + 1) % 3]
            key = (a, b) if a < b else (b, a)
            counts[key] = counts.get(key, 0) + 1
    return {edge for edge, count in counts.items() if count == 1}


def extent(unwrapper):
    xs = [point[0] for point in unwrapper.ze_nodes]
    ys = [point[1] for point in unwrapper.ze_nodes]
    return sorted((max(xs) - min(xs), max(ys) - min(ys)))
