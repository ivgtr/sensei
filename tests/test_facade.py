import asyncio
import json
from pathlib import Path

from mcp.types import CallToolResult, TextContent
from PIL import Image

from sensei_mcp.config import Config, load_config
from sensei_mcp.events import normalize_events
from sensei_mcp.server import Facade, preview_image

class FakeSession:
    def __init__(self):
        self.calls = []
    async def call_tool(self, name, arguments):
        self.calls.append((name, arguments))
        return CallToolResult(content=[TextContent(type="text", text='{"ok":true}')])

def test_disabled_by_default():
    f = Facade(Config())
    assert [t.name for t in f.tools()] == ["sensei_status"]
    assert asyncio.run(f.dispatch("paint_draw_path", {})).isError

def test_krita_reads_and_pinned_draw():
    s = FakeSession()
    f = Facade(Config(backend="krita", draw_document_id="doc", draw_layer_id="teacher"), s,
               {"krita_state", "krita_history", "krita_preview", "krita_paint_path", "krita_save_export"})
    assert "krita_save_export" not in [t.name for t in f.tools()]
    assert not asyncio.run(f.dispatch("paint_history")).isError
    assert s.calls[-1] == ("krita_history", {"action":"status"})
    args = {"points":[{"x":1,"y":2,"pressure":.5},{"x":4,"y":5,"pressure":.8}], "brush_size": 2}
    assert asyncio.run(f.dispatch("paint_draw_path", {**args, "layer_id":"other"})).isError
    assert not asyncio.run(f.dispatch("paint_draw_path", args)).isError
    assert s.calls[-1][1]["layer_id"] == "teacher"
    assert s.calls[-1][1]["document_id"] == "doc"
    assert asyncio.run(f.dispatch("paint_preview", {"x": 1})).isError

def test_csp_is_read_only_by_default():
    s = FakeSession()
    f = Facade(Config(backend="csp"), s, {"csp_navigate","csp_brush_state","csp_set_color"})
    assert [t.name for t in f.tools()] == ["sensei_status", "paint_state"]
    assert not asyncio.run(f.dispatch("paint_state")).isError
    assert asyncio.run(f.dispatch("paint_navigate", {"action":"ZoomIn"})).isError

def test_event_privacy():
    rows = [
        {"app_name":"Krita", "event_type":"click", "timestamp":"2026-01-01T10:00:00+09:00",
         "window_title":"secret", "data_json":json.dumps({"x":2,"y":3,"content":"private","key_code":42})},
        {"app_name":"Krita","event_type":"text","data_json":"{}"},
        {"app_name":"Browser","event_type":"click","data_json":"{}"},
        {"app_name":"Krita","event_type":"clipboard","data_json":"{}"},
    ]
    out = normalize_events(rows, ("krita",))
    assert len(out) == 1
    assert out[0]["data"] == {"x":2,"y":3}
    assert "secret" not in str(out) and "private" not in str(out)

def test_preview_root(tmp_path):
    root = tmp_path / "previews"
    root.mkdir()
    outside = tmp_path / "outside.png"
    inside = root / "inside.png"
    Image.new("RGB", (10,10)).save(outside)
    Image.new("RGB", (10,10)).save(inside)
    assert preview_image({"path":str(inside)},root).mimeType == "image/png"
    import pytest
    with pytest.raises(ValueError):
        preview_image({"path":str(outside)},root)

def test_no_unscoped_deskmate(tmp_path):
    p = tmp_path / "config.json"
    p.write_text('{"deskmate_port":3030}')
    import pytest
    with pytest.raises(ValueError):
        load_config(str(p))
