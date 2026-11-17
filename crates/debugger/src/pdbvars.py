"""ScratchRunner's variables helper for the Python ``pdb`` debugger.

This module is written into the debug session directory and put on
``PYTHONPATH``.  While the debuggee is stopped, the debugger sends::

    !__import__('scratchdbg_pdbvars').emit(locals(), globals(), 2, 0, 100)

``pdb`` executes the statement in the *current frame*, so the helper receives
the frame's local and global dictionaries.  It prints one line

    SCRATCHDBG:{"variables": [...]}

on stdout.  The session extracts the JSON after the marker.  Returning a
string instead (``p ...``) would wrap it in Python ``repr`` quotes, which is
why the helper prints directly.

Values recursively describe their children (dict entries, sequence elements,
object attributes) as long as the requested depth allows it.  Traversal has a
five second budget, a per-container child limit and a ``repr`` length limit,
so inspecting a large object cannot wedge the debugger.

This is our own implementation; it does not use CodeRunner's helper scripts.
"""

import json
import sys
import time

MARKER = "SCRATCHDBG:"
TIME_BUDGET = 5.0
DEFAULT_CHARS = 1500
MAX_DEPTH = 8


def _truncate(text, limit):
    if limit <= 0:
        return ""
    if len(text) <= limit:
        return text
    if limit <= 3:
        return text[:limit]
    return text[: limit - 3] + "..."


def _safe_repr(value, limit):
    try:
        text = repr(value)
    except Exception as exc:  # noqa: BLE001 - repr is user code
        text = "<repr failed: %s>" % (type(exc).__name__,)
    return _truncate(text, limit)


def _type_name(value):
    try:
        cls = type(value)
        module = getattr(cls, "__module__", "")
        name = getattr(cls, "__qualname__", getattr(cls, "__name__", "object"))
        if module in ("", "builtins", "__builtin__"):
            return name
        return module + "." + name
    except Exception:  # noqa: BLE001
        return "object"


def _is_container(value):
    if isinstance(value, (dict, list, tuple, set, frozenset)):
        return True
    return hasattr(value, "__dict__")


def _entry_name(value):
    """Names used by the variables outline for children."""
    try:
        return repr(value)
    except Exception:  # noqa: BLE001
        return "?"


def _dump(value, name, expr, kind, depth, deadline, seen, max_children, chars):
    node = {
        "name": name,
        "display_name": name,
        "type": _type_name(value),
        "kind": kind,
        "expr": expr,
        "value": None,
        "description": None,
        "raw_value": None,
        "children": [],
        "expandable": False,
        "truncated": False,
    }
    container = _is_container(value)
    if not container or depth <= 0:
        node["value"] = _safe_repr(value, chars)
        node["description"] = node["value"]
        node["expandable"] = bool(container)
        return node
    if id(value) in seen:
        node["value"] = "<cycle>"
        node["description"] = "<cycle>"
        return node
    seen.add(id(value))
    children = []
    truncated = False
    try:
        items = []
        if isinstance(value, dict):
            for key, child in list(value.items()):
                try:
                    child_expr = "%s[%r]" % (expr, key)
                except Exception:  # noqa: BLE001 - hostile __repr__
                    child_expr = expr + "[?]"
                items.append((_entry_name(key), child, child_expr))
        elif isinstance(value, (list, tuple)):
            for index, child in enumerate(value):
                items.append(("[%d]" % index, child, "%s[%d]" % (expr, index)))
        elif isinstance(value, (set, frozenset)):
            for index, child in enumerate(list(value)):
                items.append(
                    ("[%d]" % index, child, "list(%s)[%d]" % (expr, index))
                )
        else:
            attributes = getattr(value, "__dict__", None)
            if isinstance(attributes, dict):
                for key, child in list(attributes.items()):
                    items.append((str(key), child, "%s.%s" % (expr, key)))
        for child_name, child, child_expr in items:
            if len(children) >= max_children or time.monotonic() > deadline:
                truncated = True
                break
            children.append(
                _dump(
                    child,
                    child_name,
                    child_expr,
                    "member",
                    depth - 1,
                    deadline,
                    seen,
                    max_children,
                    chars,
                )
            )
    finally:
        seen.discard(id(value))
    node["children"] = children
    node["truncated"] = truncated
    node["expandable"] = truncated
    node["description"] = _safe_repr(value, min(chars, 200))
    return node


def _payload(frame_locals, frame_globals, depth, show_all, max_children):
    try:
        depth = int(depth)
    except Exception:  # noqa: BLE001
        depth = 2
    depth = max(0, min(depth, MAX_DEPTH))
    try:
        show_all = int(show_all) != 0
    except Exception:  # noqa: BLE001
        show_all = False
    try:
        max_children = max(1, int(max_children))
    except Exception:  # noqa: BLE001
        max_children = 100
    deadline = time.monotonic() + TIME_BUDGET
    seen = set()
    roots = []
    local_names = set()
    for name, value in list(frame_locals.items()):
        local_names.add(name)
        kind = "return_value" if name == "__return__" else "variable"
        roots.append(
            _dump(
                value,
                str(name),
                str(name),
                kind,
                depth,
                deadline,
                seen,
                max_children,
                DEFAULT_CHARS,
            )
        )
    if show_all:
        for name, value in list(frame_globals.items()):
            if name in local_names or name == "__builtins__":
                continue
            if name.startswith("__") and name.endswith("__"):
                continue
            if len(roots) >= max_children * 4:
                break
            roots.append(
                _dump(
                    value,
                    str(name),
                    str(name),
                    "global",
                    depth,
                    deadline,
                    seen,
                    max_children,
                    DEFAULT_CHARS,
                )
            )
    return {"variables": roots}


def emit(frame_locals, frame_globals, depth=2, show_all=0, max_children=100):
    """Print all variables of the current frame as one JSON line."""
    try:
        payload = _payload(frame_locals, frame_globals, depth, show_all, max_children)
    except Exception as exc:  # noqa: BLE001
        payload = {"variables": [], "error": "%s: %s" % (type(exc).__name__, exc)}
    _respond(payload)


def emit_at(expr, frame_locals, frame_globals, depth=2, max_children=100):
    """Print one expression's value (and children) as one JSON line."""
    try:
        value = eval(expr, frame_globals, frame_locals)  # noqa: S307 - debugger
    except Exception as exc:  # noqa: BLE001
        _respond({"variables": [], "error": "%s: %s" % (type(exc).__name__, exc)})
        return
    try:
        depth = max(0, min(int(depth), MAX_DEPTH))
    except Exception:  # noqa: BLE001
        depth = 2
    try:
        max_children = max(1, int(max_children))
    except Exception:  # noqa: BLE001
        max_children = 100
    node = _dump(
        value,
        expr,
        expr,
        "variable",
        depth,
        time.monotonic() + TIME_BUDGET,
        set(),
        max_children,
        DEFAULT_CHARS,
    )
    _respond({"variables": [node]})


def _respond(payload):
    sys.stdout.write(MARKER + json.dumps(payload, default=str) + "\n")
    sys.stdout.flush()
