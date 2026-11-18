"""Actual provider-free authenticated daemon and project test bootstrap."""
import os
import socket
import sys
import threading
import types
from datetime import datetime
from pathlib import Path

import uvicorn

PROJECT = Path(os.environ["PORYGON_SOURCE_ROOT"]).resolve()
sys.path[:0] = [str(PROJECT), str(PROJECT / "tests"), str(PROJECT / "src")]
tests_package = types.ModuleType("tests")
tests_package.__path__ = [str(PROJECT / "tests")]
sys.modules["tests"] = tests_package
import conftest
from porygon.server import auth, main
from porygon.server.conversations import ConversationStore
from test_native_client_contract import _no_lifespan

TOKEN = "native-selection-isolated-fixture"


class Daemon:
    def __init__(self, context, monkeypatch):
        monkeypatch.setattr(auth, "_daemon_token", TOKEN)
        monkeypatch.setattr(auth, "_api_keys", {})
        self.app = main.create_app()
        self.app.router.lifespan_context = _no_lifespan
        self.app.state.context = context
        self.app.state.start_time = datetime.now()
        self.app.state.mcp_manager = None
        self.app.state.conversation_store = ConversationStore()
        self.listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        self.listener.bind(("127.0.0.1", 0))
        self.listener.listen(128)
        self.server = uvicorn.Server(uvicorn.Config(self.app, lifespan="off", log_level="warning", access_log=False))
        self.thread = threading.Thread(target=self.server.run, kwargs={"sockets": [self.listener]}, daemon=True)
        self.env = {"PORYGON_URL": f"http://127.0.0.1:{self.listener.getsockname()[1]}", "PORYGON_TOKEN": TOKEN}

    def __enter__(self):
        from terminal_support import wait
        self.thread.start()
        wait(lambda: self.server.started, "isolated daemon")
        return self

    def __exit__(self, *args):
        self.server.should_exit = True
        self.thread.join(10)
        self.listener.close()
