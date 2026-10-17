#!/usr/bin/env python3
"""End-to-end externalDocs regression without launching a browser.

HELIX_TEST_EXECUTABLE=/path/to/hx python3 contrib/test-external-docs.py
Uses a small LSP fixture to exercise the installed editor's RPC, keymaps,
local-file checks, workspace filtering and opener. Requires cargo and Python.
"""
from __future__ import annotations

from collections.abc import Callable
import fcntl
import json
import os
from pathlib import Path
import pty
import select
import shutil
import struct
import subprocess
import sys
import tempfile
import termios
import time


def server() -> None:
    """Deterministic rust-analyzer protocol fixture, not a semantic Rust parser."""
    root = Path(os.environ["DOCS_TEST_ROOT"])
    while True:
        headers: dict[str, str] = {}
        while line := sys.stdin.buffer.readline():
            if line == b"\r\n":
                break
            key, value = line.decode().split(":", 1)
            headers[key.lower()] = value.strip()
        if not headers:
            return
        message = json.loads(sys.stdin.buffer.read(int(headers["content-length"])))
        method = message.get("method")
        with (root / "requests.jsonl").open("a") as log:
            log.write(json.dumps(message) + "\n")
        if method == "exit":
            return
        if "id" not in message:
            continue
        result: object = None
        error: dict[str, object] | None = None
        if method == "initialize":
            assert message["params"]["capabilities"]["experimental"]["localDocs"]
            result = {"capabilities": {"textDocumentSync": 1, "hoverProvider": True,
                                       "definitionProvider": True, "positionEncoding": "utf-16"}}
        elif method == "experimental/externalDocs":
            row = message["params"]["position"]["line"]
            web = "https://docs.rs/dependency/1/dependency/struct.Item.html#method.run"
            local = (root / "local docs" / "struct.Item.html").as_uri() + "#method.run"
            if row == 0:
                result = {"local": local, "web": web}
            elif row == 1:
                result = {"local": (root / "missing.html").as_uri(), "web": web}
            elif row == 2:
                result = {"local": None, "web": "https://docs.rs/local/0.1/local/fn.local.html"}
            elif row == 4:
                error = {"code": -32601, "message": "externalDocs unsupported"}
            elif row == 5:
                result = web  # older rust-analyzer's response
        elif method == "textDocument/definition":
            row = message["params"]["position"]["line"]
            path = root / ("workspace/src/lib.rs" if row == 2 else "dependency/src/lib.rs")
            span = {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}}
            result = {"uri": path.as_uri(), "range": span}
            if row == 1:
                result = [{"targetUri": path.as_uri(), "targetRange": span, "targetSelectionRange": span}]
            elif row == 5:
                result = [result]
        elif method == "textDocument/hover":
            result = {"contents": {"kind": "markdown", "value": "HOVER_STILL_WORKS"}}
        response: dict[str, object] = {"jsonrpc": "2.0", "id": message["id"]}
        response["error" if error else "result"] = error or result
        data = json.dumps(response).encode()
        sys.stdout.buffer.write(f"Content-Length: {len(data)}\r\n\r\n".encode() + data)
        sys.stdout.buffer.flush()


def regression() -> None:
    executable = Path(os.environ.get("HELIX_TEST_EXECUTABLE", shutil.which("hx") or "hx")).resolve()
    with tempfile.TemporaryDirectory(prefix="helix-docs-") as temporary:
        root = Path(temporary)
        workspace = root / "workspace"
        for name in ["workspace", "dependency"]:
            package = root / name
            (package / "src").mkdir(parents=True)
            (package / "Cargo.toml").write_text(f'[package]\nname="{name}"\nversion="0.1.0"\nedition="2021"\n')
            (package / "src/lib.rs").write_text('pub fn local() { let _ = "😀"; }\n' + "pub fn local() {}\n" * 5)
        subprocess.run(["cargo", "generate-lockfile", "--offline"], cwd=workspace, check=True, capture_output=True)
        (root / "local docs").mkdir()
        (root / "local docs/struct.Item.html").write_text("documentation")
        config = root / "config/helix"
        config.mkdir(parents=True)
        (config / "config.toml").write_text('''[editor.workspace-trust]
        level = "insecure"
        prompt = false
        [keys.normal.space]
        K = "open_documentation"
        ''')
        script = Path(__file__).resolve()
        (config / "languages.toml").write_text(f'''[language-server.rust-analyzer]
        command = {json.dumps(sys.executable)}
        args = [{json.dumps(str(script))}, "--server"]
        [[language]]
        name = "rust"
        language-servers = ["rust-analyzer"]
        ''')
        binaries = root / "bin"
        binaries.mkdir()
        opener = binaries / "xdg-open"
        opener.write_text('#!/bin/sh\nprintf "%s\\n" "$1" >> "$DOCS_TEST_ROOT/opened"\n')
        opener.chmod(0o755)
        env = dict(os.environ, XDG_CONFIG_HOME=str(root / "config"),
                   XDG_CACHE_HOME=str(root / "cache"), XDG_DATA_HOME=str(root / "data"),
                   XDG_STATE_HOME=str(root / "state"), DOCS_TEST_ROOT=str(root),
                   PATH=f"{binaries}:{os.environ['PATH']}", TERM="xterm-256color")
        master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 30, 120, 0, 0))
        process = subprocess.Popen([executable, "--log", root / "hx.log", workspace / "src/lib.rs"],
                                   cwd=workspace, env=env, stdin=slave, stdout=slave, stderr=slave,
                                   start_new_session=True)
        os.close(slave)
        screen = bytearray()

        def drain(timeout: float = 0.1) -> None:
            if select.select([master], [], [], timeout)[0]:
                try:
                    data = os.read(master, 65536)
                except OSError:
                    return
                screen.extend(data)
                if b"\x1b[6n" in data:
                    os.write(master, b"\x1b[1;1R")
                if b"\x1b[c" in data:
                    os.write(master, b"\x1b[?1;2c")

        def wait_for(predicate: Callable[[], bool], description: str) -> None:
            deadline = time.monotonic() + 15
            while time.monotonic() < deadline:
                drain()
                if predicate():
                    return
                if process.poll() is not None:
                    break
            log = (root / "hx.log").read_text() if (root / "hx.log").exists() else ""
            raise AssertionError(f"Timed out: {description}\n{bytes(screen)[-3000:]!r}\n{log[-3000:]}")

        def requests() -> list[dict[str, object]]:
            path = root / "requests.jsonl"
            return [json.loads(line) for line in path.read_text().splitlines()] if path.exists() else []

        def opened() -> list[str]:
            path = root / "opened"
            return path.read_text().splitlines() if path.exists() else []

        try:
            wait_for(lambda: any(item.get("method") == "textDocument/didOpen" for item in requests()), "LSP ready")
            os.write(master, b" k")
            wait_for(lambda: b"HOVER_STILL_WORKS" in screen, "unchanged Space-k hover")
            os.write(master, b"\x1b")
            for _ in range(5):
                drain()
            os.write(master, b"gl K")
            wait_for(lambda: len(opened()) == 1, "local docs")
            docs_request = next(item for item in requests() if item.get("method") == "experimental/externalDocs")
            assert docs_request["params"]["position"]["character"] == 31, docs_request
            assert opened()[0] == (root / "local docs/struct.Item.html").as_uri() + "#method.run"
            os.write(master, b"j K")
            wait_for(lambda: len(opened()) == 2, "dependency web fallback")
            assert opened()[1].startswith("https://docs.rs/dependency/")
            screen.clear()
            os.write(master, b"j K")
            wait_for(lambda: b"No documentation available" in screen, "workspace URL suppressed")
            assert len(opened()) == 2
            screen.clear()
            os.write(master, b"j K")
            wait_for(lambda: b"No documentation available" in screen, "null response")
            assert len(opened()) == 2
            os.write(master, b"j K")
            wait_for(lambda: b"externalDocs unsupported" in screen, "server error")
            os.write(master, b"j K")
            wait_for(lambda: len(opened()) == 3, "legacy web response")
            assert opened()[2] == opened()[1]
            print(f"PASS {executable}: local-first + anchor, dependency web, workspace suppression, null/error/legacy responses, unchanged hover")
        finally:
            os.write(master, b"\x1b:q!\r")
            deadline = time.monotonic() + 3
            while process.poll() is None and time.monotonic() < deadline:
                drain()
            if process.poll() is None:
                process.kill()
            process.wait()
            os.close(master)


if __name__ == "__main__":
    if "--server" in sys.argv:
        server()
    else:
        regression()
