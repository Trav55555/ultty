#!/usr/bin/env python3
"""Build and install versioned Ultty session bundles without starting services."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shutil
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[2]
TEMPLATE = "ultty-session@.service"
PROFILES = {"full": ("ultty", "ultty-session", "ulttyctl"),
            "headless": ("ultty-session", "ulttyctl")}
IDENTIFIER = re.compile(r"[A-Za-z0-9][A-Za-z0-9._-]{0,63}\Z")
RELEASE = re.compile(r"[A-Za-z0-9][A-Za-z0-9._-]{0,127}\Z")


def digest(path):
    hasher = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            hasher.update(chunk)
    return hasher.hexdigest()


def identifier(value):
    if not isinstance(value, str) or not IDENTIFIER.fullmatch(value):
        raise ValueError("version must be 1–64 ASCII letters, digits, dots, underscores or hyphens")
    return value


def platform_name():
    return f"{sys.platform}-{platform.machine()}"


def git(*args):
    return subprocess.check_output(["git", "-C", str(ROOT), *args], text=True).strip()


def source_revision():
    if git("status", "--porcelain", "--untracked-files=normal"):
        raise ValueError("source has uncommitted changes; commit it before building a release")
    return git("rev-parse", "HEAD")


def create_bundle(output, version, profile, revision, target_triple, rustc_version, binaries, template):
    """Create a bundle from one build; callers supply the exact binary paths."""
    identifier(version)
    output = Path(output)
    if output.exists() or output.is_symlink():
        raise ValueError(f"bundle destination already exists: {output}")
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=".ultty-bundle-", dir=output.parent) as tmp:
        bundle = Path(tmp) / "bundle"
        (bundle / "bin").mkdir(parents=True)
        (bundle / "systemd").mkdir()
        files = {}
        for name in PROFILES[profile]:
            dest = bundle / "bin" / name
            shutil.copyfile(binaries / name, dest)
            dest.chmod(0o755)
            files[f"bin/{name}"] = digest(dest)
        dest = bundle / "systemd" / TEMPLATE
        shutil.copyfile(template, dest)
        dest.chmod(0o644)
        files[f"systemd/{TEMPLATE}"] = digest(dest)
        manifest = {"format": 1, "version": version, "profile": profile,
                    "platform": platform_name(), "source_revision": revision,
                    "target_triple": target_triple, "rustc_version": rustc_version,
                    "files": files}
        (bundle / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
        output.mkdir()
        try:
            shutil.copytree(bundle, output, dirs_exist_ok=True)
            verify(output)
        except BaseException:
            shutil.rmtree(output)
            raise
    return manifest


def verify(bundle):
    bundle = Path(bundle)
    if bundle.is_symlink() or not bundle.is_dir():
        raise ValueError("bundle must be a real directory")
    manifest_path = bundle / "manifest.json"
    if manifest_path.is_symlink() or not manifest_path.is_file():
        raise ValueError("manifest must be a regular file")
    manifest = json.loads(manifest_path.read_text())
    if not isinstance(manifest, dict):
        raise ValueError("manifest must be an object")
    profile = manifest.get("profile")
    if manifest.get("format") != 1 or not isinstance(profile, str) or profile not in PROFILES:
        raise ValueError("unsupported bundle format or profile")
    identifier(manifest.get("version", ""))
    if manifest.get("platform") != platform_name():
        raise ValueError("bundle platform does not match this machine")
    revision = manifest.get("source_revision")
    if not isinstance(revision, str) or not re.fullmatch(r"[0-9a-f]{40}", revision):
        raise ValueError("invalid source revision")
    target = manifest.get("target_triple")
    rustc = manifest.get("rustc_version")
    if not isinstance(target, str) or not re.fullmatch(r"[A-Za-z0-9_.-]{1,128}", target):
        raise ValueError("invalid target triple")
    if not isinstance(rustc, str) or not rustc.startswith("rustc ") or len(rustc) > 256:
        raise ValueError("invalid Rust compiler version")
    expected = {f"bin/{name}" for name in PROFILES[manifest["profile"]]}
    expected.add(f"systemd/{TEMPLATE}")
    files = manifest.get("files")
    if not isinstance(files, dict) or set(files) != expected:
        raise ValueError("unexpected bundle file list")
    for relative in expected:
        path = bundle / relative
        if path.parent.is_symlink() or path.is_symlink() or not path.is_file():
            raise ValueError(f"bundle file must be regular: {relative}")
        if relative.startswith("bin/") and not path.stat().st_mode & 0o111:
            raise ValueError(f"bundle binary is not executable: {relative}")
        if digest(path) != files[relative]:
            raise ValueError(f"checksum mismatch: {relative}")
    return manifest


def install(bundle, prefix):
    """Install without replacing an existing release or changing the active one."""
    manifest = verify(bundle)
    prefix = Path(prefix).expanduser().absolute()
    if prefix.is_symlink():
        raise ValueError("install prefix must not be a symlink")
    versions = prefix / "versions"
    if versions.is_symlink():
        raise ValueError("versions directory must not be a symlink")
    versions.mkdir(parents=True, exist_ok=True)
    name = f"{manifest['version']}-{manifest['profile']}-{manifest['platform']}"
    if not RELEASE.fullmatch(name):
        raise ValueError("invalid release name")
    destination = versions / name
    if destination.exists() or destination.is_symlink():
        raise ValueError(f"release already exists: {destination}")
    with tempfile.TemporaryDirectory(prefix=".install-", dir=versions) as tmp:
        staged = Path(tmp) / "release"
        create_bundle(staged, manifest["version"], manifest["profile"],
                      manifest["source_revision"], manifest["target_triple"],
                      manifest["rustc_version"], Path(bundle) / "bin",
                      Path(bundle) / "systemd" / TEMPLATE)
        if verify(staged) != manifest:
            raise ValueError("bundle changed during installation")
        # Exclusive mkdir prevents a concurrent install from replacing a release.
        destination.mkdir()
        try:
            shutil.copytree(staged, destination, dirs_exist_ok=True)
            verify(destination)
        except BaseException:
            shutil.rmtree(destination)
            raise
    return name


def activate(prefix, release):
    """Atomically select an already installed verified release; do not restart it."""
    if not RELEASE.fullmatch(release):
        raise ValueError("invalid release name")
    prefix = Path(prefix).expanduser().absolute()
    if prefix.is_symlink():
        raise ValueError("install prefix must not be a symlink")
    versions = prefix / "versions"
    if versions.is_symlink():
        raise ValueError("versions directory must not be a symlink")
    manifest = verify(versions / release)
    expected = f"{manifest['version']}-{manifest['profile']}-{manifest['platform']}"
    if release != expected:
        raise ValueError("release name does not match its manifest")
    current = prefix / "current"
    if current.exists() and not current.is_symlink():
        raise ValueError("current exists and is not a symlink")
    with tempfile.TemporaryDirectory(prefix=".activate-", dir=prefix) as tmp:
        link = Path(tmp) / "current"
        link.symlink_to(Path("versions") / release)
        os.replace(link, current)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    build = commands.add_parser("build", help="build a clean source revision")
    build.add_argument("--profile", choices=PROFILES, required=True)
    build.add_argument("--version", required=True)
    build.add_argument("--output", type=Path, required=True)
    build.add_argument("--target-dir", type=Path, required=True)
    add = commands.add_parser("install", help="verify and install without activation")
    add.add_argument("bundle", type=Path)
    add.add_argument("--prefix", type=Path, default=Path.home() / ".local/opt/ultty")
    switch = commands.add_parser("activate", help="select a verified installed release")
    switch.add_argument("release")
    switch.add_argument("--prefix", type=Path, default=Path.home() / ".local/opt/ultty")
    args = parser.parse_args()
    if args.command == "build":
        identifier(args.version)
        revision = source_revision()
        rustc = os.environ.get("RUSTC", "rustc")
        rustc_info = subprocess.check_output([rustc, "-vV"], cwd=ROOT, text=True).splitlines()
        if not rustc_info:
            raise ValueError("could not determine the Rust compiler version")
        rustc_version = rustc_info[0]
        target_triple = next((line[6:] for line in rustc_info if line.startswith("host: ")), None)
        if not target_triple or not re.fullmatch(r"[A-Za-z0-9_.-]{1,128}", target_triple):
            raise ValueError("could not determine the native Rust target")
        target = args.target_dir.expanduser().absolute()
        if args.output.exists() or args.output.is_symlink():
            raise ValueError(f"bundle destination already exists: {args.output}")
        # A fresh target prevents Cargo from reusing binaries built in another checkout.
        target.mkdir(parents=True, exist_ok=False)
        command = ["cargo", "build", "--release", "--locked", "--target", target_triple, "-p", "ultty"]
        if args.profile == "headless":
            command.append("--no-default-features")
        for name in PROFILES[args.profile]:
            command.extend(["--bin", name])
        env = dict(os.environ, CARGO_TARGET_DIR=str(target))
        subprocess.run(command, cwd=ROOT, env=env, check=True)
        if source_revision() != revision:
            raise ValueError("source revision changed during build")
        manifest = create_bundle(args.output, args.version, args.profile, revision,
                                 target_triple, rustc_version, target / target_triple / "release",
                                 ROOT / "dist/systemd" / TEMPLATE)
        print(json.dumps(manifest, indent=2))
    elif args.command == "install":
        print(install(args.bundle, args.prefix))
    else:
        activate(args.prefix, args.release)
        print(f"active release: {args.release}")


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError, json.JSONDecodeError, subprocess.CalledProcessError) as error:
        print(f"session-bundle: {error}", file=sys.stderr)
        sys.exit(1)
