"""Opt-in Windows integration test against installed CLIs and a loopback API.

Usage: python scripts/real-client-e2e.py --claude D:/npm-global/claude.ps1 --codex D:/npm-global/codex.ps1
Uses production config writers, disposable homes, fake credentials, no remote model.
"""
import argparse
import json
import os
from pathlib import Path
import subprocess
import tempfile
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

KEY = "sk-local-e2e-not-a-real-key"
REPO = Path(__file__).resolve().parents[1]
REQUESTS = []


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def send_json(self, status, data):
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.end_headers()
        self.wfile.write(json.dumps(data).encode())

    def do_CONNECT(self):
        REQUESTS.append({"blockedExternal": self.path})
        self.send_json(403, {"error": "Remote connections disabled by integration fixture"})

    def do_GET(self):
        self.send_json(200, {"object": "list", "data": [{"id": "gpt-5.3-codex", "object": "model"}]})

    def do_POST(self):
        raw = self.rfile.read(int(self.headers.get("Content-Length", 0)))
        try:
            body = json.loads(raw)
        except (ValueError, UnicodeDecodeError):
            self.send_json(400, {"error": "invalid JSON"})
            return
        auth_ok = self.headers.get("Authorization") == f"Bearer {KEY}"
        REQUESTS.append({"path": self.path, "model": body.get("model"), "authOk": auth_ok})
        if not auth_ok:
            self.send_json(401, {"error": {"type": "authentication_error", "message": "fixture credential missing"}})
            return
        if self.path.startswith("/v1/messages/count_tokens"):
            self.send_json(200, {"input_tokens": 10})
            return
        if self.path.startswith("/v1/messages"):
            message = {"id": "msg_fixture", "type": "message", "role": "assistant", "model": body["model"],
                       "content": [], "stop_reason": None, "stop_sequence": None,
                       "usage": {"input_tokens": 10, "output_tokens": 0}}
            events = [
                ("message_start", {"type": "message_start", "message": message}),
                ("content_block_start", {"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}),
                ("content_block_delta", {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "E2E_OK"}}),
                ("content_block_stop", {"type": "content_block_stop", "index": 0}),
                ("message_delta", {"type": "message_delta", "delta": {"stop_reason": "end_turn", "stop_sequence": None}, "usage": {"output_tokens": 4}}),
                ("message_stop", {"type": "message_stop"}),
            ]
        elif self.path == "/v1/responses":
            item = {"id": "msg_fixture", "type": "message", "role": "assistant", "status": "completed",
                    "content": [{"type": "output_text", "text": "E2E_OK", "annotations": []}]}
            response = {"id": "resp_fixture", "object": "response", "model": body["model"], "status": "completed",
                        "output": [item], "usage": {"input_tokens": 10, "output_tokens": 4, "total_tokens": 14}}
            events = [
                ("response.created", {"type": "response.created", "response": {**response, "status": "in_progress", "output": []}}),
                ("response.output_item.added", {"type": "response.output_item.added", "output_index": 0, "item": {**item, "content": [], "status": "in_progress"}}),
                ("response.output_text.delta", {"type": "response.output_text.delta", "output_index": 0, "content_index": 0, "item_id": "msg_fixture", "delta": "E2E_OK"}),
                ("response.output_item.done", {"type": "response.output_item.done", "output_index": 0, "item": item}),
                ("response.completed", {"type": "response.completed", "response": response}),
            ]
        else:
            self.send_json(404, {"error": "unexpected fixture endpoint"})
            return
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.end_headers()
        for name, data in events:
            self.wfile.write(f"event: {name}\ndata: {json.dumps(data)}\n\n".encode())
        self.wfile.flush()


def run_client(executable, args, env, cwd):
    command = [str(executable), *args]
    if executable.suffix.lower() == ".ps1":
        command = [str(Path(os.environ["SystemRoot"]) / "System32/WindowsPowerShell/v1.0/powershell.exe"),
                   "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", *command]
    process = subprocess.Popen(command, cwd=cwd, env=env, stdin=subprocess.DEVNULL,
                               stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                               creationflags=subprocess.CREATE_NO_WINDOW)
    try:
        out, err = process.communicate(timeout=45)
    except subprocess.TimeoutExpired:
        # Kill only this test's process tree, including npm wrappers.
        subprocess.run(["taskkill.exe", "/PID", str(process.pid), "/T", "/F"], capture_output=True, timeout=10)
        out, err = process.communicate(timeout=5)
        raise RuntimeError(f"client timed out: {err.decode(errors='replace')[-2000:]}")
    return process.returncode, out.decode(errors="replace"), err.decode(errors="replace")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--claude", type=Path, required=True)
    parser.add_argument("--codex", type=Path, required=True)
    args = parser.parse_args()
    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    base = f"http://127.0.0.1:{server.server_port}"
    failures = []
    try:
        with tempfile.TemporaryDirectory(prefix="zai-real-client-") as temporary:
            root = Path(temporary)
            work = root / "中文 space's & literal"
            work.mkdir()
            code, output, _ = run_client(args.codex, ["--version"], os.environ.copy(), work)
            if code or not output.strip().startswith("codex-cli "):
                raise RuntimeError("Unrecognized Codex executable")
            version = output.strip().split()[-1]
            subprocess.run(["cargo", "run", "--locked", "--example", "client_fixture", "--", str(root), base, version],
                           cwd=REPO / "src-tauri", check=True, timeout=180)
            for name in ["codex-selected", "codex-bearer", "claude"]:
                env = {k: v for k, v in os.environ.items() if not k.upper().startswith(("ANTHROPIC_", "OPENAI_", "CODEX_", "CLAUDE_"))}
                user = root / (name + "-user")
                user.mkdir()
                env.update({"USERPROFILE": str(user), "HOME": str(user), "APPDATA": str(user / "AppData/Roaming"),
                            "LOCALAPPDATA": str(user / "AppData/Local"), "CODEX_HOME": str(root / name),
                            "CLAUDE_CONFIG_DIR": str(root / "claude"), "HTTP_PROXY": base, "HTTPS_PROXY": base,
                            "ALL_PROXY": base, "NO_PROXY": "127.0.0.1,localhost", "DISABLE_AUTOUPDATER": "1",
                            "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": "1"})
                before = len(REQUESTS)
                if name == "claude":
                    executable = args.claude
                    options = ["-p", "Return E2E_OK only.", "--setting-sources", "user", "--no-session-persistence",
                               "--system-prompt", "Reply with a short text, never use tools.", "--output-format", "json"]
                    expected, model = "/v1/messages", "claude-sonnet-4-5"
                else:
                    executable = args.codex
                    options = ["exec", "--skip-git-repo-check", "--ephemeral", "--color", "never", "--json", "Return E2E_OK only. Do not use tools."]
                    expected, model = "/v1/responses", "gpt-5.3-codex"
                try:
                    code, out, err = run_client(executable, options, env, work)
                    seen = REQUESTS[before:]
                    ok = code == 0 and "E2E_OK" in out and any(r.get("path", "").startswith(expected) and r.get("authOk") and r.get("model") == model for r in seen)
                    ok = ok and not any(r.get("blockedExternal", "").split(":")[0] == "api.openai.com" for r in seen)
                    print(json.dumps({"case": name, "passed": ok, "exitCode": code, "requests": seen}, ensure_ascii=False), flush=True)
                    if not ok:
                        print((err + "\n" + out)[-4000:], flush=True)
                        failures.append(name)
                except Exception as error:
                    print(f"{name}: {error}", flush=True)
                    failures.append(name)
    finally:
        server.shutdown()
        server.server_close()
    if failures:
        raise SystemExit("FAILED: " + ", ".join(failures))


if __name__ == "__main__":
    main()
