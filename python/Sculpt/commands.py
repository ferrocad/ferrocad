"""The workbench commands: ``Sculpt_MakeCage``, ``Sculpt_MakeSkin``, ``Sculpt_NestInBody``, …

The commands are thin, as upstream's are: they police the selection, open one
transaction, call the bundle's creation functions and report what they made. The
object types own the geometry and the document owns the undo, so a command that
fails leaves the document as it was.

They register through ``FreeCADGui.addCommand`` and carry ``Group: "Sculpt"``, so a
workbench switcher — or the shell's command list — separates them from Part's
without knowing the module. Registration is idempotent: importing an app script
twice, or a test following the shell, must not raise upstream's duplicate-id
error.
"""

import FreeCAD
import FreeCADGui

from . import DEFAULT_BOX, DEFAULT_LEVELS, attach, box_cage, makeCage, makeSkin, nest
from .objects import is_cage, is_skin

GROUP = "Sculpt"

#: The largest refinement level a command will set (a level-4 box cage is 1 536
#: patches, already a slow rebuild in a debug build).
MAX_LEVELS = 4

MAKE_CAGE = "Sculpt_MakeCage"
MAKE_SKIN = "Sculpt_MakeSkin"
NEST_IN_BODY = "Sculpt_NestInBody"
LEVEL_UP = "Sculpt_LevelUp"
LEVEL_DOWN = "Sculpt_LevelDown"
ATTACH_PROXIES = "Sculpt_AttachProxies"


def cages(document):
    return [obj for obj in document.Objects if is_cage(obj)]


def skins(document):
    return [obj for obj in document.Objects if is_skin(obj)]


def _document():
    document = FreeCAD.ActiveDocument
    if document is None:
        raise RuntimeError("no active document")
    return document


def _chosen(predicate):
    return [obj for obj in FreeCADGui.Selection.getSelection() if predicate(obj)]


def _select(*objects):
    FreeCADGui.Selection.clearSelection()
    for obj in objects:
        FreeCADGui.Selection.addSelection(obj)


def _transaction(document, label, build):
    """Run ``build`` inside one named transaction and return its result."""
    document.openTransaction(label)
    try:
        result = build()
        document.recompute()
    except Exception:
        document.abortTransaction()
        raise
    document.commitTransaction()
    return result


def _names(objects):
    return ", ".join(obj.Name for obj in objects)


class _CreateCage:
    """``Sculpt_MakeCage`` — a box cage with its subdivision preview."""

    def GetResources(self):
        return {
            "MenuText": "Create Cage",
            "ToolTip": "Add a box cage and its Catmull-Clark preview",
            "Group": GROUP,
            "CmdType": "ForEdit",
            "FerroCAD_Surface": "dock",
        }

    def IsActive(self):
        return FreeCAD.ActiveDocument is not None

    def Activated(self):
        document = _document()
        cage = _transaction(
            document,
            "Sculpt cage",
            lambda: makeCage(document, "Cage", *box_cage(*DEFAULT_BOX), levels=DEFAULT_LEVELS),
        )
        _select(cage)
        return {
            "status": "created %s (%d control points, %d preview facets)"
            % (cage.Name, len(cage.CagePoints), cage.Mesh.CountFacets),
            "object": cage.Name,
        }


class _CreateSkin:
    """``Sculpt_MakeSkin`` — the B-rep a selected cage builds."""

    def GetResources(self):
        return {
            "MenuText": "Create Skin",
            "ToolTip": "Sew a cage into a solid",
            "Group": GROUP,
            "CmdType": "ForEdit",
            "FerroCAD_Surface": "dock",
        }

    def _subject(self):
        """The cage to skin: the selected one, else the document's only one."""
        document = FreeCAD.ActiveDocument
        if document is None:
            return None
        chosen = _chosen(is_cage)
        if chosen:
            return chosen[0]
        available = cages(document)
        return available[0] if len(available) == 1 else None

    def IsActive(self):
        return self._subject() is not None

    def Activated(self):
        document = _document()
        cage = self._subject()
        if cage is None:
            raise RuntimeError("select a cage first")
        skin = _transaction(
            document, "Sculpt skin", lambda: makeSkin(document, "Skin", cage)
        )
        _select(skin)
        if skin.isValid():
            detail = "%d faces, volume %.3f mm^3" % (
                len(skin.Shape.Faces),
                skin.Shape.Volume,
            )
        else:
            detail = "invalid: %s" % skin.getStatusString()
        return {"status": "created %s (%s)" % (skin.Name, detail), "object": skin.Name}


class _NestInBody:
    """``Sculpt_NestInBody`` — cage(s) as support, skin as the base feature."""

    def GetResources(self):
        return {
            "MenuText": "Nest in Body",
            "ToolTip": "Put the selected cage and skin in a PartDesign body",
            "Group": GROUP,
            "CmdType": "ForEdit",
            "FerroCAD_Surface": "dock",
        }

    def _subjects(self):
        document = FreeCAD.ActiveDocument
        if document is None:
            return []
        chosen = []
        for obj in FreeCADGui.Selection.getSelection():
            if is_skin(obj):
                # A skin brings its cage: the cage is the reference geometry the
                # solid is built from, so nesting one without the other would
                # leave the body's shape detached from its control hull.
                cage = obj.Cage
                if cage is not None and is_cage(cage) and cage not in chosen:
                    chosen.append(cage)
                chosen.append(obj)
            elif is_cage(obj):
                chosen.append(obj)
        # No selection: the whole bundle in the document, which is what the
        # workbench's single-model workflow expects.
        return chosen or (cages(document) + skins(document))

    def _body(self):
        chosen = _chosen(lambda obj: obj.TypeId == "PartDesign::Body")
        if chosen:
            return chosen[0], False
        for obj in _document().Objects:
            if obj.TypeId == "PartDesign::Body":
                return obj, False
        return None, True

    def IsActive(self):
        return bool(self._subjects())

    def Activated(self):
        document = _document()
        subjects = self._subjects()
        if not subjects:
            raise RuntimeError("select a cage or skin first")
        # Supports first: a base is accepted only while the body holds no solid,
        # so the cages must be in the group before the skin is.
        ordered = [obj for obj in subjects if is_cage(obj)] + [
            obj for obj in subjects if is_skin(obj)
        ]
        body, fresh = self._body()

        def build():
            target = body
            if target is None:
                target = document.addObject("PartDesign::Body", "SculptBody")
            return target, nest(target, *ordered)

        target, roles = _transaction(document, "Sculpt nest", build)
        _select(target)
        summary = ", ".join(
            "%s=%s" % (obj.Name, role) for obj, role in zip(ordered, roles)
        )
        return {
            "status": "nested %s in %s%s"
            % (summary, target.Name, " (new body)" if fresh else ""),
            "body": target.Name,
            "roles": list(roles),
        }


class _Level:
    """The shared ``Sculpt_LevelUp``/``Sculpt_LevelDown`` behaviour."""

    step = 0

    def GetResources(self):
        return {
            "MenuText": "More Levels" if self.step > 0 else "Fewer Levels",
            "ToolTip": "Refine the subdivision" if self.step > 0 else "Coarsen the subdivision",
            "Group": GROUP,
            "CmdType": "View",
        }

    def _subjects(self):
        document = FreeCAD.ActiveDocument
        if document is None:
            return []
        chosen = _chosen(
            lambda obj: is_cage(obj) or (is_skin(obj) and obj.getPropertyByName("Levels"))
        )
        return chosen or (cages(document) + skins(document))

    def IsActive(self):
        return bool(self._subjects())

    def Activated(self):
        document = _document()
        subjects = self._subjects()
        if not subjects:
            raise RuntimeError("select a cage or skin first")

        def build():
            changed = []
            for obj in subjects:
                current = int(obj.getPropertyByName("Levels") or 0)
                level = max(0, min(MAX_LEVELS, current + self.step))
                if level != current:
                    obj.Levels = level
                    changed.append("%s=%d" % (obj.Name, level))
            return changed

        changed = _transaction(document, "Sculpt levels", build)
        return {
            "status": "levels: %s" % (", ".join(changed) if changed else "unchanged")
        }


class _LevelUp(_Level):
    step = 1


class _LevelDown(_Level):
    step = -1


class _AttachProxies:
    """``Sculpt_AttachProxies`` — re-install the proxies after a ``.FCStd`` load."""

    def GetResources(self):
        return {
            "MenuText": "Attach Sculpt Proxies",
            "ToolTip": "Re-attach the cage and skin proxies to the open document",
            "Group": GROUP,
            "CmdType": "View",
        }

    def IsActive(self):
        return FreeCAD.ActiveDocument is not None

    def Activated(self):
        document = _document()
        attached = _transaction(document, "Sculpt attach", lambda: attach(document))
        return {
            "status": "attached: %s"
            % (", ".join(attached) if attached else "nothing to attach"),
            "objects": list(attached),
        }


COMMANDS = (
    (MAKE_CAGE, _CreateCage),
    (MAKE_SKIN, _CreateSkin),
    (NEST_IN_BODY, _NestInBody),
    (LEVEL_UP, _LevelUp),
    (LEVEL_DOWN, _LevelDown),
    (ATTACH_PROXIES, _AttachProxies),
)


def register():
    """Register every command once; returns the ids that were added now."""
    registered = set(FreeCADGui.listCommands())
    added = []
    for name, factory in COMMANDS:
        if name in registered:
            continue
        FreeCADGui.addCommand(name, factory())
        added.append(name)
    return added
