"""User-invoked installer for isolated, version-selected upstream integrations.

No package downloads, recording or pairing occur during MCP server startup.
"""
from __future__ import annotations

import argparse
import asyncio
import json
import os
import shutil
import subprocess
import sys
from pathlib import Path

CSP_URL = "https://github.com/TMAK1710/csp-companion-mcp.git"
CSP_REV = "f0d611e4a156e29931ce9255dd76627d8160f7ec"
KRITA_PACKAGE = "krita-codex-mcp==0.2.2"
DESKMATE_PACKAGE = "deskmate @ git+https://github.com/zhaohb/deskmate.git@6c56c62b500408c2e86d6b6c3bf08bde3fca68bf"

def default_root() -> Path:
    return (Path(os.environ.get("LOCALAPPDATA", str(Path.home() / ".local" / "share"))) / "Sensei").resolve()

def executable(env: Path, name: str) -> Path:
    return env / ("Scripts" if sys.platform == "win32" else "bin") / (name + (".exe" if sys.platform == "win32" else ""))

def run(*args: str | Path, cwd: Path | None = None, env: dict | None = None) -> None:
    subprocess.run([str(a) for a in args], cwd=cwd, env=env, check=True)

def output(*args: str | Path, cwd: Path | None = None) -> str:
    return subprocess.check_output([str(a) for a in args], cwd=cwd, text=True).strip()

def make_env(directory: Path) -> Path:
    python = executable(directory, "python")
    if not python.is_file():
        run(sys.executable, "-m", "venv", directory)
    return python

def checkout_csp(root: Path) -> Path:
    source = root / "upstreams" / "csp-companion-mcp"
    if not source.exists():
        source.parent.mkdir(parents=True, exist_ok=True)
        if not shutil.which("git"):
            raise RuntimeError("Git is required to install CSP Companion.")
        # Stage separately: a failed fetch must not look like an installed checkout.
        import tempfile
        with tempfile.TemporaryDirectory(dir=source.parent) as tmp:
            staged = Path(tmp) / "checkout"
            run("git", "init", staged)
            run("git", "fetch", "--depth=1", CSP_URL, CSP_REV, cwd=staged)
            run("git", "checkout", "--detach", "FETCH_HEAD", cwd=staged)
            if output("git", "rev-parse", "HEAD", cwd=staged) != CSP_REV:
                raise RuntimeError("Unexpected CSP revision")
            staged.rename(source)
    if output("git", "rev-parse", "HEAD", cwd=source) != CSP_REV:
        raise RuntimeError("CSP checkout has another revision; preserve pairing data and inspect manually.")
    if output("git", "status", "--porcelain", "--untracked-files=no", cwd=source):
        raise RuntimeError("CSP checkout has local modifications; refusing to overwrite.")
    return source

def pip_install(python: Path, *args: str | Path) -> None:
    run(python, "-m", "pip", "install", *args)

def write_new(path: Path, value: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("x", encoding="utf-8") as f:
        f.write(value)

def config_for(root: Path, paint: str, desktop: bool, apps: list[str], port: int) -> dict:
    cfg = {"backend": paint, "allow_view_changes": False}
    if paint == "csp":
        source = root / "upstreams" / "csp-companion-mcp"
        cfg.update(command=str(executable(root / "envs" / "csp", "python")),
                   args=["-u", str(source / "server.py")],
                   env={"PYTHONUTF8": "1", "CSP_COMPANION_CONFIG": str(source / "session.json")},
                   preview_root=str(root / "previews"))
    elif paint == "krita":
        cfg.update(command=str(executable(root / "envs" / "krita", "krita-codex-mcp")), args=[])
    if desktop:
        if not apps:
            raise ValueError("Desktop integration requires at least one --app.")
        cfg.update(deskmate_port=port, allowed_apps=apps)
    return cfg

def desk_config(port: int, apps: list[str]) -> str:
    # Upstream may still log other apps' event metadata. This is NOT a capture allowlist.
    quoted = ", ".join(json.dumps(a, ensure_ascii=False) for a in apps)
    return f"""[capture]
enabled = false
event_driven = false
capture_on_visual_change = false
include_screenshot = false
capture_on_keystroke = false
capture_on_clipboard = false
record_input_events = true

[a11y]
enabled = true
capture_clicks = true
capture_keystrokes = false
capture_clipboard = false
capture_mouse_move = false
persist_elements = false

[ocr]
engine = "off"

[audio]
enabled = false

[filters]
included_windows = [{quoted}]

[server]
host = "127.0.0.1"
port = {port}

[habits]
enabled = false

[learning]
enabled = false

[model_service]
auto_start = false

[search]
semantic_enabled = false
"""

def host_config(root: Path, python: Path) -> dict:
    return {"mcpServers": {"sensei": {"command": str(python), "args": ["-m", "sensei_mcp.server"],
                                      "env": {"SENSEI_CONFIG": str(root / "config.json")}}}}

def codex_block(root: Path, python: Path) -> str:
    # JSON quoted strings are valid TOML basic strings for normal filesystem paths.
    return ("[mcp_servers.sensei]\ncommand = " + json.dumps(str(python), ensure_ascii=False)
            + '\nargs = ["-m", "sensei_mcp.server"]\ntool_timeout_sec = 60\n\n'
            + "[mcp_servers.sensei.env]\nSENSEI_CONFIG = "
            + json.dumps(str(root / "config.json"), ensure_ascii=False) + "\n")

def install(args: argparse.Namespace, root: Path) -> None:
    if sys.platform != "win32":
        raise RuntimeError("Managed integrations are intended for Windows.")
    if (root / "config.json").exists():
        raise RuntimeError("Configuration already exists. Use doctor/pair or choose a new --root; no settings will be overwritten.")
    if args.desktop and not args.app:
        args.app = ["CLIPStudioPaint"] if args.paint == "csp" else ["krita"] if args.paint == "krita" else []
    if not 1 <= args.port <= 65535:
        raise ValueError("Port must be between 1 and 65535")
    if args.desktop and not args.app:
        raise ValueError("Specify --app for Desktop-only setup")
    root.mkdir(parents=True, exist_ok=True)
    if args.paint == "csp":
        source = checkout_csp(root)
        python = make_env(root / "envs" / "csp")
        pip_install(python, "-r", source / "requirements.txt")
        (root / "previews").mkdir(exist_ok=True)
    elif args.paint == "krita":
        python = make_env(root / "envs" / "krita")
        pip_install(python, KRITA_PACKAGE)
    if args.desktop:
        python = make_env(root / "envs" / "desktop")
        pip_install(python, DESKMATE_PACKAGE)
        path = root / "desktop-data" / "config.toml"
        if not path.exists():
            write_new(path, desk_config(args.port, args.app))
        else:
            if path.read_text(encoding="utf-8") != desk_config(args.port, args.app):
                raise RuntimeError("Existing DeskMate settings differ. Choose a new --root or inspect them manually.")
    config = config_for(root, args.paint, args.desktop, args.app, args.port)
    # Generate host snippets, not overwrite existing Codex or Claude settings.
    write_new(root / "mcp.json", json.dumps(host_config(root, Path(sys.executable)), indent=2, ensure_ascii=False) + "\n")
    write_new(root / "codex.toml", codex_block(root, Path(sys.executable)))
    write_new(root / "config.json", json.dumps(config, indent=2, ensure_ascii=False) + "\n")
    print("Installed Sensei integrations at", root)
    print("Register only Sensei in your host using codex.toml or mcp.json.")
    if args.paint == "csp":
        print("Next: sensei-setup pair (manual CSP QR link), then doctor.")
    elif args.paint == "krita":
        print("Next: close Krita, run sensei-setup krita-bridge --allowed-root <existing-art-directory>, then enable the plugin in Krita and restart.")
    if args.desktop:
        print("Desktop recording is NOT running. Start it explicitly with sensei-setup desktop-start after reading the privacy notes.")

def read_managed(root: Path) -> dict:
    return json.loads((root / "config.json").read_text(encoding="utf-8"))

def pair(root: Path) -> None:
    if read_managed(root)["backend"] != "csp":
        raise RuntimeError("CSP backend is not configured")
    source = checkout_csp(root)
    session = source / "session.json"
    if session.exists():
        print("CSP already has pairing data; will not overwrite it.")
        return
    print("In CSP: File > Connect to smartphone. Obtain the pairing QR URL locally. Never share it with a model.")
    run(executable(root / "envs" / "csp", "python"), source / "setup_pairing.py", cwd=source)

def krita_bridge(root: Path, allowed_root: Path) -> None:
    if read_managed(root)["backend"] != "krita":
        raise RuntimeError("Krita backend is not configured")
    if not allowed_root.is_dir():
        raise ValueError("Choose an existing directory for permitted Krita artwork")
    run(executable(root / "envs" / "krita", "krita-codex-install"),
        "install", "--allowed-root", allowed_root.resolve())
    print("Enable Krita Codex Bridge in Krita's Python Plugin Manager and restart Krita.")

def desktop_start(root: Path) -> None:
    cfg = read_managed(root)
    if not cfg.get("deskmate_port"):
        raise RuntimeError("Desktop integration is not configured")
    expected = desk_config(cfg["deskmate_port"], cfg["allowed_apps"])
    if (root / "desktop-data" / "config.toml").read_text(encoding="utf-8") != expected:
        raise RuntimeError("DeskMate settings differ from the minimal capture profile. Review manually before starting.")
    # Do not inherit DeskMate environment overrides from another installation.
    env = {k: v for k, v in os.environ.items() if not k.upper().startswith("DESKMATE_")}
    env["DESKMATE_HOME"] = str(root / "desktop-data")
    print("Starting local Desktop recorder in the foreground. Other apps' event metadata may be recorded by DeskMate.")
    print("Screenshots, typed keys, clipboard, audio and OCR are disabled in the supplied profile. Ctrl+C stops it.")
    run(executable(root / "envs" / "desktop", "deskmate"), "serve",
        "--host", "127.0.0.1", "--port", str(cfg["deskmate_port"]), env=env)

async def doctor(root: Path) -> None:
    from mcp import ClientSession, StdioServerParameters
    from mcp.client.stdio import stdio_client
    from datetime import timedelta
    from .config import load_config
    cfg = load_config(str(root / "config.json"))
    params = StdioServerParameters(command=sys.executable, args=["-m", "sensei_mcp.server"],
                                   env={"SENSEI_CONFIG": str(root / "config.json")})
    async with stdio_client(params) as (read, write):
        async with ClientSession(read, write, read_timeout_seconds=timedelta(seconds=60)) as client:
            await client.initialize()
            tools = {t.name for t in (await client.list_tools()).tools}
            result = await client.call_tool("sensei_status")
            if result.isError:
                raise RuntimeError("Sensei status failed")
            print("MCP handshake OK:", ", ".join(sorted(tools)))
            if cfg.backend == "csp":
                status = await client.call_tool("paint_connection_status")
                print("CSP status:", status.content[0].text)
                if status.isError:
                    raise RuntimeError("CSP status failed")
            elif cfg.backend == "krita":
                print("Krita: this checks tool discovery only. For a live bridge check use krita-check.")
    if cfg.deskmate_port:
        import httpx
        try:
            async with httpx.AsyncClient(timeout=3, trust_env=False) as h:
                response = await h.get(f"http://127.0.0.1:{cfg.deskmate_port}/health")
                response.raise_for_status()
            print("DeskMate HTTP endpoint reachable (not a capture-quality check).")
        except httpx.HTTPError:
            print("DeskMate is not reachable; start desktop-start in another terminal if desired.")

def main() -> None:
    parser = argparse.ArgumentParser(description="Install and configure Sensei integrations locally")
    parser.add_argument("--root", type=Path, default=default_root())
    sub = parser.add_subparsers(dest="action", required=True)
    p = sub.add_parser("install")
    p.add_argument("--paint", choices=["csp", "krita", "none"], default="csp")
    p.add_argument("--desktop", action="store_true", help="Install optional DeskMate; do not start recording")
    p.add_argument("--app", action="append", default=[], help="Exact app name for returned actions (repeatable)")
    p.add_argument("--port", type=int, default=3031, help="Separate local DeskMate HTTP port")
    sub.add_parser("pair")
    k = sub.add_parser("krita-bridge")
    k.add_argument("--allowed-root", type=Path, required=True)
    sub.add_parser("krita-check")
    sub.add_parser("desktop-start")
    sub.add_parser("doctor")
    sub.add_parser("host-config")
    args = parser.parse_args()
    root = args.root.expanduser().resolve()
    try:
        if args.action == "install":
            install(args, root)
        elif args.action == "pair":
            pair(root)
        elif args.action == "krita-bridge":
            krita_bridge(root, args.allowed_root)
        elif args.action == "krita-check":
            if read_managed(root)["backend"] != "krita":
                raise RuntimeError("Krita backend is not configured")
            run(executable(root / "envs" / "krita", "krita-codex-install"), "check")
        elif args.action == "desktop-start":
            desktop_start(root)
        elif args.action == "doctor":
            asyncio.run(doctor(root))
        elif args.action == "host-config":
            read_managed(root)
            print(codex_block(root, Path(sys.executable)))
    except KeyboardInterrupt:
        raise SystemExit(130) from None
    except (OSError, ValueError, RuntimeError, subprocess.CalledProcessError) as exc:
        print(f"Setup failed: {exc}", file=sys.stderr)
        raise SystemExit(1) from None

if __name__ == "__main__":
    main()
