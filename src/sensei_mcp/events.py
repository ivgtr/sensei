"""Read DeskMate's local REST API; expose only allowlisted, non-text events."""
from __future__ import annotations

import json
from collections import Counter
from typing import Any

import httpx

SAFE_TYPES = {"click", "move", "scroll", "app_switch", "window_focus"}
SAFE_NUMBERS = ("x", "y", "delta_x", "delta_y", "click_count")

def normalize_events(rows: list[dict[str, Any]], apps: tuple[str, ...]) -> list[dict[str, Any]]:
    result = []
    for row in rows:
        if not isinstance(row, dict):
            continue
        app, kind = row.get("app_name"), row.get("event_type")
        if not isinstance(app, str) or app.casefold() not in apps or kind not in SAFE_TYPES:
            continue
        data = row.get("data_json") or {}
        if isinstance(data, str):
            try:
                data = json.loads(data)
            except ValueError:
                data = {}
        if not isinstance(data, dict):
            data = {}
        safe = {k: data[k] for k in SAFE_NUMBERS if type(data.get(k)) is int}
        if data.get("button") in ("left", "right", "middle"):
            safe["button"] = data["button"]
        timestamp = row.get("timestamp")
        event = {"source": "deskmate", "app": app, "type": kind,
                 "coordinates": "screen_pixels", "data": safe}
        # Do not forward arbitrary strings from titles, text, clipboard or URLs.
        if isinstance(timestamp, str):
            from datetime import datetime
            try:
                event["timestamp"] = datetime.fromisoformat(timestamp).isoformat()
            except ValueError:
                pass
        if type(row.get("id")) is int:
            event["id"] = row["id"]
        result.append(event)
    return result

async def recent_actions(port: int, apps: tuple[str, ...], limit: int) -> dict:
    # Bounded scan: other applications may occupy some of the latest rows.
    scan = min(500, limit * 5)
    async with httpx.AsyncClient(timeout=5.0, trust_env=False, follow_redirects=False) as client:
        response = await client.get(f"http://127.0.0.1:{port}/events/recent",
                                    params={"limit": scan})
        response.raise_for_status()
        rows = response.json()
    if not isinstance(rows, list):
        raise ValueError("Unexpected DeskMate event response")
    events = normalize_events(rows[:scan], apps)[:limit]
    return {"events": list(reversed(events)),
            "counts": dict(Counter(e["type"] for e in events)),
            "scanned": min(len(rows), scan),
            "note": "Bounded recent sample, not complete history. Mouse/UI events are not pen strokes. Text and key events are excluded."}
