from __future__ import annotations

import json
import os
from dataclasses import dataclass
from pathlib import Path

@dataclass(frozen=True)
class Config:
    backend: str = "none"
    command: str = ""
    args: tuple[str, ...] = ()
    env: dict[str, str] | None = None
    preview_root: Path | None = None
    deskmate_port: int | None = None
    allowed_apps: tuple[str, ...] = ()
    allow_view_changes: bool = False
    draw_document_id: str | None = None
    draw_layer_id: str | None = None

def load_config(path: str | None = None) -> Config:
    path = path or os.environ.get("SENSEI_CONFIG")
    if not path:
        return Config()
    raw = json.loads(Path(path).expanduser().read_text(encoding="utf-8-sig"))
    if not isinstance(raw, dict):
        raise ValueError("Config must be a JSON object")
    backend = raw.get("backend", "none")
    if backend not in ("none", "csp", "krita"):
        raise ValueError("backend must be none, csp or krita")
    command = raw.get("command", "")
    args = raw.get("args", [])
    env = raw.get("env", {})
    if backend != "none" and (not isinstance(command, str) or not Path(command).is_absolute()
                              or not Path(command).is_file()):
        raise ValueError("Use an absolute path to the installed upstream executable")
    if not isinstance(args, list) or not all(isinstance(a, str) for a in args):
        raise ValueError("args must be a list of strings")
    if not isinstance(env, dict) or not all(isinstance(k, str) and isinstance(v, str) for k, v in env.items()):
        raise ValueError("env must contain string pairs")
    port = raw.get("deskmate_port")
    apps = raw.get("allowed_apps", [])
    if port is not None and (type(port) is not int or not 1 <= port <= 65535):
        raise ValueError("Invalid DeskMate port")
    if not isinstance(apps, list) or not all(isinstance(a, str) and a.strip() for a in apps):
        raise ValueError("allowed_apps must be nonempty exact app names")
    if port is not None and not apps:
        raise ValueError("DeskMate requires an explicit app allowlist")
    root = raw.get("preview_root")
    preview_root = Path(root).expanduser().resolve() if root else None
    if preview_root and not preview_root.is_dir():
        raise ValueError("preview_root must be an existing dedicated preview directory")
    if preview_root and backend == "csp":
        # Both sides must use the same output directory.
        env = {**env, "CSP_COMPANION_OUTPUT": str(preview_root)}
    view = raw.get("allow_view_changes", False)
    if type(view) is not bool:
        raise ValueError("allow_view_changes must be boolean")
    doc, layer = raw.get("draw_document_id"), raw.get("draw_layer_id")
    if doc is not None or layer is not None:
        if backend != "krita" or not all(isinstance(x, str) and x.strip() for x in (doc, layer)):
            raise ValueError("Drawing requires a Krita document and dedicated layer ID")
    return Config(backend, command, tuple(args), env, preview_root, port,
                  tuple(a.casefold() for a in apps), view, doc, layer)
