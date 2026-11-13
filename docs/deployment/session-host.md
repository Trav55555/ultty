# Deploying a persistent session host

These instructions build versioned bundles for a Linux user account. A bundle
is a directory containing executable files, a systemd user-service template,
and a manifest with source revision, profile, platform, Rust compiler, native
target triple and SHA-256 hashes.
The hashes detect accidental changes; they do not authenticate an untrusted
bundle. Verify the source and use a trusted transfer channel.

The host owns the terminal processes. Detaching a viewer leaves that host
running, but stopping or losing the host ends its sessions and processes.
Starting a new host creates a new owner; it does not recover those processes.
Neither installation nor activation starts or stops a service.

## Build matching bundles

On each target machine, check out the same committed revision and build for
that machine's OS and architecture. A release build refuses a checkout with
tracked or untracked changes. Python 3, Git, the pinned Rust toolchain and the
platform's normal Ultty build dependencies are required.
Choose a fresh target directory for each build; the tool refuses an existing
one so it cannot accidentally package stale binaries from another checkout.

```sh
# On a graphical workstation:
python3 scripts/deploy/session-bundle.py build \
  --profile full --version 2026-11-13-a \
  --target-dir "$PWD/target/session-release-full" \
  --output "$PWD/target/ultty-full-bundle"

# On a headless host, such as an ARM64 devbox:
python3 scripts/deploy/session-bundle.py build \
  --profile headless --version 2026-11-13-a \
  --target-dir "$PWD/target/session-release-headless" \
  --output "$PWD/target/ultty-host-bundle"
```

The full profile builds `ultty`, `ultty-session`, and `ulttyctl`; the headless
profile builds the latter two with `--no-default-features`. They may have
different binary hashes and architectures. Confirm their manifests have the
same `source_revision` before pairing them. Sharing a version label alone does
not establish compatibility. Preserve each bundle and its manifest when
transferring it; install refuses the wrong local platform, unexpected file
lists, symlinked files and hash mismatches. The output directory must not exist.

## Install and start

On the destination, from a checkout containing the packaging tool:

```sh
python3 scripts/deploy/session-bundle.py install /path/to/bundle
# Copy the release name printed above into the next command:
python3 scripts/deploy/session-bundle.py activate RELEASE_NAME
```

The default prefix is `~/.local/opt/ultty`; use `--prefix PATH` for a disposable
or custom installation. Releases live under `versions/`, and `current` is an
atomically replaced symlink. Installation never overwrites an existing release,
and activation rechecks its manifest and files. Use
`~/.local/opt/ultty/current/bin/ultty-session` and, on a workstation,
`~/.local/opt/ultty/current/bin/ultty` directly, or add that directory to PATH.

To run one named host without systemd:

```sh
~/.local/opt/ultty/current/bin/ultty-session host work
```

For a systemd user service at the default prefix, install the template and
start it explicitly:

```sh
mkdir -p ~/.config/systemd/user
install -m 644 ~/.local/opt/ultty/current/systemd/ultty-session@.service \
  ~/.config/systemd/user/ultty-session@.service
systemctl --user daemon-reload
systemctl --user start ultty-session@work.service
systemctl --user status ultty-session@work.service
```

The template sets `XDG_RUNTIME_DIR` to systemd's `%t` and uses `Restart=no`.
It does not enable the service at login or configure lingering. A custom prefix
requires editing both absolute executable paths in the installed unit before
`daemon-reload`; the shipped template targets the default prefix. Session names
must satisfy the host's name validation. The user manager and its runtime
directory must be available. A reboot or user-manager shutdown loses the host.

SSH attachment invokes the fixed remote command `ultty-session attach-stdio`.
The noninteractive remote SSH PATH must resolve `ultty-session` to the installed
release, and its runtime directory must match the user manager's runtime
directory. Check both from the viewer machine:

```sh
ssh HOST 'command -v ultty-session; printf "SSH runtime: %s\n" "${XDG_RUNTIME_DIR:-<unset>}"; printf "user runtime: "; loginctl show-user "$(id -un)" -p RuntimePath --value'
```

If the runtime differs or is unset, a user-owned wrapper in a directory already
on the remote noninteractive PATH can set `XDG_RUNTIME_DIR` to the *existing
private user runtime directory* shown above and then `exec` the absolute
`~/.local/opt/ultty/current/bin/ultty-session` with `"$@"`. For example, with
the actual path substituted for `/run/user/1001`:

```sh
#!/bin/sh
XDG_RUNTIME_DIR=/run/user/1001
export XDG_RUNTIME_DIR
exec "$HOME/.local/opt/ultty/current/bin/ultty-session" "$@"
```

Create that wrapper only where it does not replace an existing command, make it
executable, and check command resolution again. The general SSH environment can
still report an unset runtime: the wrapper sets it only for the helper. Confirm
the wrapper works with the scoped snapshot command linked below. Do not invent a
shared `/tmp` runtime path. The wrapper's directory must actually occur in
noninteractive SSH's PATH.
Installing a bundle does not edit shell startup files, SSH configuration,
Tailscale, or network policy. Create an expiring scoped credential, transfer it
securely and attach as shown in [Grant, inspect and revoke](../SESSION_ATTACHMENT.md#grant-inspect-and-revoke)
and [SSH and Tailscale](../SESSION_ATTACHMENT.md#ssh-and-tailscale).

## Upgrade and rollback

Build and install the next bundle while the old host is running, then plan an
explicit interruption. Save work and stop every host using the old release
before switching `current`:

```sh
systemctl --user stop ultty-session@work.service
python3 scripts/deploy/session-bundle.py activate NEW_RELEASE_NAME
systemctl --user start ultty-session@work.service
```

For a manually started host, use its current `ultty-session stop work` command
and wait for it to exit before activation. Stopping ends that owner's children.
Repeat the same stop/activate/start sequence with the old release name to roll
back the executable version; it cannot restore the old sessions. If the unit
template changes, explicitly copy the new template and run `daemon-reload`
before starting. Existing version directories remain available for rollback.
