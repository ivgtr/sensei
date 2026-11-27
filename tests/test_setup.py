import json
import sys
import tomllib
from pathlib import Path

import pytest

from sensei_mcp.setup import config_for, desk_config, host_config, codex_block, write_new, CSP_REV

def test_managed_csp_paths(tmp_path):
    cfg = config_for(tmp_path, "csp", True, ["CLIPStudioPaint"], 3031)
    assert cfg["backend"] == "csp"
    assert "envs" in Path(cfg["command"]).parts
    assert cfg["env"]["CSP_COMPANION_CONFIG"].endswith("session.json")
    assert cfg["allowed_apps"] == ["CLIPStudioPaint"]
    assert cfg["allow_view_changes"] is False
    assert len(CSP_REV) == 40

def test_managed_krita_paths(tmp_path):
    cfg = config_for(tmp_path, "krita", False, [], 3031)
    assert "krita-codex-mcp" in cfg["command"]
    assert "deskmate_port" not in cfg
    assert "draw_layer_id" not in cfg

def test_host_configs_match(tmp_path):
    python = Path(sys.executable)
    j = host_config(tmp_path, python)["mcpServers"]["sensei"]
    t = tomllib.loads(codex_block(tmp_path, python))["mcp_servers"]["sensei"]
    assert j["env"] == t["env"]
    assert j["command"] == t["command"]
    assert j["args"] == t["args"]

def test_desktop_profile(tmp_path):
    cfg = tomllib.loads(desk_config(3031, ["Krita"]))
    assert cfg["capture"]["enabled"] is False
    assert cfg["capture"]["include_screenshot"] is False
    assert cfg["a11y"]["capture_keystrokes"] is False
    assert cfg["a11y"]["capture_clipboard"] is False
    assert cfg["audio"]["enabled"] is False
    assert cfg["server"]["host"] == "127.0.0.1"
    assert cfg["server"]["port"] == 3031

def test_no_overwrite(tmp_path):
    f = tmp_path / "config.json"
    write_new(f, "original")
    with pytest.raises(FileExistsError):
        write_new(f, "replacement")
    assert f.read_text() == "original"

def test_desktop_requires_allowlist(tmp_path):
    with pytest.raises(ValueError):
        config_for(tmp_path, "none", True, [], 3031)
