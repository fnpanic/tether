#!/usr/bin/env python3
"""macbuild: PC-side client of the Mac build runner (docs/mac-build-runner.md).

Packs a job (manifest + work folder + secrets), signs it with this sender's ed25519 key
(ssh-keygen -Y sign), uploads it into the Mac's inbox with the Tether file-transfer helpers
(put.sh/get.sh, no GUI), and reads status, log and artifacts back by download.

  macbuild.py keygen   --user U
  macbuild.py register --user U --pub FILE [--quota-gb N] [--no-sandbox] [--replace]   (admin key)
  macbuild.py submit   --user U --work DIR [--secret FILE[=NAME]]... [--artifact GLOB]...
                       [--env K=V]... [--timeout S] [--expires-hours H] [--pack-only] -- CMD...
  macbuild.py status   --user U ID          macbuild.py wait --user U ID [--interval S]
  macbuild.py log      --user U ID [--tail N] [--file on-unlock.log]
  macbuild.py fetch    --user U ID NAME --out FILE
  macbuild.py cancel   --user U ID
  macbuild.py update-runner                 (admin: sends tools/macbuild/mac/{runner.py,install.sh})

Environment: MACBUILD_DIR (keys and job copies, default ~/.macbuild), MACBUILD_MAC_HOME
(default /Users/thomasschend), MACBUILD_PUT / MACBUILD_GET (default C:/vms/night/put.sh, get.sh:
"<script> <local> <remote>" / "<script> <remote> <local>", print "job_done" on success),
MACBUILD_BASH, MACBUILD_SSH_KEYGEN, MACBUILD_LOCK.
"""
import argparse
import io
import json
import os
import secrets as rnd
import shutil
import subprocess
import sys
import tarfile
import tempfile
import time

HERE = os.path.dirname(os.path.abspath(__file__))
DIR = os.environ.get("MACBUILD_DIR") or os.path.join(os.path.expanduser("~"), ".macbuild")
MAC_HOME = os.environ.get("MACBUILD_MAC_HOME", "/Users/thomasschend")
REMOTE = MAC_HOME + "/build-runner"
NS = lambda user: "macbuild-admin" if user == "admin" else "macbuild-job"


def die(msg):
    sys.exit("macbuild: " + msg)


def keyfile(user):
    return os.path.join(DIR, user, "id_ed25519")


def ssh_keygen():
    return os.environ.get("MACBUILD_SSH_KEYGEN") or shutil.which("ssh-keygen") or die("ssh-keygen not found")


def bash():
    b = os.environ.get("MACBUILD_BASH")
    if not b and os.name == "nt":  # never WSL's bash.exe: the helpers are Git Bash scripts
        for c in (r"C:\Program Files\Git\bin\bash.exe", r"C:\Program Files\Git\usr\bin\bash.exe"):
            if os.path.exists(c):
                b = c
                break
    return b or shutil.which("bash") or die("bash not found")


# ---------------------------------------------------------------- packing and signing
def new_id():
    return time.strftime("%Y%m%dT%H%M%SZ", time.gmtime()) + "-" + rnd.token_hex(8)


def _norm(ti):
    if ti.issym() or ti.islnk() or not (ti.isfile() or ti.isdir()):
        die("only plain files and folders can be sent (pack links into a .tgz): %s" % ti.name)
    ti.uid = ti.gid = 0
    ti.uname = ti.gname = ""
    ti.mode = 0o755 if ti.isdir() or ti.mode & 0o100 or ti.name.endswith(".sh") else 0o644
    return ti


def bundle_bytes(manifest, work=None, secrets=()):
    buf = io.BytesIO()
    with tarfile.open(fileobj=buf, mode="w:gz", format=tarfile.PAX_FORMAT) as t:
        data = json.dumps(manifest, indent=1).encode()
        ti = tarfile.TarInfo("manifest.json")
        ti.size, ti.mode, ti.mtime = len(data), 0o644, int(time.time())
        t.addfile(ti, io.BytesIO(data))
        if work:
            t.add(work, arcname="work", filter=_norm)
        for path, name in secrets:
            t.add(path, arcname="secrets/" + name, filter=_norm)
    return buf.getvalue()


def sign(user, data):
    kf = keyfile(user)
    if not os.path.exists(kf):
        die("no key for %s: run  macbuild.py keygen --user %s  and have it registered" % (user, user))
    d = tempfile.mkdtemp(prefix="macbuild-sign-")
    try:
        f = os.path.join(d, "bundle.tgz")
        with open(f, "wb") as fh:
            fh.write(data)
        r = subprocess.run([ssh_keygen(), "-Y", "sign", "-f", kf, "-n", NS(user), f], stdin=subprocess.DEVNULL,
                           stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
        if r.returncode:
            die("ssh-keygen -Y sign failed: " + r.stdout.decode(errors="replace"))
        with open(f + ".sig", "rb") as fh:
            return fh.read()
    finally:
        shutil.rmtree(d, ignore_errors=True)


def pack(user, manifest, work=None, secrets=(), kind="job"):
    """Write <DIR>/<user>/jobs/<id>/<id>.<kind> (alone in its folder, as the uploader wants)."""
    data = bundle_bytes(manifest, work, secrets)
    sig = sign(user, data)
    d = os.path.join(DIR, user, "jobs", manifest["id"])
    os.makedirs(d, exist_ok=True)
    out = os.path.join(d, "%s.%s" % (manifest["id"], kind))
    with tarfile.open(out, "w:", format=tarfile.PAX_FORMAT) as t:
        for name, blob in (("bundle.tgz", data), ("bundle.sig", sig)):
            ti = tarfile.TarInfo(name)
            ti.size, ti.mode, ti.mtime = len(blob), 0o600, int(time.time())
            t.addfile(ti, io.BytesIO(blob))
    return out


def manifest(user, action="run", expires_hours=24, **kw):
    t = int(time.time())
    m = {"v": 1, "user": user, "id": new_id(), "action": action, "created": t, "expires": t + int(expires_hours * 3600)}
    m.update(kw)
    return m


# ---------------------------------------------------------------- transfer (Tether file transfer helpers)
class FtLock:
    """One file transfer to the Mac at a time from this PC (shared by all macbuild users)."""

    def __init__(self):
        self.path = os.environ.get("MACBUILD_LOCK") or os.path.join(tempfile.gettempdir(), "macbuild-ft.lock")

    def __enter__(self):
        for _ in range(1800):
            try:
                os.mkdir(self.path)
                return self
            except FileExistsError:
                try:
                    if time.time() - os.stat(self.path).st_mtime > 900:  # stale: holder died
                        os.rmdir(self.path)
                        continue
                except OSError:
                    pass
                time.sleep(1)
        die("file transfer lock %s held for 30 min" % self.path)

    def __exit__(self, *a):
        try:
            os.rmdir(self.path)
        except OSError:
            pass


# put.sh's viewers carry "--password-env MACPW"; it kills its leftovers itself before each transfer.
OWN_VIEWER = '$_.Name -eq "tether.exe" -and $_.CommandLine -like "*--file-transfer*" -and $_.CommandLine -like "*password-env MACPW*"'


def foreign_transfer_running():
    """A Tether viewer in file-transfer mode started by someone else (e.g. another project's script)."""
    if os.name != "nt" or os.environ.get("MACBUILD_PUT"):
        return False
    ps = ('@(Get-CimInstance Win32_Process | Where-Object { $_.Name -eq "tether.exe" -and '
          '$_.CommandLine -like "*--file-transfer*" -and -not ($_.CommandLine -like "*password-env MACPW*") }).Count')
    try:
        out = subprocess.run(["powershell", "-NoProfile", "-Command", ps], stdout=subprocess.PIPE, timeout=60).stdout
        return int(out.strip() or 0) > 0
    except (OSError, ValueError, subprocess.SubprocessError):
        return False


def transfer(kind, src, dst, quiet=False):
    script = os.environ.get("MACBUILD_" + kind.upper()) or "C:/vms/night/%s.sh" % kind
    with FtLock():
        for i in range(90):  # keep out of another session's transfer: up to 15 min
            if not foreign_transfer_running():
                break
            if i == 0:
                print("waiting for another file transfer to the Mac to finish...", file=sys.stderr)
            time.sleep(10)
        env = dict(os.environ, MSYS_NO_PATHCONV="1")
        try:
            r = subprocess.run([bash(), script, src, dst], stdout=subprocess.PIPE, stderr=subprocess.STDOUT, env=env, timeout=600)
        finally:
            if os.name == "nt" and not os.environ.get("MACBUILD_PUT"):  # close the session: keep the Mac free for others
                subprocess.run(["powershell", "-NoProfile", "-Command", "Get-CimInstance Win32_Process | Where-Object { %s } | "
                                "ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }; exit 0" % OWN_VIEWER],
                               stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=60)
    out = r.stdout.decode(errors="replace")
    if '"job_done"' not in out:
        if quiet:
            return False
        die("%s %s -> %s failed: %s" % (kind, src, dst, out.strip()[-300:]))
    return True


def download(remote, quiet=False):
    """Fetch one remote file into a fresh folder (the transfer never overwrites); return its bytes."""
    d = tempfile.mkdtemp(prefix="macbuild-get-")
    local = os.path.join(d, os.path.basename(remote))
    try:
        if not transfer("get", remote, local, quiet) or not os.path.exists(local):
            return None
        with open(local, "rb") as f:
            return f.read()
    finally:
        shutil.rmtree(d, ignore_errors=True)


def jobpath(user, job_id, *a):
    return "/".join((REMOTE, "users", user, "jobs", job_id) + a)


def get_status(user, job_id):
    raw = download(jobpath(user, job_id, "status.json"), quiet=True)
    if raw is not None:
        return json.loads(raw)
    raw = download("%s/users/%s/rejected/%s.job.json" % (REMOTE, user, job_id), quiet=True)
    if raw is not None:
        return dict(json.loads(raw), state="rejected")
    return {"state": "unknown", "reason": "no status yet (not picked up, or the runner is not running)"}


# ---------------------------------------------------------------- commands
def cmd_keygen(a):
    kf = keyfile(a.user)
    if os.path.exists(kf):
        die("%s exists" % kf)
    os.makedirs(os.path.dirname(kf), exist_ok=True)
    subprocess.run([ssh_keygen(), "-q", "-t", "ed25519", "-N", "", "-C", "macbuild-" + a.user, "-f", kf], check=True)
    print(open(kf + ".pub").read().strip())
    print("public key: %s.pub  (register it: macbuild.py register --user %s --pub %s.pub)" % (kf, a.user, kf))


def submit(user, m, work=None, secrets=(), kind="job", pack_only=False):
    path = pack(user, m, work, secrets, kind)
    if pack_only:
        print(path)
        return m["id"]
    try:
        transfer("put", path, "%s/inbox/%s/%s" % (REMOTE, user, os.path.basename(path)))
    finally:
        os.unlink(path)  # it may hold secrets; the manifest copy stays for reference
        with open(os.path.join(os.path.dirname(path), "manifest.json"), "w") as f:
            json.dump(m, f, indent=1)
    print(m["id"])
    return m["id"]


def cmd_submit(a):
    if not a.cmd:
        die("give the command after --")
    cmd = a.cmd[1:] if a.cmd[0] == "--" else a.cmd
    secrets = []
    for s in a.secret:
        path, _, name = s.partition("=")
        if not os.path.isfile(path):
            die("secret %s is not a file" % path)
        secrets.append((path, name or os.path.basename(path)))
    env = dict(kv.split("=", 1) for kv in a.env)
    m = manifest(a.user, expires_hours=a.expires_hours, cmd=cmd, timeout=a.timeout, env=env, artifacts=a.artifact)
    submit(a.user, m, a.work, secrets, pack_only=a.pack_only)


def cmd_register(a):
    pub = open(a.pub).read().strip()
    args = [a.user, pub] + (["--quota-gb", str(a.quota_gb)] if a.quota_gb else []) + \
        (["--no-sandbox"] if a.no_sandbox else []) + (["--replace"] if a.replace else [])
    m = manifest("admin", cmd=["/bin/sh", "-c", 'exec "$JOB_PYTHON" "$JOB_RUNNER" add-user "$@"', "sh"] + args, timeout=120)
    submit("admin", m, pack_only=a.pack_only)


def cmd_update_runner(a):
    d = tempfile.mkdtemp(prefix="macbuild-upd-")
    for f in ("runner.py", "install.sh"):
        shutil.copy(os.path.join(HERE, "mac", f), d)
    m = manifest("admin", cmd=["bash", "install.sh", "--update"], timeout=300)
    submit("admin", m, d, pack_only=a.pack_only)
    shutil.rmtree(d, ignore_errors=True)


def cmd_cancel(a):
    m = manifest(a.user, action="cancel", expires_hours=1, target=a.id)
    submit(a.user, m, kind="cancel", pack_only=a.pack_only)


def cmd_status(a):
    print(json.dumps(get_status(a.user, a.id), indent=1, sort_keys=True))


def cmd_log(a):
    name = getattr(a, "file", None) or "job.log"
    raw = download(jobpath(a.user, a.id, name), quiet=True)
    if raw is None:
        die("no %s for %s yet" % (name, a.id))
    lines = raw.decode(errors="replace").splitlines()
    print("\n".join(lines[-a.tail:] if a.tail else lines))


def cmd_wait(a):
    while True:
        st = get_status(a.user, a.id)
        line = "%s %s %s" % (time.strftime("%H:%M:%S"), st.get("state"), st.get("reason") or "")
        print(line.strip(), flush=True)
        if st.get("state") in ("ok", "failed", "cancelled", "rejected"):
            if st.get("state") != "rejected":
                a.tail = a.tail or 40
                cmd_log(a)
            print(json.dumps(st, indent=1, sort_keys=True))
            sys.exit(0 if st["state"] == "ok" else 1)
        time.sleep(a.interval)


def cmd_fetch(a):
    if os.path.exists(a.out):
        die("%s exists" % a.out)
    raw = download(jobpath(a.user, a.id, "artifacts", a.name))
    if raw is None:
        die("no artifact %s" % a.name)
    with open(a.out, "wb") as f:
        f.write(raw)
    print("%s (%d bytes)" % (a.out, len(raw)))


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)

    def add(name, fn, user=True, job=False):
        s = sub.add_parser(name)
        s.set_defaults(fn=fn)
        if user:
            s.add_argument("--user", required=True)
        if job:
            s.add_argument("id")
        return s

    add("keygen", cmd_keygen)
    s = add("register", cmd_register)
    s.add_argument("--pub", required=True)
    s.add_argument("--quota-gb", type=int)
    s.add_argument("--no-sandbox", action="store_true")
    s.add_argument("--replace", action="store_true")
    s.add_argument("--pack-only", action="store_true")
    s = add("submit", cmd_submit)
    s.add_argument("--work")
    s.add_argument("--secret", action="append", default=[])
    s.add_argument("--artifact", action="append", default=[])
    s.add_argument("--env", action="append", default=[])
    s.add_argument("--timeout", type=int, default=7200)
    s.add_argument("--expires-hours", type=float, default=24)
    s.add_argument("--pack-only", action="store_true")
    s.add_argument("cmd", nargs=argparse.REMAINDER)
    add("status", cmd_status, job=True)
    s = add("wait", cmd_wait, job=True)
    s.add_argument("--interval", type=int, default=60)
    s.add_argument("--tail", type=int, default=40)
    s = add("log", cmd_log, job=True)
    s.add_argument("--tail", type=int, default=0)
    s.add_argument("--file", help="job.log (default) or on-unlock.log")
    s = add("fetch", cmd_fetch, job=True)
    s.add_argument("name")
    s.add_argument("--out", required=True)
    s = add("cancel", cmd_cancel, job=True)
    s.add_argument("--pack-only", action="store_true")
    s = add("update-runner", cmd_update_runner, user=False)
    s.add_argument("--pack-only", action="store_true")
    a = ap.parse_args(argv)
    if getattr(a, "user", None) and not __import__("re").match(r"^[a-z][a-z0-9-]{1,31}$", a.user):
        die("bad user name")
    a.fn(a)


if __name__ == "__main__":
    main()
