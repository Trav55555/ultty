#!/usr/bin/env python3
"""Exercise bundle validation, activation and an optional real host lifecycle."""

import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import time
import unittest

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("session_bundle", HERE / "session-bundle.py")
bundle = importlib.util.module_from_spec(spec)
spec.loader.exec_module(bundle)
TEMPLATE = HERE.parents[1] / "dist/systemd" / bundle.TEMPLATE


class BundleTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix="ultty-package-test-")
        self.root = Path(self.tmp.name)
        self.bins = self.root / "bins"
        self.bins.mkdir()
        for name in bundle.PROFILES["full"]:
            (self.bins / name).write_text("#!/bin/sh\nexit 0\n")
        self.prefix = self.root / "install"

    def tearDown(self):
        self.tmp.cleanup()

    def make(self, name, profile="full"):
        path = self.root / name
        bundle.create_bundle(path, name, profile, "a" * 40, "test-target", "rustc test", self.bins, TEMPLATE)
        return path

    def test_install_activate_and_rollback(self):
        first, second = self.make("one"), self.make("two")
        one = bundle.install(first, self.prefix)
        self.assertFalse((self.prefix / "current").exists())
        bundle.activate(self.prefix, one)
        self.assertEqual(os.readlink(self.prefix / "current"), f"versions/{one}")
        two = bundle.install(second, self.prefix)
        bundle.activate(self.prefix, two)
        self.assertEqual(os.readlink(self.prefix / "current"), f"versions/{two}")
        bundle.activate(self.prefix, one)
        self.assertEqual(os.readlink(self.prefix / "current"), f"versions/{one}")
        with self.assertRaisesRegex(ValueError, "already exists"):
            bundle.install(first, self.prefix)

    def test_reject_tampering_and_untrusted_paths(self):
        path = self.make("one")
        (path / "bin/ultty").write_text("changed")
        with self.assertRaisesRegex(ValueError, "checksum mismatch"):
            bundle.install(path, self.prefix)
        self.assertFalse((self.prefix / "current").exists())
        with self.assertRaises(ValueError):
            bundle.activate(self.prefix, "../outside")
        path = self.make("two")
        (path / "bin/ultty").unlink()
        (path / "bin/ultty").symlink_to(self.bins / "ultty")
        with self.assertRaisesRegex(ValueError, "regular"):
            bundle.verify(path)
        manifest = json.loads((path / "manifest.json").read_text())
        manifest["files"]["../outside"] = "a" * 64
        (path / "manifest.json").write_text(json.dumps(manifest))
        with self.assertRaisesRegex(ValueError, "file list"):
            bundle.verify(path)

    def test_headless_profile_and_existing_current_file(self):
        path = self.make("host", "headless")
        self.assertEqual(bundle.verify(path)["profile"], "headless")
        release = bundle.install(path, self.prefix)
        (self.prefix / "current").write_text("do not replace")
        with self.assertRaisesRegex(ValueError, "not a symlink"):
            bundle.activate(self.prefix, release)
        self.assertEqual((self.prefix / "current").read_text(), "do not replace")

    def test_activation_rechecks_installed_bytes(self):
        release = bundle.install(self.make("one"), self.prefix)
        bundle.activate(self.prefix, release)
        next_release = bundle.install(self.make("two"), self.prefix)
        (self.prefix / "versions" / next_release / "bin/ultty").write_text("corrupt")
        with self.assertRaisesRegex(ValueError, "checksum mismatch"):
            bundle.activate(self.prefix, next_release)
        self.assertEqual(os.readlink(self.prefix / "current"), f"versions/{release}")

    def test_rejects_non_executable_binary(self):
        path = self.make("one")
        (path / "bin/ultty-session").chmod(0o644)
        with self.assertRaisesRegex(ValueError, "not executable"):
            bundle.install(path, self.prefix)

    def test_service_template_uses_user_runtime_and_does_not_restart(self):
        unit = TEMPLATE.read_text()
        self.assertIn("Environment=XDG_RUNTIME_DIR=%t\n", unit)
        self.assertIn("ExecStart=%h/.local/opt/ultty/current/bin/ultty-session host %i\n", unit)
        self.assertIn("Restart=no\n", unit)

    def test_rejects_symlinked_versions_directory(self):
        path = self.make("one")
        self.prefix.mkdir()
        (self.prefix / "versions").symlink_to(self.bins, target_is_directory=True)
        with self.assertRaisesRegex(ValueError, "versions directory"):
            bundle.install(path, self.prefix)

    def test_build_command_profiles_and_dirty_checkout(self):
        repo = self.root / "checkout"
        scripts = repo / "scripts/deploy"
        scripts.mkdir(parents=True)
        shutil.copyfile(HERE / "session-bundle.py", scripts / "session-bundle.py")
        (repo / "dist/systemd").mkdir(parents=True)
        shutil.copyfile(TEMPLATE, repo / "dist/systemd" / bundle.TEMPLATE)
        tools = self.root / "tools"
        tools.mkdir()
        git = tools / "git"
        git.write_text("#!/bin/sh\ncase \"$3\" in status) if [ -n \"$DIRTY_TEST\" ] || [ -f \"$DIRTY_FILE\" ]; then echo dirty; fi;; rev-parse) printf '%040d\\n' 1;; esac\n")
        git.chmod(0o755)
        cargo = tools / "cargo"
        cargo.write_text("#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$BUILD_ARGS\"\nwhile [ $# -gt 0 ]; do case \"$1\" in --target) shift; target=$1; mkdir -p \"$CARGO_TARGET_DIR/$target/release\";; --bin) shift; printf '#!/bin/sh\\nexit 0\\n' > \"$CARGO_TARGET_DIR/$target/release/$1\";; esac; shift; done\nif [ -n \"$TOUCH_AFTER_BUILD\" ]; then touch \"$DIRTY_FILE\"; fi\n")
        cargo.chmod(0o755)
        env = dict(os.environ, PATH=f"{tools}:{os.environ['PATH']}",
                   BUILD_ARGS=str(self.root / "build-args"), DIRTY_TEST="",
                   DIRTY_FILE=str(self.root / "became-dirty"), TOUCH_AFTER_BUILD="")
        for profile in ("headless", "full"):
            out = self.root / f"built-{profile}"
            command = ["python3", str(scripts / "session-bundle.py"), "build", "--profile", profile,
                       "--version", "test", "--output", str(out), "--target-dir", str(self.root / f"target-{profile}")]
            subprocess.run(command, env=env, capture_output=True, check=True)
            self.assertEqual(bundle.verify(out)["profile"], profile)
            flags = (self.root / "build-args").read_text().splitlines()
            target = bundle.verify(out)["target_triple"]
            self.assertEqual(flags[flags.index("--target") + 1], target)
            self.assertEqual("--no-default-features" in flags, profile == "headless")
            bins = [flags[i + 1] for i, flag in enumerate(flags) if flag == "--bin"]
            self.assertEqual(set(bins), set(bundle.PROFILES[profile]))
        env["DIRTY_TEST"] = "1"
        command[command.index(str(out))] = str(self.root / "dirty-output")
        result = subprocess.run(command, env=env, capture_output=True, text=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("uncommitted changes", result.stderr)
        self.assertFalse((self.root / "dirty-output").exists())
        env["DIRTY_TEST"] = ""
        env["TOUCH_AFTER_BUILD"] = "1"
        command[command.index(str(self.root / "dirty-output"))] = str(self.root / "changed-output")
        command[command.index(str(self.root / "target-full"))] = str(self.root / "target-changed")
        result = subprocess.run(command, env=env, capture_output=True, text=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("uncommitted changes", result.stderr)
        self.assertFalse((self.root / "changed-output").exists())

    @unittest.skipUnless(os.environ.get("ULTTY_DEPLOY_TEST_BIN_DIR"), "real host binaries not supplied")
    def test_real_host_lifecycle_in_disposable_runtime(self):
        binaries = Path(os.environ["ULTTY_DEPLOY_TEST_BIN_DIR"])
        source = self.root / "real"
        bundle.create_bundle(source, "real", "headless", "b" * 40,
                             "test-target", "rustc test", binaries, TEMPLATE)
        release = bundle.install(source, self.prefix)
        bundle.activate(self.prefix, release)
        executable = self.prefix / "current/bin/ultty-session"
        # Unix socket paths have a small platform limit; keep this root short.
        runtime_dir = tempfile.TemporaryDirectory(prefix="ultty-deploy-", dir="/tmp")
        self.addCleanup(runtime_dir.cleanup)
        runtime = Path(runtime_dir.name)
        env = dict(os.environ, XDG_RUNTIME_DIR=str(runtime), TMPDIR=str(runtime))
        host = subprocess.Popen([str(executable), "host", "package-test", "-e", "/bin/sh", "-c", "sleep 60"],
                                env=env, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        try:
            for _ in range(100):
                if (runtime / "ultty-session-package-test.admin").exists():
                    break
                if host.poll() is not None:
                    self.fail(f"host exited: {host.stderr.read().decode()}")
                time.sleep(0.05)
            result = subprocess.run([str(executable), "snapshot", "package-test"], env=env,
                                    capture_output=True, text=True, check=True, timeout=10)
            snapshot = json.loads(result.stdout)["reply"]["snapshot"]
            self.assertEqual(snapshot["owner"]["pid"], host.pid)
            child = snapshot["panes"][0]["pid"]
            subprocess.run([str(executable), "stop", "package-test"], env=env,
                           capture_output=True, check=True, timeout=10)
            self.assertEqual(host.wait(timeout=10), 0)
            with self.assertRaises(ProcessLookupError):
                os.kill(child, 0)
        finally:
            if host.poll() is None:
                host.terminate()
                host.wait(timeout=10)
            host.stderr.close()


if __name__ == "__main__":
    unittest.main()
