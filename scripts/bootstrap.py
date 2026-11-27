"""One-command bootstrap from a checkout. No monitoring is started."""
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

if __name__ == "__main__":
    if sys.version_info < (3, 11):
        raise SystemExit("Python 3.11+ is required (3.12 recommended).")
    env = ROOT / ".venv"
    python = env / ("Scripts/python.exe" if sys.platform == "win32" else "bin/python")
    if not python.is_file():
        subprocess.run([sys.executable, "-m", "venv", str(env)], check=True)
    subprocess.run([str(python), "-m", "pip", "install", "-e", str(ROOT)], check=True)
    subprocess.run([str(python), "-m", "sensei_mcp.setup", *sys.argv[1:]], check=True)
