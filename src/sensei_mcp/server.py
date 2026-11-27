from __future__ import annotations

import asyncio
import base64
import io
import json
import sys
from contextlib import AsyncExitStack
from datetime import timedelta
from pathlib import Path
from typing import Any

import anyio
from jsonschema import validate
from mcp import ClientSession, StdioServerParameters
from mcp.client.stdio import stdio_client
from mcp.server import Server
from mcp.server.stdio import stdio_server
from mcp.types import CallToolResult, ImageContent, TextContent, Tool, ToolAnnotations
from PIL import Image

from .config import Config, load_config
from .events import recent_actions

def obj(properties: dict | None = None, required: list[str] | None = None) -> dict:
    return {"type": "object", "properties": properties or {},
            "required": required or [], "additionalProperties": False}

def integer(low: int, high: int) -> dict:
    return {"type": "integer", "minimum": low, "maximum": high}

def tool(name: str, description: str, schema: dict | None = None, write: bool = False) -> Tool:
    return Tool(name=name, description=description, inputSchema=schema or obj(),
                annotations=ToolAnnotations(readOnlyHint=not write, destructiveHint=write,
                                            openWorldHint=False))

def reply(value: Any, error: bool = False) -> CallToolResult:
    return CallToolResult(content=[TextContent(type="text", text=json.dumps(value, ensure_ascii=False))],
                          isError=error)

def payload(result: CallToolResult) -> dict:
    if isinstance(result.structuredContent, dict):
        return result.structuredContent
    for item in result.content:
        if isinstance(item, TextContent):
            try:
                value = json.loads(item.text)
                if isinstance(value, dict):
                    return value
            except ValueError:
                continue
    return {}

def preview_image(data: dict, root: Path) -> ImageContent:
    # Never accept a model-supplied filename. Only read the upstream-generated
    # PNG in the dedicated local preview directory.
    path = Path(data["path"]).resolve(strict=True)
    if not path.is_relative_to(root.resolve()) or path.suffix.lower() != ".png":
        raise ValueError("Preview path is outside the configured root")
    if not path.is_file() or path.stat().st_size > 12 * 1024 * 1024:
        raise ValueError("Preview is missing or too large")
    with Image.open(path) as image:
        if image.format != "PNG" or image.width > 1600 or image.height > 1600:
            raise ValueError("Unexpected preview dimensions or format")
        buffer = io.BytesIO()
        image.convert("RGB").save(buffer, format="PNG")
    return ImageContent(type="image", data=base64.b64encode(buffer.getvalue()).decode("ascii"),
                        mimeType="image/png")

class Facade:
    def __init__(self, config: Config, session: ClientSession | None = None,
                 available: set[str] | None = None):
        self.config = config
        self.session = session
        self.available = available or set()
        self.lock = asyncio.Lock()

    def routes(self) -> dict[str, tuple[Tool, str]]:
        c = self.config
        result: dict[str, tuple[Tool, str]] = {}
        def add(spec: Tool, upstream: str):
            if upstream in self.available:
                result[spec.name] = (spec, upstream)
        if c.backend == "csp":
            add(tool("paint_connection_status", "Read CSP Companion connection status."), "csp_status")
            add(tool("paint_connect", "Connect after local manual CSP Companion pairing. No artwork edit.", write=True), "csp_connect")
            add(tool("paint_disconnect", "Disconnect CSP Companion.", write=True), "csp_disconnect")
            add(tool("paint_state", "Read current synchronized CSP brush state. Layers and stroke history are unavailable."), "csp_brush_state")
            add(tool("paint_preview_gallery", "Request on-demand composite preview metadata, not a desktop screenshot.",
                     obj({"max_length": integer(1, 1600)})), "csp_preview_gallery")
            if c.preview_root:
                fields = {k: integer(0, 2**31-1) for k in ("gallery_id", "canvas_index", "left", "top", "right", "bottom")}
                add(tool("paint_preview_block", "Return an on-demand CSP composite preview PNG. Use gallery IDs and preview coordinates.",
                         obj(fields, list(fields))), "csp_preview_read")
            if c.allow_view_changes:
                add(tool("paint_navigate", "Change the current CSP view only, after the user's request.",
                         obj({"action": {"enum": ["ZoomIn", "ZoomOut", "PixelSize", "Fitting", "WholeSize",
                                                   "RotateLeft", "RotateRight", "Rotate0", "ReverseHorz",
                                                   "ReverseVert", "ResetPosition"]}}, ["action"]), write=True), "csp_navigate")
        elif c.backend == "krita":
            add(tool("paint_state", "Read active Krita document, layers, brush and capabilities."), "krita_state")
            add(tool("paint_history", "Read Krita Undo/Redo availability (not full stroke history)."), "krita_history")
            add(tool("paint_preview", "Return an on-demand whole-canvas or cropped Krita image.",
                     obj({"max_width": integer(64, 1600), "max_height": integer(64, 1600),
                          "x": integer(0, 1000000), "y": integer(0, 1000000),
                          "width": integer(1, 32768), "height": integer(1, 32768)})), "krita_preview")
            if c.draw_document_id and c.draw_layer_id:
                point = obj({"x": integer(0, 1000000), "y": integer(0, 1000000),
                             "pressure": {"type": "number", "minimum": 0, "maximum": 1}}, ["x", "y", "pressure"])
                add(tool("paint_draw_path", "Draw on the locally configured Krita document and layer ONLY. Requires user permission before each call.",
                         obj({"points": {"type": "array", "minItems": 2, "maxItems": 256, "items": point},
                              "brush_size": {"type": "number", "exclusiveMinimum": 0, "maximum": 100}},
                             ["points", "brush_size"]), write=True), "krita_paint_path")
        return result

    def tools(self) -> list[Tool]:
        result = [tool("sensei_status", "Report enabled backends, limitations and available tools.")]
        if self.config.deskmate_port:
            result.append(tool("desktop_recent_actions", "Read a bounded, app-filtered sample of recent Windows actions; no screenshots, text or clipboard.",
                               obj({"limit": integer(1, 100)})))
        return result + [spec for spec, _ in self.routes().values()]

    async def dispatch(self, name: str, args: dict | None = None) -> CallToolResult:
        args = dict(args or {})
        specs = {t.name: t for t in self.tools()}
        if name not in specs:
            return reply({"error": "TOOL_DISABLED_OR_UNKNOWN"}, True)
        try:
            validate(args, specs[name].inputSchema)
            if name == "sensei_status":
                return reply({"backend": self.config.backend,
                              "tools": list(specs),
                              "desktop_enabled": self.config.deskmate_port is not None,
                              "continuous_images": False,
                              "pen_pressure_capture": False,
                              "autonomous_inference": False,
                              "note": "MCP facade only; upstream applications and an agent host are separate."})
            if name == "desktop_recent_actions":
                return reply(await recent_actions(self.config.deskmate_port, self.config.allowed_apps,
                                                  args.get("limit", 50)))
            _, upstream = self.routes()[name]
            if name == "paint_history":
                args = {"action": "status"}
            if name == "paint_preview":
                crop = {"x", "y", "width", "height"}
                if crop.intersection(args) and not crop.issubset(args):
                    raise ValueError("Supply all four crop coordinates")
                args = {"max_width": 1024, "max_height": 1024, **args}
            if name == "paint_preview_gallery":
                args.setdefault("max_length", 1024)
            if name == "paint_preview_block":
                w, h = args["right"] - args["left"], args["bottom"] - args["top"]
                if not 0 < w <= 1600 or not 0 < h <= 1600:
                    raise ValueError("Preview block must be positive and at most 1600 pixels per side")
            if name == "paint_draw_path":
                args.update(document_id=self.config.draw_document_id, layer_id=self.config.draw_layer_id)
            if self.session is None:
                return reply({"error": "UPSTREAM_NOT_CONNECTED"}, True)
            async with self.lock:
                result = await self.session.call_tool(upstream, arguments=args)
            data = payload(result)
            if result.isError:
                return result
            if data.get("ok") is False:
                return reply(data, True)
            if name == "paint_preview_block":
                image = preview_image(data, self.config.preview_root)
                metadata = {k: data[k] for k in ("width", "height", "source") if k in data}
                return CallToolResult(content=[TextContent(type="text", text=json.dumps(metadata)), image])
            return result
        except Exception as exc:
            # Do not echo secret paths, upstream environment or raw validation values.
            return reply({"error": type(exc).__name__,
                          "message": "Call failed. Check local setup and inputs. Never automatically retry an edit; outcome may be unknown."}, True)

def make_server(facade: Facade) -> Server:
    server = Server("sensei")
    @server.list_tools()
    async def list_tools() -> list[Tool]:
        return facade.tools()
    @server.call_tool()
    async def call_tool(name: str, arguments: dict[str, Any]) -> CallToolResult:
        return await facade.dispatch(name, arguments)
    return server

async def run(config: Config) -> None:
    async with AsyncExitStack() as stack:
        session = None
        available = set()
        if config.backend != "none":
            params = StdioServerParameters(command=config.command, args=list(config.args), env=config.env)
            read, write = await stack.enter_async_context(stdio_client(params))
            session = await stack.enter_async_context(ClientSession(read, write,
                                         read_timeout_seconds=timedelta(seconds=45)))
            await session.initialize()
            cursor = None
            while True:
                page = await session.list_tools(cursor=cursor)
                available.update(t.name for t in page.tools)
                cursor = page.nextCursor
                if not cursor:
                    break
        server = make_server(Facade(config, session, available))
        async with stdio_server() as (read, write):
            await server.run(read, write, server.create_initialization_options())

def main() -> None:
    try:
        config = load_config()
    except (ValueError, OSError) as exc:
        print(f"Invalid Sensei local configuration ({type(exc).__name__})", file=sys.stderr)
        raise SystemExit(2) from None
    anyio.run(run, config)

if __name__ == "__main__":
    main()
