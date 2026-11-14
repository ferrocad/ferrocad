"""A Catmull-Clark quad mesh as an editable B-rep surface.

Each refined quad becomes one untrimmed bicubic ``Part.BSplineSurface``, built
with the 4x4 pole grid sampled from the quad's bilinear patch. Two neighbouring
patches share their boundary straight line exactly (the boundary control points
are collinear with the shared corners), so the shell sews without gaps and the
surface is G0 by construction; the smoothness comes from the refinement level,
not from the patch stitch.

That is a **house approximation**, not the add-on's limit-surface patch: matching
Catmull-Clark's limit stencils needs the one-ring of every corner and is a
separate piece of work. The level knob is the accuracy dial, and the drop in
volume between levels measures how far the approximation still is.
"""

import Part

import FreeCAD


def bicubic_patch(corners):
    """One bicubic patch over a quad, as a ``Part.Face``.

    ``corners`` are the quad's four points in order. The poles are the bilinear
    interpolation of the corners on a 4x4 grid, so the patch's four boundaries
    are the quad's straight edges and the interior follows the bilinear patch.
    """
    p0, p1, p2, p3 = [FreeCAD.Vector(corner) for corner in corners]

    def bilinear(u, v):
        return (
            p0 * ((1.0 - u) * (1.0 - v))
            + p1 * (u * (1.0 - v))
            + p2 * (u * v)
            + p3 * ((1.0 - u) * v)
        )

    poles = [[bilinear(i / 3.0, j / 3.0) for j in range(4)] for i in range(4)]
    surface = Part.BSplineSurface()
    surface.buildFromPolesMultsKnots(poles, [4, 4], [4, 4], udegree=3, vdegree=3)
    return surface.toShape()


def patch_shell(points, quads, tolerance):
    """The sewn shell of one patch per quad, or ``None`` when it is not closed.

    ``tolerance`` is passed to ``Shape.sewShape``; the points must already be in
    the frame the result should live in.
    """
    faces = [bicubic_patch([points[index] for index in quad]) for quad in quads]
    if not faces:
        return None
    shell = Part.Shell(faces)
    shell.sewShape(tolerance)
    return shell if shell.isClosed() else None


def patch_solid(points, quads, tolerance):
    """The closed solid of the patch shell (upstream ``Part.Solid(shell)``).

    Raises ``ValueError`` when the cage is not closed: an open cage is a surface,
    and silently returning a shell would let a body adopt a shape it cannot use.
    """
    shell = patch_shell(points, quads, tolerance)
    if shell is None:
        raise ValueError(
            "the cage is not closed: its subdivision sews to an open surface"
        )
    solid = Part.Solid(shell)
    if not solid.isValid():
        raise ValueError("the sewn surface is not a valid solid")
    return solid
