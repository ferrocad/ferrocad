"""Python-declarative UI spike.

A standalone, hermetic version of the eventual ``FreeCAD/ui/declarative.py``.
A workbench builds an ``Element`` tree in Python; ``render_ui()`` serializes it to
JSON and ``dispatch_event(handler_id)`` runs a callback and re-renders. The Rust
host parses the JSON and drives `bite-gpui` elements from it.
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

    def increment(self):
        self.counter += 1

    def fail_gracefully(self):
        raise RuntimeError("Controlled workbench fault")


app_state = AppState()
ACTIVE_REGISTRY: Dict[str, Callable] = {}


def render_ui() -> str:
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

    btn_fault = Element(
        "button", id="btn_err", title="Trigger Error"
    ).on("click", lambda: app_state.fail_gracefully())

    root.add(label).add(btn_increment).add(btn_fault)

    return json.dumps(root.serialize(ACTIVE_REGISTRY))


def dispatch_event(handler_id: str) -> str:
    if handler_id in ACTIVE_REGISTRY:
        # Executes the callback; may raise to test boundary isolation.
        ACTIVE_REGISTRY[handler_id]()
    return render_ui()
