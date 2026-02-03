#!/usr/bin/env python3
"""
Visual testing harness for ultty terminal emulator.

This module provides infrastructure for automated visual testing:
- Xvfb virtual display management
- ultty process lifecycle
- ulttyctl command execution
- Screenshot capture and comparison
"""

import json
import os
import signal
import socket
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Optional, List, Dict, Any

try:
    from PIL import Image, ImageChops

    HAS_PIL = True
except ImportError:
    HAS_PIL = False


@dataclass
class ComparisonResult:
    """Result of comparing two images."""

    matches: bool
    diff_pixels: int
    diff_percent: float
    diff_image_path: Optional[Path] = None


class UlttyHarness:
    """
    Test harness for visual testing of ultty terminal emulator.

    Usage:
        with UlttyHarness() as harness:
            harness.wait_ready()
            harness.send_text("echo hello")
            harness.send_key("enter")
            harness.wait_for("hello")
            harness.screenshot("/tmp/test.png")
    """

    def __init__(
        self,
        width: int = 1280,
        height: int = 720,
        display: int = 99,
        build_release: bool = True,
        project_root: Optional[Path] = None,
    ):
        self.width = width
        self.height = height
        self.display = display
        self.build_release = build_release
        self.project_root = project_root or Path(__file__).parent.parent.parent

        self.xvfb_proc: Optional[subprocess.Popen] = None
        self.ultty_proc: Optional[subprocess.Popen] = None
        self.socket_path: Optional[Path] = None
        self._original_display = os.environ.get("DISPLAY")

    def __enter__(self):
        self.start()
        return self

    def __exit__(self, exc_type, exc_val, exc_tb):
        self.stop()
        return False

    def start(self):
        """Start Xvfb and ultty."""
        self._start_xvfb()
        self._build_ultty()
        self._start_ultty()

    def stop(self):
        """Stop ultty and Xvfb."""
        self._stop_ultty()
        self._stop_xvfb()

    def _start_xvfb(self):
        """Start Xvfb virtual framebuffer."""
        display_str = f":{self.display}"

        # Check if display is already in use
        lock_file = Path(f"/tmp/.X{self.display}-lock")
        if lock_file.exists():
            print(f"Display {display_str} appears to be in use, trying to clean up...")
            try:
                lock_file.unlink()
            except:
                pass

        cmd = [
            "Xvfb",
            display_str,
            "-screen",
            "0",
            f"{self.width}x{self.height}x24",
            "-ac",  # Disable access control
        ]

        print(f"Starting Xvfb on display {display_str}...")
        self.xvfb_proc = subprocess.Popen(
            cmd,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )

        # Wait for Xvfb to be ready
        time.sleep(0.5)
        if self.xvfb_proc.poll() is not None:
            raise RuntimeError(
                f"Xvfb failed to start (exit code: {self.xvfb_proc.returncode})"
            )

        os.environ["DISPLAY"] = display_str
        print(f"Xvfb started (PID: {self.xvfb_proc.pid})")

    def _stop_xvfb(self):
        """Stop Xvfb."""
        if self.xvfb_proc:
            print("Stopping Xvfb...")
            self.xvfb_proc.terminate()
            try:
                self.xvfb_proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.xvfb_proc.kill()
            self.xvfb_proc = None

        # Restore original display
        if self._original_display:
            os.environ["DISPLAY"] = self._original_display
        elif "DISPLAY" in os.environ:
            del os.environ["DISPLAY"]

    def _build_ultty(self):
        """Build ultty if needed."""
        binary_name = "ultty"
        profile = "release" if self.build_release else "debug"
        binary_path = self.project_root / "target" / profile / binary_name

        # Check if binary exists and is recent
        if binary_path.exists():
            # Check if source is newer than binary
            src_dir = self.project_root / "crates" / "ultty" / "src"
            newest_src = max(
                (f.stat().st_mtime for f in src_dir.rglob("*.rs")), default=0
            )
            if binary_path.stat().st_mtime >= newest_src:
                print(f"Using existing binary: {binary_path}")
                return

        print(f"Building ultty ({profile})...")
        cmd = ["cargo", "build", "-p", "ultty"]
        if self.build_release:
            cmd.append("--release")

        result = subprocess.run(
            cmd,
            cwd=self.project_root,
            capture_output=True,
            text=True,
        )

        if result.returncode != 0:
            print(f"Build failed:\n{result.stderr}")
            raise RuntimeError("Failed to build ultty")

        print("Build complete")

    def _start_ultty(self):
        """Start ultty terminal emulator."""
        profile = "release" if self.build_release else "debug"
        binary_path = self.project_root / "target" / profile / "ultty"

        if not binary_path.exists():
            raise RuntimeError(f"Binary not found: {binary_path}")

        # Determine socket path
        runtime_dir = os.environ.get("XDG_RUNTIME_DIR", "/tmp")
        self.socket_path = Path(runtime_dir) / "ultty-ctl.sock"

        # Remove stale socket
        if self.socket_path.exists():
            self.socket_path.unlink()

        print(f"Starting ultty...")
        self.ultty_proc = subprocess.Popen(
            [str(binary_path)],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            env={**os.environ, "RUST_LOG": "ultty=debug"},
        )

        print(f"ultty started (PID: {self.ultty_proc.pid})")

    def _stop_ultty(self):
        """Stop ultty."""
        if self.ultty_proc:
            print("Stopping ultty...")
            self.ultty_proc.terminate()
            try:
                self.ultty_proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.ultty_proc.kill()
            self.ultty_proc = None

    def wait_ready(self, timeout: float = 30.0) -> bool:
        """Wait for ultty to be ready (control socket available)."""
        print(f"Waiting for ultty to be ready (socket: {self.socket_path})...")
        start = time.time()

        while time.time() - start < timeout:
            if self.socket_path and self.socket_path.exists():
                # Try to connect
                try:
                    result = self._send_command("ping")
                    if "PONG" in result or "pong" in result.lower():
                        print("ultty is ready")
                        return True
                except:
                    pass
            time.sleep(0.1)

        raise TimeoutError(f"ultty did not become ready within {timeout}s")

    def _send_command(self, command: str) -> str:
        """Send a command to ultty via control socket."""
        if not self.socket_path or not self.socket_path.exists():
            raise RuntimeError("Control socket not available")

        sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        sock.settimeout(10.0)

        try:
            sock.connect(str(self.socket_path))
            sock.sendall(command.encode("utf-8"))
            sock.shutdown(socket.SHUT_WR)

            response = b""
            while True:
                chunk = sock.recv(4096)
                if not chunk:
                    break
                response += chunk

            return response.decode("utf-8")
        finally:
            sock.close()

    def _send_json_command(self, request: Dict[str, Any]) -> Dict[str, Any]:
        """Send a JSON command and parse JSON response."""
        response = self._send_command(json.dumps(request))
        return json.loads(response)

    def send_text(self, text: str) -> str:
        """Send text to the terminal."""
        return self._send_command(f"send-text {text}")

    def send_key(self, key: str, modifiers: Optional[str] = None) -> str:
        """Send a key press to the terminal."""
        cmd = f"send-key {key}"
        if modifiers:
            cmd += f" {modifiers}"
        return self._send_command(cmd)

    def get_info(self) -> Dict[str, Any]:
        """Get terminal info."""
        return self._send_json_command({"type": "get_info"})

    def get_text(self, include_scrollback: bool = False) -> str:
        """Get terminal text content."""
        cmd = "text"
        if include_scrollback:
            cmd += " --scrollback"
        return self._send_command(cmd)

    def search(self, pattern: str, regex: bool = False) -> str:
        """Search for pattern in terminal."""
        cmd = f"search {pattern}"
        if regex:
            cmd = f"search --regex {pattern}"
        return self._send_command(cmd)

    def wait_for(
        self, pattern: str, timeout_ms: int = 30000, regex: bool = False
    ) -> bool:
        """Wait for pattern to appear in terminal."""
        request = {
            "type": "wait_for",
            "pattern": pattern,
            "regex": regex,
            "timeout_ms": timeout_ms,
        }
        response = self._send_json_command(request)
        return response.get("type") == "wait_matched"

    def screenshot(self, path: Optional[str] = None) -> Optional[bytes]:
        """
        Capture a screenshot.

        If path is provided, saves to file and returns None.
        If path is None, returns PNG bytes.
        """
        if path:
            response = self._send_command(f"screenshot {path}")
            return None
        else:
            request = {"type": "screenshot"}
            response = self._send_json_command(request)
            if response.get("type") == "screenshot_data":
                import base64

                return base64.b64decode(response["data"])
            return None

    def screenshot_to_file(self, path: Path) -> bool:
        """Capture screenshot and save to file."""
        response = self._send_command(f"screenshot {path}")
        return "saved" in response.lower() or "screenshot" in response.lower()


def compare_images(
    actual: Path,
    expected: Path,
    threshold: float = 0.01,
    diff_output: Optional[Path] = None,
) -> ComparisonResult:
    """
    Compare two images and return the result.

    Args:
        actual: Path to the actual (test) image
        expected: Path to the expected (golden) image
        threshold: Maximum allowed difference (0.0-1.0)
        diff_output: Optional path to save diff image

    Returns:
        ComparisonResult with match status and difference metrics
    """
    if not HAS_PIL:
        raise RuntimeError(
            "Pillow is required for image comparison: pip install Pillow"
        )

    img_actual = Image.open(actual).convert("RGB")
    img_expected = Image.open(expected).convert("RGB")

    if img_actual.size != img_expected.size:
        return ComparisonResult(
            matches=False,
            diff_pixels=-1,
            diff_percent=100.0,
        )

    diff = ImageChops.difference(img_actual, img_expected)

    # Count non-zero pixels
    diff_data = list(diff.getdata())
    diff_pixels = sum(1 for pixel in diff_data if any(c > 0 for c in pixel))
    total_pixels = img_actual.width * img_actual.height
    diff_percent = (diff_pixels / total_pixels) * 100

    # Save diff image if requested
    if diff_output and diff_pixels > 0:
        # Amplify differences for visibility
        diff_amplified = diff.point(lambda x: min(255, x * 10))
        diff_amplified.save(diff_output)

    return ComparisonResult(
        matches=diff_percent <= threshold * 100,
        diff_pixels=diff_pixels,
        diff_percent=diff_percent,
        diff_image_path=diff_output if diff_pixels > 0 else None,
    )


def run_visual_test(
    test_name: str,
    commands: List[tuple],
    golden_dir: Path,
    output_dir: Path,
    harness: Optional[UlttyHarness] = None,
    threshold: float = 0.01,
) -> bool:
    """
    Run a visual test.

    Args:
        test_name: Name of the test (used for golden image filename)
        commands: List of (method_name, args) tuples to execute
        golden_dir: Directory containing golden images
        output_dir: Directory to save test outputs
        harness: Optional existing harness (creates new one if None)
        threshold: Maximum allowed image difference

    Returns:
        True if test passed, False otherwise
    """
    output_dir.mkdir(parents=True, exist_ok=True)

    own_harness = harness is None
    if own_harness:
        harness = UlttyHarness()
        harness.start()
        harness.wait_ready()

    try:
        # Execute commands
        for method_name, args in commands:
            method = getattr(harness, method_name)
            if isinstance(args, dict):
                method(**args)
            elif isinstance(args, (list, tuple)):
                method(*args)
            else:
                method(args)

        # Small delay for rendering to complete
        time.sleep(0.2)

        # Capture screenshot
        actual_path = output_dir / f"{test_name}.png"
        harness.screenshot_to_file(actual_path)

        # Compare with golden
        golden_path = golden_dir / f"{test_name}.png"
        if not golden_path.exists():
            print(f"WARNING: Golden image not found: {golden_path}")
            print(f"  Actual image saved to: {actual_path}")
            print(f"  To accept as golden: cp {actual_path} {golden_path}")
            return False

        diff_path = output_dir / f"{test_name}_diff.png"
        result = compare_images(actual_path, golden_path, threshold, diff_path)

        if result.matches:
            print(f"PASS: {test_name} (diff: {result.diff_percent:.2f}%)")
            return True
        else:
            print(
                f"FAIL: {test_name} (diff: {result.diff_percent:.2f}%, {result.diff_pixels} pixels)"
            )
            if result.diff_image_path:
                print(f"  Diff image: {result.diff_image_path}")
            return False

    finally:
        if own_harness:
            harness.stop()


if __name__ == "__main__":
    # Quick self-test
    print("Visual test harness self-test")
    print("=" * 40)

    with UlttyHarness() as harness:
        harness.wait_ready()

        # Basic tests
        print("\nTesting send_text...")
        harness.send_text("echo 'Hello from visual test'")
        harness.send_key("enter")
        time.sleep(0.5)

        print("\nTesting get_text...")
        text = harness.get_text()
        print(f"Terminal text (first 200 chars): {text[:200]}...")

        print("\nTesting screenshot...")
        output_dir = Path("/tmp/ultty-visual-test")
        output_dir.mkdir(exist_ok=True)
        screenshot_path = output_dir / "self_test.png"
        harness.screenshot_to_file(screenshot_path)
        print(f"Screenshot saved to: {screenshot_path}")

        print("\nSelf-test complete!")
