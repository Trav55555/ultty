#!/usr/bin/env python3
"""
Visual tests for Claude Code rendering in ultty.

These tests verify that Claude Code renders correctly in the ultty terminal.
"""

import sys
import time
from pathlib import Path

# Add parent to path for imports
sys.path.insert(0, str(Path(__file__).parent))

from harness import UlttyHarness, compare_images, run_visual_test


GOLDEN_DIR = Path(__file__).parent / "golden"
OUTPUT_DIR = Path("/tmp/ultty-visual-tests")


def test_basic_shell_prompt():
    """Test that a basic shell prompt renders correctly."""
    with UlttyHarness() as harness:
        harness.wait_ready()

        # Wait for shell to be ready
        time.sleep(1.0)

        # Capture initial shell state
        OUTPUT_DIR.mkdir(parents=True, exist_ok=True)
        screenshot_path = OUTPUT_DIR / "shell_prompt.png"
        harness.screenshot_to_file(screenshot_path)

        golden_path = GOLDEN_DIR / "shell_prompt.png"
        if not golden_path.exists():
            print(f"Golden image not found: {golden_path}")
            print(f"To create golden: cp {screenshot_path} {golden_path}")
            return False

        result = compare_images(screenshot_path, golden_path, threshold=0.05)
        print(
            f"Shell prompt test: {'PASS' if result.matches else 'FAIL'} ({result.diff_percent:.2f}% diff)"
        )
        return result.matches


def test_colors_and_formatting():
    """Test that ANSI colors and formatting render correctly."""
    with UlttyHarness() as harness:
        harness.wait_ready()
        time.sleep(0.5)

        # Print colored text
        harness.send_text(r"printf '\e[31mRed\e[0m \e[32mGreen\e[0m \e[34mBlue\e[0m\n'")
        harness.send_key("enter")
        time.sleep(0.3)

        # Print bold and italic
        harness.send_text(
            r"printf '\e[1mBold\e[0m \e[3mItalic\e[0m \e[4mUnderline\e[0m\n'"
        )
        harness.send_key("enter")
        time.sleep(0.3)

        # Print 256 color sample
        harness.send_text(
            r"for i in {0..7}; do printf '\e[48;5;${i}m  '; done; printf '\e[0m\n'"
        )
        harness.send_key("enter")
        time.sleep(0.5)

        OUTPUT_DIR.mkdir(parents=True, exist_ok=True)
        screenshot_path = OUTPUT_DIR / "colors_formatting.png"
        harness.screenshot_to_file(screenshot_path)

        golden_path = GOLDEN_DIR / "colors_formatting.png"
        if not golden_path.exists():
            print(f"Golden image not found: {golden_path}")
            print(f"To create golden: cp {screenshot_path} {golden_path}")
            return False

        result = compare_images(screenshot_path, golden_path, threshold=0.02)
        print(
            f"Colors test: {'PASS' if result.matches else 'FAIL'} ({result.diff_percent:.2f}% diff)"
        )
        return result.matches


def test_claude_code_launch():
    """Test that Claude Code launches and displays correctly."""
    with UlttyHarness() as harness:
        harness.wait_ready()
        time.sleep(0.5)

        # Launch claude
        harness.send_text("claude")
        harness.send_key("enter")

        print("Waiting for Claude Code to start...")
        start = time.time()
        timeout = 30

        while time.time() - start < timeout:
            try:
                text = harness.get_text()
                if "Claude" in text or ">" in text or "claude" in text.lower():
                    print("Claude Code detected in terminal output")
                    break
            except Exception as e:
                print(f"Warning: get_text failed: {e}")
            time.sleep(1.0)
        else:
            print(f"Timeout waiting for Claude Code (waited {timeout}s)")

        time.sleep(2.0)

        OUTPUT_DIR.mkdir(parents=True, exist_ok=True)
        screenshot_path = OUTPUT_DIR / "claude_code_launch.png"

        try:
            harness.screenshot_to_file(screenshot_path)
        except Exception as e:
            print(f"Screenshot failed: {e}")
            try:
                harness.send_key("c", "ctrl")
            except:
                pass
            return False

        golden_path = GOLDEN_DIR / "claude_code_launch.png"
        if not golden_path.exists():
            print(f"Golden image not found: {golden_path}")
            print(f"To create golden: cp {screenshot_path} {golden_path}")
            try:
                harness.send_key("c", "ctrl")
                time.sleep(0.5)
            except:
                pass
            return False

        result = compare_images(screenshot_path, golden_path, threshold=0.05)
        print(
            f"Claude Code launch test: {'PASS' if result.matches else 'FAIL'} ({result.diff_percent:.2f}% diff)"
        )

        # Exit claude
        try:
            harness.send_key("c", "ctrl")
            time.sleep(0.5)
        except:
            pass

        return result.matches


def test_claude_code_input():
    """Test that Claude Code accepts and displays input correctly."""
    with UlttyHarness() as harness:
        harness.wait_ready()
        time.sleep(0.5)

        harness.send_text("claude")
        harness.send_key("enter")

        print("Waiting for Claude Code to start...")
        start = time.time()
        timeout = 30

        while time.time() - start < timeout:
            try:
                text = harness.get_text()
                if "Claude" in text or ">" in text:
                    break
            except:
                pass
            time.sleep(1.0)

        time.sleep(2.0)

        test_input = "Hello, this is a test message"
        harness.send_text(test_input)
        time.sleep(0.5)

        OUTPUT_DIR.mkdir(parents=True, exist_ok=True)
        screenshot_path = OUTPUT_DIR / "claude_code_input.png"

        try:
            harness.screenshot_to_file(screenshot_path)
        except Exception as e:
            print(f"Screenshot failed: {e}")
            try:
                harness.send_key("c", "ctrl")
            except:
                pass
            return False

        golden_path = GOLDEN_DIR / "claude_code_input.png"
        if not golden_path.exists():
            print(f"Golden image not found: {golden_path}")
            print(f"To create golden: cp {screenshot_path} {golden_path}")
            try:
                harness.send_key("c", "ctrl")
                time.sleep(0.5)
            except:
                pass
            return False

        result = compare_images(screenshot_path, golden_path, threshold=0.05)
        print(
            f"Claude Code input test: {'PASS' if result.matches else 'FAIL'} ({result.diff_percent:.2f}% diff)"
        )

        try:
            harness.send_key("c", "ctrl")
            time.sleep(0.5)
        except:
            pass

        return result.matches


def test_unicode_rendering():
    """Test that Unicode characters render correctly."""
    with UlttyHarness() as harness:
        harness.wait_ready()
        time.sleep(0.5)

        # Print various Unicode characters
        harness.send_text("echo '日本語 中文 한국어'")
        harness.send_key("enter")
        time.sleep(0.3)

        harness.send_text("echo '→ ← ↑ ↓ ★ ☆ ♠ ♣ ♥ ♦'")
        harness.send_key("enter")
        time.sleep(0.3)

        harness.send_text("echo '│ ─ ┌ ┐ └ ┘ ├ ┤ ┬ ┴ ┼'")
        harness.send_key("enter")
        time.sleep(0.3)

        harness.send_text("echo '😀 🎉 🚀 ✨ 💻'")
        harness.send_key("enter")
        time.sleep(0.5)

        OUTPUT_DIR.mkdir(parents=True, exist_ok=True)
        screenshot_path = OUTPUT_DIR / "unicode_rendering.png"
        harness.screenshot_to_file(screenshot_path)

        golden_path = GOLDEN_DIR / "unicode_rendering.png"
        if not golden_path.exists():
            print(f"Golden image not found: {golden_path}")
            print(f"To create golden: cp {screenshot_path} {golden_path}")
            return False

        result = compare_images(screenshot_path, golden_path, threshold=0.02)
        print(
            f"Unicode test: {'PASS' if result.matches else 'FAIL'} ({result.diff_percent:.2f}% diff)"
        )
        return result.matches


def run_all_tests():
    """Run all visual tests."""
    print("=" * 60)
    print("Ultty Visual Tests - Claude Code Rendering")
    print("=" * 60)
    print()

    tests = [
        ("Basic Shell Prompt", test_basic_shell_prompt),
        ("Colors and Formatting", test_colors_and_formatting),
        ("Unicode Rendering", test_unicode_rendering),
        ("Claude Code Launch", test_claude_code_launch),
        ("Claude Code Input", test_claude_code_input),
    ]

    results = []
    for name, test_fn in tests:
        print(f"\nRunning: {name}")
        print("-" * 40)
        try:
            passed = test_fn()
            results.append((name, passed, None))
        except Exception as e:
            print(f"ERROR: {e}")
            results.append((name, False, str(e)))

    print("\n")
    print("=" * 60)
    print("SUMMARY")
    print("=" * 60)

    passed = sum(1 for _, p, _ in results if p)
    failed = sum(1 for _, p, _ in results if not p)

    for name, success, error in results:
        status = "PASS" if success else "FAIL"
        print(f"  [{status}] {name}")
        if error:
            print(f"         Error: {error}")

    print()
    print(f"Total: {passed} passed, {failed} failed")
    print()

    return failed == 0


if __name__ == "__main__":
    import argparse

    parser = argparse.ArgumentParser(
        description="Run visual tests for Claude Code in ultty"
    )
    parser.add_argument("--test", help="Run a specific test")
    parser.add_argument("--list", action="store_true", help="List available tests")
    parser.add_argument(
        "--update-golden",
        action="store_true",
        help="Update golden images from current output",
    )
    args = parser.parse_args()

    if args.list:
        print("Available tests:")
        print("  - shell_prompt")
        print("  - colors_formatting")
        print("  - unicode_rendering")
        print("  - claude_code_launch")
        print("  - claude_code_input")
        sys.exit(0)

    if args.update_golden:
        print("Updating golden images from output directory...")
        GOLDEN_DIR.mkdir(parents=True, exist_ok=True)
        import shutil

        for png in OUTPUT_DIR.glob("*.png"):
            if not png.name.endswith("_diff.png"):
                dest = GOLDEN_DIR / png.name
                shutil.copy(png, dest)
                print(f"  Updated: {dest}")
        sys.exit(0)

    if args.test:
        test_map = {
            "shell_prompt": test_basic_shell_prompt,
            "colors_formatting": test_colors_and_formatting,
            "unicode_rendering": test_unicode_rendering,
            "claude_code_launch": test_claude_code_launch,
            "claude_code_input": test_claude_code_input,
        }
        if args.test not in test_map:
            print(f"Unknown test: {args.test}")
            print(f"Available: {', '.join(test_map.keys())}")
            sys.exit(1)
        success = test_map[args.test]()
        sys.exit(0 if success else 1)

    success = run_all_tests()
    sys.exit(0 if success else 1)
