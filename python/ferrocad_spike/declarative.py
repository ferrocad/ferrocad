"""Python-declarative UI spike.

A standalone, hermetic version of the eventual ``FreeCAD/ui/declarative.py``.
A workbench builds an ``Element`` tree in Python; ``render_ui()`` serializes the
full tree (initial state), and ``dispatch_event(handler_id)`` runs a callback
then emits a *diffed patch stream* against the previous tree, so only changes
cross the PyO3 boundary.
"""

import json
from typing import Callable, Dict, List, Optional


class Element:
    def __init__(self, tag: str, **attributes):
        self.tag: str = tag
        self.attributes: Dict = dict(attributes)
        self.children: List["Element"] = []
        self.event_handlers: Dict[str, Callable] = {}
        self.id: str = attributes.get("id", "")

    def on(self, event_name: str, handler: Callable) -> "Element":
        self.event_handlers[event_name] = handler
        return self

    def add(self, child: "Element") -> "Element":
        self.children.append(child)
        return self

    def serialize(self, registry: Dict[str, Callable]) -> dict:
        event_keys = []
        for evt, handler in self.event_handlers.items():
            handler_id = f"{self.id}_{evt}"
            registry[handler_id] = handler
            event_keys.append([evt, handler_id])

        return {
            "id": self.id,
            "tag": self.tag,
            "props": {k: v for k, v in self.attributes.items() if k != "id"},
            "events": event_keys,
            "children": [c.serialize(registry) for c in self.children],
        }


class AppState:
    def __init__(self):
        self.counter: int = 0
        self.label_prefix: str = "Total Operations: "
        self.items: List[str] = []

    def increment(self):
        self.counter += 1

    def add_item(self):
        self.items.append(f"Item {len(self.items) + 1}")

    def remove_item(self):
        if self.items:
            self.items.pop()

    def fail_gracefully(self):
        raise RuntimeError("Controlled workbench fault")


app_state = AppState()
ACTIVE_REGISTRY: Dict[str, Callable] = {}
_prev_tree: Optional[dict] = None


def reset_state() -> None:
    """Reset module-global state (test isolation between Rust tests)."""
    global _prev_tree
    app_state.counter = 0
    app_state.items = []
    _prev_tree = None


def _snapshot() -> dict:
    ACTIVE_REGISTRY.clear()

    root = Element("container", id="root", layout="vertical")

    label = Element(
        "label",
        id="counter_label",
        text=f"{app_state.label_prefix}{app_state.counter}",
    )

    btn_increment = Element(
        "button", id="btn_inc", title="Increment Count"
    ).on("click", lambda: app_state.increment())

    btn_add = Element(
        "button", id="btn_add", title="Add Item"
    ).on("click", lambda: app_state.add_item())

    btn_remove = Element(
        "button", id="btn_rem", title="Remove Item"
    ).on("click", lambda: app_state.remove_item())

    btn_fault = Element(
        "button", id="btn_err", title="Trigger Error"
    ).on("click", lambda: app_state.fail_gracefully())

    root.add(label).add(btn_increment).add(btn_add).add(btn_remove).add(btn_fault)

    for index, item in enumerate(app_state.items):
        root.add(Element("label", id=f"item_{index}", text=item))

    return root.serialize(ACTIVE_REGISTRY)


def render_ui() -> str:
    """Return the full tree (used only for the initial render)."""
    global _prev_tree
    _prev_tree = _snapshot()
    return json.dumps(_prev_tree)


def _diff_children(old_list, new_list, parent_id, patches):
    old_by_id = {c["id"]: c for c in old_list}
    new_by_id = {c["id"]: c for c in new_list}

    for cid in old_by_id:
        if cid not in new_by_id:
            patches.append({"op": "remove", "id": cid})

    for index, new_child in enumerate(new_list):
        old_child = old_by_id.get(new_child["id"])
        if old_child is None:
            patches.append(
                {"op": "insert", "parent": parent_id, "index": index, "node": new_child}
            )
        else:
            _diff_node(old_child, new_child, patches)


def _diff_node(old, new, patches):
    changed = {}
    for key in ("text", "title"):
        ov = old.get("props", {}).get(key)
        nv = new.get("props", {}).get(key)
        if ov != nv:
            changed[key] = nv
    if changed:
        patches.append({"op": "update", "id": new["id"], "props": changed})

    _diff_children(old.get("children", []), new.get("children", []), new["id"], patches)


def _diff(old, new):
    patches = []
    _diff_node(old, new, patches)
    return patches


def dispatch_event(handler_id: str) -> str:
    """Run a handler, then return the diffed patch stream (JSON array)."""
    global _prev_tree
    if handler_id in ACTIVE_REGISTRY:
        # Executes the callback; may raise to test boundary isolation.
        ACTIVE_REGISTRY[handler_id]()
    new_tree = _snapshot()
    patches = _diff(_prev_tree, new_tree)
    _prev_tree = new_tree
    return json.dumps(patches)
