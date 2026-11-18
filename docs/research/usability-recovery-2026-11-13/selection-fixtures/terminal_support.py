"""No-build native terminal environment probe using frozen binaries.

Private Xvfb, XDG dirs, shell, tmux socket; never touches user's sessions.
"""
import base64
import json
import os
from pathlib import Path
import shlex
import signal
import subprocess
import tempfile
import time


ROOT = Path(os.environ.get("PORYGON_ARTIFACT_ROOT", str(Path(__file__).resolve().parent)))


def wait(fn, label, timeout=20):
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        result = fn()
        if result:
            return result
        time.sleep(.1)
    raise AssertionError(f"timeout: {label}")


class Terminal:
    def __init__(self, extra_env=None):
        self.out = ROOT / "receipts" / f"terminal-{time.time_ns()}"
        self.out.mkdir(parents=True)
        self.env = os.environ.copy()
        self.children = []
        self.logs = []
        self.extra_env = extra_env or {}

    def __enter__(self):
        for key in ("WAYLAND_DISPLAY", "ULTTY_SOCKET", "ULTTY_PID", "ULTTY_TTY", "TMUX", "STY"):
            self.env.pop(key, None)
        for key in ("RUNTIME", "CONFIG", "DATA", "STATE", "CACHE"):
            path = Path(tempfile.mkdtemp(prefix="ux-", dir="/tmp")) if key == "RUNTIME" else self.out / key.lower()
            if key != "RUNTIME":
                path.mkdir(mode=0o700)
            self.env[f"XDG_{key}_DIR" if key == "RUNTIME" else f"XDG_{key}_HOME"] = str(path)
        self.env.update(SHELL="/bin/sh", PS1="$ ")
        self.env.update(PORYGON_URL="http://127.0.0.1:9", PORYGON_TOKEN="isolated-fixture", PORYGON_WEATHER="0")
        self.env.update(self.extra_env)
        helpers = self.out / "bin"
        helpers.mkdir()
        opener = helpers / "xdg-open"
        opener.write_text("#!/bin/sh\nprintf '%s\\n' \"$1\" >> " + shlex.quote(str(self.out / "opened-links.txt")) + "\n")
        opener.chmod(0o755)
        self.env["PATH"] = str(helpers) + os.pathsep + self.env["PATH"]
        config = self.out / "config" / "ultty"
        config.mkdir()
        (config / "config.toml").write_text("[behavior]\naccessibility = true\ncursor_blink = false\n")
        log = (self.out / "xvfb.log").open("w")
        self.logs.append(log)
        xvfb = subprocess.Popen(["Xvfb", "-displayfd", "1", "-screen", "0", "1280x800x24", "-nolisten", "tcp"], stdout=subprocess.PIPE, stderr=log, text=True, start_new_session=True)
        self.children.append(xvfb)
        display = xvfb.stdout.readline().strip()
        assert display.isdigit(), self.out
        self.env["DISPLAY"] = f":{display}"
        lvp = next(iter(Path("/usr/share/vulkan/icd.d").glob("lvp_icd*.json")), None)
        if lvp:
            self.env["VK_ICD_FILENAMES"] = str(lvp)
        log = (self.out / "ultty.log").open("w")
        self.logs.append(log)
        self.proc = subprocess.Popen([os.environ.get("ULTTY_BINARY", "/tmp/porygon-compose-verification/ultty-v3-binary")], env=self.env, stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
        self.children.append(self.proc)
        self.env["ULTTY_PID"] = str(self.proc.pid)
        wait(lambda: (Path(self.env["XDG_RUNTIME_DIR"]) / f"ultty-ctl-{self.proc.pid}.sock").is_socket(), "control socket", 30)
        self.window = wait(self.find_window, "X11 window")
        self.ctl("resize", "1000", "650")
        return self

    def find_window(self):
        result = subprocess.run(["xdotool", "search", "--onlyvisible", "--pid", str(self.proc.pid)], env=self.env, capture_output=True, text=True)
        return result.stdout.splitlines()[0] if result.returncode == 0 and result.stdout.strip() else None

    def ctl(self, *args):
        result = subprocess.run([os.environ.get("ULTTYCTL_BINARY", "/tmp/porygon-compose-verification/ulttyctl-v3-binary"), *map(str, args)], env=self.env, capture_output=True, text=True, timeout=15)
        assert result.returncode == 0, (args, result.stdout, result.stderr)
        return result.stdout

    def shell(self, command):
        self.ctl("send-text", command)
        self.ctl("send-key", "enter")

    def key(self, chord):
        subprocess.run(["xdotool", "windowfocus", "--sync", self.window, "key", "--clearmodifiers", chord], env=self.env, check=True, timeout=10)

    def __exit__(self, *args):
        for process in reversed(self.children):
            if process.poll() is None:
                os.killpg(process.pid, signal.SIGTERM)
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    os.killpg(process.pid, signal.SIGKILL)
                    process.wait(timeout=5)
        for log in self.logs:
            log.close()


def run():
    with Terminal() as term:
        receipt = {"output": str(term.out), "claims": {}}
        try:
            payload = "clipboard-qual-alpha"
            b64 = base64.b64encode(payload.encode()).decode()
            term.shell(f"printf '\\033]52;c;{b64}\\007'; printf '\\nCLIPBOARD_READY\\n'")
            wait(lambda: "CLIPBOARD_READY" in term.ctl("text"), "clipboard setup")
            time.sleep(.3)
            copied = term.out / "clipboard.txt"
            term.shell(f"IFS= read -r pasted; printf '%s' \"$pasted\" > {shlex.quote(str(copied))}")
            time.sleep(.2)
            term.key("ctrl+shift+v")
            term.ctl("send-key", "enter")
            wait(copied.is_file, "clipboard pasted shell input")
            assert copied.read_text() == payload, copied.read_text()
            receipt["claims"]["osc52_system_clipboard_to_paste_exact"] = True
            # Two distinct processes and independent output in split panes.
            split = term.ctl("--json", "split", "right", "/bin/sh", "-c", "printf 'SPLIT_RIGHT_ONLY\\n'; sleep 60")
            (term.out / "split-response.json").write_text(split)
            term.ctl("screenshot", term.out / "split.png")
            (term.out / "panes.json").write_text(term.ctl("--json", "panes"))
            pane = json.loads(split)["pane"]
            wait(lambda: "SPLIT_RIGHT_ONLY" in term.ctl("--pane", pane, "text"), "split right independent output")
            receipt["claims"]["split_independent_shell_output"] = True
            term.ctl("new-tab", "/bin/sh", "-c", "printf 'https://qualification.invalid/alpha\\n'; sleep 60")
            wait(lambda: "https://qualification.invalid/alpha" in term.ctl("text"), "link line")
            term.ctl("screenshot", term.out / "link-before-click.png")
            (term.out / "link.text.txt").write_text(term.ctl("text"))
            subprocess.run(["xdotool", "windowfocus", "--sync", term.window, "mousemove", "--window", term.window, "150", "83", "keydown", "ctrl", "click", "1", "keyup", "ctrl"], env=term.env, check=True, timeout=10)
            opened = term.out / "opened-links.txt"
            wait(opened.is_file, "Ctrl-click URL invokes private opener")
            assert opened.read_text().splitlines() == ["https://qualification.invalid/alpha"], opened.read_text()
            term.ctl("screenshot", term.out / "link.png")
            receipt["claims"]["ctrl_click_exact_link_private_opener"] = True
            expected_selection = "alpha  beta   gamma"
            term.ctl("new-tab", "/bin/sh", "-c", f"printf '{expected_selection}\\n'; sleep 60")
            wait(lambda: expected_selection in term.ctl("text"), "selection source text")
            subprocess.run(["xdotool", "windowfocus", "--sync", term.window,
                            "mousemove", "--window", term.window, "100", "83",
                            "click", "--repeat", "3", "--delay", "80", "1"], env=term.env, check=True, timeout=10)
            selection = term.ctl("selection")
            (term.out / "selection.txt").write_text(selection)
            assert expected_selection in selection, repr(selection)
            term.key("ctrl+shift+c")
            selection_copy = term.out / "selection-copy.txt"
            term.ctl("new-tab", "/bin/sh", "-c", f"IFS= read -r line; printf '%s' \"$line\" > {shlex.quote(str(selection_copy))}; sleep 60")
            time.sleep(.2)
            term.key("ctrl+shift+v")
            term.ctl("send-key", "enter")
            wait(selection_copy.is_file, "copied selection pasted")
            assert selection_copy.read_text() == expected_selection, repr(selection_copy.read_text())
            receipt["claims"]["grid_selection_copy_internal_spaces_exact"] = True
            tmux_socket = str(Path(term.env["XDG_RUNTIME_DIR"]) / "tmux.sock")
            porygon = os.environ.get("PORYGON_NATIVE_TEST_BINARY", "/tmp/porygon-compose-verification/candidate-v5-binary")
            try:
                term.ctl("new-tab", "tmux", "-S", tmux_socket, "-f", "/dev/null", "new-session", porygon)
                wait(lambda: "Message Porygon" in term.ctl("text"), "tmux fallback composer")
                term.ctl("send-text", "tmux-draft-exact")
                wait(lambda: "tmux-draft-exact" in term.ctl("text"), "tmux fallback draft")
                regions = json.loads(term.ctl("--json", "regions"))
                (term.out / "tmux.regions.json").write_text(json.dumps(regions, indent=2))
                assert not regions.get("scenes"), regions
                (term.out / "tmux.text.txt").write_text(term.ctl("text"))
                term.ctl("screenshot", term.out / "tmux.png")
                receipt["claims"]["tmux_ansi_fallback_composer_and_draft"] = True
            finally:
                subprocess.run(["tmux", "-S", tmux_socket, "kill-server"], capture_output=True)
        finally:
            (term.out / "receipt.json").write_text(json.dumps(receipt, indent=2))
            print(json.dumps(receipt), flush=True)


if __name__ == "__main__":
    run()
