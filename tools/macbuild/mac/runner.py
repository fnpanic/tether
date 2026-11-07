#!/usr/bin/env python3
"""build-runner: runs signed build jobs for several senders on a Mac, from a user LaunchAgent.

Jobs arrive as files in ~/build-runner/inbox/<user>/<id>.job (Tether file transfer), are checked
(ssh-keygen ed25519 signature for that user, expiry, replay), queued, and run one at a time (or
up to max_parallel, one per user, fairly across users) with a log, a status file, a timeout and
caffeinate. Works while the screen is locked; nothing here needs sudo or the GUI.
See docs/mac-build-runner.md. Python 3.9 (the Command Line Tools' /usr/bin/python3).

  runner.py serve                                  main loop (the LaunchAgent)
  runner.py tick                                   one pass: ingest, run queued jobs to the end
  runner.py add-user <name> "<ssh-ed25519 key>" [--quota-gb N] [--no-sandbox] [--replace]
  runner.py lockstate                              locked | unlocked | unknown
  runner.py list                                   jobs and their states
"""
import io
import json
import os
import plistlib
import re
import shutil
import signal
import subprocess
import sys
import tarfile
import tempfile
import time

USER_RE = re.compile(r"^[a-z][a-z0-9-]{1,31}$")
ID_RE = re.compile(r"^[0-9]{8}T[0-9]{6}Z-[0-9a-f]{16}$")
KEY_RE = re.compile(r"^ssh-ed25519 [A-Za-z0-9+/=]{60,80}( [ -~]{0,80})?$")
ENV_RE = re.compile(r"^[A-Za-z_][A-Za-z0-9_]{0,63}$")
NS_JOB, NS_ADMIN = "macbuild-job", "macbuild-admin"
TERMINAL = ("ok", "failed", "cancelled")
DEFAULTS = {
    "max_parallel": 1,  # >1 lets a second user's job run beside the first: their secrets then coexist
    "poll_seconds": 3,
    "retention_days": 7,
    "max_expiry_hours": 48,
    "clock_skew_seconds": 300,
    "default_timeout": 7200,
    "max_timeout": 6 * 3600,
    "kill_grace_seconds": 30,
    "quota_check_seconds": 600,
    "default_quota_gb": 60,
    "max_job_mb": 1024,
    "unlock_timeout": 900,
    "users": {},
}
EXTRACT_KW = {"filter": "data"} if hasattr(tarfile, "data_filter") else {}  # paths are checked by bundle_members anyway
DEFAULT_PATH = "/opt/homebrew/bin:/opt/homebrew/sbin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"


class Reject(Exception):
    pass


def now():
    return int(time.time())


def log(msg):
    print(time.strftime("%Y-%m-%d %H:%M:%S"), msg, flush=True)


def write_json(path, obj):
    tmp = path + ".tmp"  # atomic: the PC may download the file at any moment
    with open(tmp, "w") as f:
        json.dump(obj, f, indent=1, sort_keys=True)
    os.replace(tmp, path)


def read_json(path, default=None):
    try:
        with open(path) as f:
            return json.load(f)
    except (OSError, ValueError):
        return default


def check_manifest(m, user, job_id, kind, cfg, t):
    """Validate a decoded manifest against the inbox path it came from; return it normalised."""
    if not isinstance(m, dict) or m.get("v") != 1:
        raise Reject("manifest: unsupported version")
    if m.get("user") != user or m.get("id") != job_id:
        raise Reject("manifest: user/id do not match the inbox path")
    action = m.get("action", "run")
    if action != ("cancel" if kind == "cancel" else "run"):
        raise Reject("manifest: action %r does not match .%s" % (action, kind))
    created, expires = m.get("created"), m.get("expires")
    if not isinstance(created, int) or not isinstance(expires, int):
        raise Reject("manifest: created/expires must be integers")
    if created > t + cfg["clock_skew_seconds"]:
        raise Reject("manifest: created in the future")
    if expires <= t:
        raise Reject("manifest: expired")
    if expires - created > cfg["max_expiry_hours"] * 3600:
        raise Reject("manifest: expiry too far ahead")
    out = {"v": 1, "user": user, "id": job_id, "action": action, "created": created, "expires": expires}
    if action == "cancel":
        if not isinstance(m.get("target"), str) or not ID_RE.match(m["target"]):
            raise Reject("manifest: bad cancel target")
        out["target"] = m["target"]
        return out
    cmd = m.get("cmd")
    if not isinstance(cmd, list) or not cmd or not all(isinstance(c, str) for c in cmd):
        raise Reject("manifest: cmd must be a non-empty list of strings")
    timeout = m.get("timeout", cfg["default_timeout"])
    if not isinstance(timeout, int) or timeout < 1:
        raise Reject("manifest: bad timeout")
    env = m.get("env", {})
    if not isinstance(env, dict) or not all(ENV_RE.match(k) and isinstance(v, str) for k, v in env.items()):
        raise Reject("manifest: bad env")
    arts = m.get("artifacts", [])
    if not isinstance(arts, list) or not all(isinstance(a, str) and a and not a.startswith("/") and ".." not in a.split("/") for a in arts):
        raise Reject("manifest: bad artifacts")
    out.update(cmd=cmd, timeout=min(timeout, cfg["max_timeout"]), env=env, artifacts=arts)
    return out


def read_container(path, max_bytes):
    """The .job/.cancel file is an uncompressed tar of exactly bundle.tgz + bundle.sig."""
    if os.path.getsize(path) > max_bytes:
        raise Reject("container too large")
    try:
        with tarfile.open(path, "r:") as t:
            members = {m.name: m for m in t.getmembers()}
            if sorted(members) != ["bundle.sig", "bundle.tgz"] or not all(m.isfile() for m in members.values()):
                raise Reject("container must hold exactly bundle.tgz and bundle.sig")
            return t.extractfile(members["bundle.tgz"]).read(), t.extractfile(members["bundle.sig"]).read()
    except (tarfile.TarError, OSError, EOFError) as e:
        raise Reject("bad container: %s" % e)


def verify_signature(keys_file, user, data, sig):
    """ssh-keygen -Y verify: the key must be listed for <user> and allowed for the namespace."""
    ns = NS_ADMIN if user == "admin" else NS_JOB
    fd, sigpath = tempfile.mkstemp(suffix=".sig")
    try:
        with os.fdopen(fd, "wb") as f:
            f.write(sig)
        r = subprocess.run(["ssh-keygen", "-Y", "verify", "-f", keys_file, "-I", user, "-n", ns, "-s", sigpath],
                           input=data, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=60)
    finally:
        os.unlink(sigpath)
    if r.returncode != 0 or not r.stdout.startswith(b"Good"):
        raise Reject("signature not valid for %s: %s" % (user, r.stdout.decode(errors="replace").strip()[:200]))


def bundle_members(data):
    """Check every entry of bundle.tgz (only manifest.json, work/**, secrets/**; files and dirs only)."""
    try:
        t = tarfile.open(fileobj=io.BytesIO(data), mode="r:gz")
        members = t.getmembers()
    except (tarfile.TarError, OSError, EOFError) as e:
        raise Reject("bad bundle: %s" % e)
    for m in members:
        norm = os.path.normpath(m.name)
        if m.name.startswith("/") or norm == ".." or norm.startswith("../") or "\\" in m.name:
            raise Reject("unsafe path in bundle: %r" % m.name)
        if not (m.isfile() or m.isdir()):
            raise Reject("only files and directories allowed in bundle: %r" % m.name)
        top = norm.split("/")[0]
        if top not in ("manifest.json", "work", "secrets", "."):
            raise Reject("unexpected entry in bundle: %r" % m.name)
    return t, members


def pick_next(queued, busy_users, last_start):
    """Fair queue. queued: [(queued_at, user, id)]. At most one running job per user; among the
    users who wait, the one whose last job started longest ago goes first; FIFO within a user."""
    heads = {}
    for q in sorted(queued):
        if q[1] not in busy_users and q[1] not in heads:
            heads[q[1]] = q
    if not heads:
        return None
    return min(heads.values(), key=lambda q: (last_start.get(q[1], 0), q[0]))


def parse_lockstate(raw, uid):
    try:
        d = plistlib.loads(raw)
    except Exception:
        return "unknown"
    if isinstance(d, list):
        d = d[0] if d else {}
    for u in d.get("IOConsoleUsers", []):
        if u.get("kCGSSessionUserIDKey") == uid:
            if u.get("CGSSessionScreenIsLocked") or not u.get("kCGSSessionOnConsoleKey", False):
                return "locked"
            return "unlocked"
    return "unknown"


def lockstate():
    """This user's console session: locked | unlocked | unknown (ioreg, no sudo, no GUI)."""
    try:
        raw = subprocess.run(["ioreg", "-n", "Root", "-d1", "-a"], stdout=subprocess.PIPE, timeout=10).stdout
    except (OSError, subprocess.SubprocessError):
        return "unknown"
    return parse_lockstate(raw, os.getuid())


def sandbox_profile(root, user, users, home):
    """sandbox-exec profile: everything allowed except other users' runner folders and the runner's
    own control files. A guard against accidents, not against a deliberately malicious sender."""
    q = lambda s: '"%s"' % s.replace("\\", "\\\\").replace('"', '\\"')
    rules = ["(version 1)", "(allow default)"]
    for other in sorted(users):
        if other != user:
            rules.append("(deny file-read* file-write* (subpath %s) (subpath %s))"
                         % (q(os.path.join(root, "users", other)), q(os.path.join(root, "inbox", other))))
    rules.append("(deny file-write* (subpath %s) (subpath %s) (literal %s) (literal %s) (subpath %s))"
                 % (q(os.path.join(root, "keys")), q(os.path.join(root, "state")), q(os.path.join(root, "runner.py")),
                    q(os.path.join(root, "config.json")), q(os.path.join(home, "Library", "LaunchAgents"))))
    return "\n".join(rules) + "\n"


def du_bytes(path):
    try:
        out = subprocess.run(["du", "-sk", path], stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, timeout=600).stdout
        return int(out.split()[0]) * 1024
    except (OSError, subprocess.SubprocessError, ValueError, IndexError):
        return 0


def killpg(pgid, sig):
    try:
        os.killpg(pgid, sig)
    except (OSError, TypeError):
        pass


class Runner:
    def __init__(self, root):
        self.root = os.path.realpath(root)
        self.home = os.path.expanduser("~")
        self.running = {}  # (user, id) -> dict(proc, caf, deadline, kill_at, reason, next_quota)
        self.unlock = None  # dict(proc, user, id, deadline) of the on-unlock step being run
        self.last_start = {}
        self.next_cleanup = 0
        self.lockstate = lockstate
        self.code_mtime = self._mtime(os.path.join(self.root, "runner.py"))

    # ------------------------------------------------------------------ paths and config
    def p(self, *a):
        return os.path.join(self.root, *a)

    def cfg(self):
        c = dict(DEFAULTS)
        c.update(read_json(self.p("config.json"), {}))
        return c

    def user_cfg(self, cfg, user):
        u = dict(cfg["users"].get(user, {}))
        u.setdefault("quota_gb", cfg["default_quota_gb"])
        u["sandbox"] = bool(u.get("sandbox", True)) and user != "admin"
        return u

    def jobdir(self, user, job_id):
        return self.p("users", user, "jobs", job_id)

    def status(self, user, job_id):
        return read_json(os.path.join(self.jobdir(user, job_id), "status.json"), {})

    def set_status(self, user, job_id, /, **kw):
        path = os.path.join(self.jobdir(user, job_id), "status.json")
        st = read_json(path, {})
        st.update(kw)
        write_json(path, st)
        return st

    @staticmethod
    def _mtime(path):
        try:
            return os.stat(path).st_mtime
        except OSError:
            return None

    # ------------------------------------------------------------------ users
    def add_user(self, name, key, quota_gb=None, sandbox=True, replace=False):
        if not USER_RE.match(name):
            raise SystemExit("bad user name %r (a-z, 0-9, -; 2-32 chars)" % name)
        key = key.strip()
        if not KEY_RE.match(key):
            raise SystemExit("not an ssh-ed25519 public key line")
        os.makedirs(self.p("keys"), exist_ok=True)
        os.chmod(self.root, 0o700)
        kf = self.p("keys", "allowed_signers")
        try:
            with open(kf) as f:
                lines = f.read().splitlines()
        except FileNotFoundError:
            lines = []
        ns = NS_ADMIN if name == "admin" else NS_JOB
        line = '%s namespaces="%s" %s' % (name, ns, " ".join(key.split()[:2]))
        if replace:
            lines = [l for l in lines if l.split(" ", 1)[0] != name]
        if line not in lines:
            lines.append(line)
        with open(kf + ".tmp", "w") as f:
            f.write("\n".join(lines) + "\n")
        os.replace(kf + ".tmp", kf)
        cfgp = self.p("config.json")
        c = read_json(cfgp, {})
        u = c.setdefault("users", {}).setdefault(name, {})
        if quota_gb is not None:
            u["quota_gb"] = quota_gb
        u["sandbox"] = sandbox and name != "admin"
        write_json(cfgp, c)
        for d in (self.p("inbox", name), self.p("users", name, "jobs"), self.p("users", name, "cache"),
                  self.p("users", name, "rejected"), self.p("state")):
            os.makedirs(d, exist_ok=True)
        print("user %s: key %s... registered (%s), sandbox %s" % (name, key.split()[1][:16], ns, u["sandbox"]))

    def users(self, cfg):
        return sorted(u for u in cfg["users"] if USER_RE.match(u))

    # ------------------------------------------------------------------ inbox
    def seen_check(self, user, job_id, expires, t):
        """Replay protection: an id is accepted once. Ids are kept until they expire; expired
        manifests are refused anyway, so the list stays short."""
        path = self.p("state", "seen-%s.json" % user)
        seen = {k: v for k, v in read_json(path, {}).items() if v > t}
        if job_id in seen or os.path.exists(self.jobdir(user, job_id)):
            raise Reject("replay: job id already used")
        seen[job_id] = expires
        write_json(path, seen)

    def reject(self, user, name, reason):
        log("rejected %s/%s: %s" % (user, name, reason))
        d = self.p("users", user, "rejected")
        if os.path.isdir(d):
            write_json(os.path.join(d, name + ".json"), {"file": name, "reason": reason, "at": now()})

    def ingest(self, user, path, cfg, t):
        name = os.path.basename(path)
        m = re.match(r"^(.+)\.(job|cancel)$", name)
        if not m or not ID_RE.match(m.group(1)):
            raise Reject("file name must be <id>.job or <id>.cancel")
        job_id, kind = m.group(1), m.group(2)
        data, sig = read_container(path, cfg["max_job_mb"] << 20)
        verify_signature(self.p("keys", "allowed_signers"), user, data, sig)
        tf, members = bundle_members(data)
        mf = [x for x in members if os.path.normpath(x.name) == "manifest.json" and x.isfile()]
        if len(mf) != 1:
            raise Reject("bundle has no manifest.json")
        try:
            manifest = json.loads(tf.extractfile(mf[0]).read())
        except ValueError:
            raise Reject("manifest.json is not JSON")
        manifest = check_manifest(manifest, user, job_id, kind, cfg, t)
        self.seen_check(user, job_id, manifest["expires"], t)
        if kind == "cancel":
            self.cancel(user, manifest["target"], cfg)
            return
        d = self.jobdir(user, job_id)
        os.makedirs(d)
        for x in members:  # checked above: files and directories under work/ and secrets/ only
            if os.path.normpath(x.name) != "manifest.json":
                tf.extract(x, d, set_attrs=False, **EXTRACT_KW)
                if x.isfile():
                    os.chmod(os.path.join(d, x.name), 0o700 if x.mode & 0o100 else 0o600)
        for sub in ("work", "secrets", "artifacts"):
            os.makedirs(os.path.join(d, sub), exist_ok=True)
        os.chmod(os.path.join(d, "secrets"), 0o700)
        write_json(os.path.join(d, "manifest.json"), manifest)
        self.set_status(user, job_id, id=job_id, user=user, state="queued", queued_at=time.time(), timeout=manifest["timeout"])
        log("queued %s/%s: %s" % (user, job_id, " ".join(manifest["cmd"])[:120]))

    def scan_inbox(self, cfg):
        t = now()
        for user in self.users(cfg):
            d = self.p("inbox", user)
            try:
                names = sorted(os.listdir(d))
            except OSError:
                continue
            for name in names:
                if name.startswith(".") or name.endswith((".download", ".digest", ".tmp")):
                    continue  # Tether file transfer writes <name>.download and renames it when complete
                path = os.path.join(d, name)
                try:
                    if os.path.isfile(path):
                        self.ingest(user, path, cfg, t)
                    else:
                        raise Reject("not a file")
                except Reject as e:
                    self.reject(user, name, str(e))
                except Exception as e:  # never let one bad file stop the runner
                    self.reject(user, name, "internal error: %r" % e)
                finally:
                    if os.path.isdir(path) and not os.path.islink(path):
                        shutil.rmtree(path, ignore_errors=True)
                    elif os.path.lexists(path):
                        os.unlink(path)

    # ------------------------------------------------------------------ jobs
    def queued(self, cfg):
        out = []
        for user in self.users(cfg):
            jd = self.p("users", user, "jobs")
            for job_id in os.listdir(jd) if os.path.isdir(jd) else []:
                st = self.status(user, job_id)
                if st.get("state") == "queued" and (user, job_id) not in self.running:
                    out.append((st.get("queued_at", 0), user, job_id))
        return out

    def drop_secrets(self, user, job_id):
        shutil.rmtree(os.path.join(self.jobdir(user, job_id), "secrets"), ignore_errors=True)

    def start(self, user, job_id, cfg):
        d = self.jobdir(user, job_id)
        m = read_json(os.path.join(d, "manifest.json"))
        ucfg = self.user_cfg(cfg, user)
        t = now()
        self.last_start[user] = t
        if du_bytes(self.p("users", user)) > ucfg["quota_gb"] * (1 << 30):
            self.drop_secrets(user, job_id)
            self.set_status(user, job_id, state="failed", reason="disk quota of %s GB exceeded" % ucfg["quota_gb"], ended_at=t)
            return
        env = {"HOME": self.home, "USER": os.environ.get("USER", ""), "LOGNAME": os.environ.get("USER", ""),
               "SHELL": "/bin/bash", "LANG": "en_US.UTF-8", "PATH": DEFAULT_PATH, "TMPDIR": os.environ.get("TMPDIR", "/tmp/"),
               "JOB_ID": job_id, "JOB_USER": user, "JOB_DIR": d, "JOB_WORK": os.path.join(d, "work"),
               "JOB_SECRETS": os.path.join(d, "secrets"), "JOB_ARTIFACTS": os.path.join(d, "artifacts"),
               "JOB_CACHE": self.p("users", user, "cache"), "JOB_ON_UNLOCK": os.path.join(d, "on-unlock.sh"),
               "JOB_RUNNER": self.p("runner.py"), "JOB_PYTHON": sys.executable, "BUILD_RUNNER_ROOT": self.root}
        env.update(m["env"])
        cmd = list(m["cmd"])
        if ucfg["sandbox"]:
            if not shutil.which("sandbox-exec"):
                self.drop_secrets(user, job_id)
                self.set_status(user, job_id, state="failed", reason="sandbox-exec not available", ended_at=t)
                return
            cmd = ["sandbox-exec", "-p", sandbox_profile(self.root, user, self.users(cfg), self.home)] + cmd
        logf = open(os.path.join(d, "job.log"), "ab")
        try:
            proc = subprocess.Popen(cmd, cwd=os.path.join(d, "work"), env=env, stdin=subprocess.DEVNULL,
                                    stdout=logf, stderr=subprocess.STDOUT, start_new_session=True, umask=0o022)
        except OSError as e:
            logf.close()
            self.drop_secrets(user, job_id)
            self.set_status(user, job_id, state="failed", reason="cannot start: %s" % e, ended_at=t)
            return
        logf.close()
        caf = None
        if shutil.which("caffeinate"):  # no idle/system sleep while the job runs (display may sleep)
            caf = subprocess.Popen(["caffeinate", "-i", "-s", "-w", str(proc.pid)], stdin=subprocess.DEVNULL,
                                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        self.running[(user, job_id)] = {"proc": proc, "caf": caf, "deadline": t + m["timeout"], "kill_at": None,
                                        "reason": None, "next_quota": t + cfg["quota_check_seconds"]}
        self.set_status(user, job_id, state="running", started_at=t, pid=proc.pid, sandbox=ucfg["sandbox"])
        log("started %s/%s pid %d" % (user, job_id, proc.pid))

    def stop(self, key, reason, cfg):
        r = self.running[key]
        if r["kill_at"] is None:
            r["reason"] = reason
            r["kill_at"] = now() + cfg["kill_grace_seconds"]
            killpg(r["proc"].pid, signal.SIGTERM)
            log("stopping %s/%s: %s" % (key[0], key[1], reason))

    def poll(self, cfg):
        t = now()
        for key in list(self.running):
            r = self.running[key]
            rc = r["proc"].poll()
            if rc is None:
                if r["kill_at"] is not None and t >= r["kill_at"]:
                    killpg(r["proc"].pid, signal.SIGKILL)
                elif t >= r["deadline"]:
                    self.stop(key, "timeout", cfg)
                elif t >= r["next_quota"]:
                    r["next_quota"] = t + cfg["quota_check_seconds"]
                    q = self.user_cfg(cfg, key[0])["quota_gb"]
                    if du_bytes(self.p("users", key[0])) > q * (1 << 30):
                        self.stop(key, "disk quota of %s GB exceeded" % q, cfg)
                continue
            killpg(r["proc"].pid, signal.SIGKILL)  # leftovers of the job's process group
            if r["caf"]:
                r["caf"].terminate()
            del self.running[key]
            self.finish(key[0], key[1], rc, r["reason"])

    def finish(self, user, job_id, rc, reason):
        d = self.jobdir(user, job_id)
        m = read_json(os.path.join(d, "manifest.json"), {})
        self.drop_secrets(user, job_id)
        exit_code = rc if rc >= 0 else 128 - rc
        state = "cancelled" if reason == "cancelled" else ("ok" if rc == 0 and not reason else "failed")
        arts = self.collect(d, m.get("artifacts", []))
        extra = {}
        if state == "ok" and os.path.isfile(os.path.join(d, "on-unlock.sh")):
            extra["on_unlock"] = "pending"
        self.set_status(user, job_id, state=state, exit_code=exit_code, ended_at=now(), reason=reason or ("exit %d" % exit_code if rc else None),
                        artifacts=arts, **extra)
        log("%s %s/%s exit %d %s" % (state, user, job_id, exit_code, reason or ""))

    @staticmethod
    def collect(d, patterns):
        """Copy artifacts (globs relative to work/) to artifacts/; folders (e.g. .app) as .zip."""
        import glob
        work = os.path.realpath(os.path.join(d, "work"))
        out, names = os.path.join(d, "artifacts"), []
        for pat in patterns:
            for src in sorted(glob.glob(os.path.join(work, pat))):
                real = os.path.realpath(src)
                if not real.startswith(work + os.sep):
                    continue
                base = os.path.basename(real)
                try:
                    if os.path.isdir(real):
                        dst = os.path.join(out, base + ".zip")
                        if shutil.which("ditto"):
                            subprocess.run(["ditto", "-c", "-k", "--sequesterRsrc", "--keepParent", real, dst], check=True, timeout=1800)
                        else:
                            shutil.make_archive(dst[:-4], "zip", os.path.dirname(real), base)
                    else:
                        dst = os.path.join(out, base)
                        shutil.copy2(real, dst)
                    names.append(os.path.basename(dst))
                except (OSError, subprocess.SubprocessError) as e:
                    log("artifact %s: %s" % (src, e))
        return names

    def cancel(self, user, target, cfg):
        st = self.status(user, target)
        key = (user, target)
        if key in self.running:
            self.stop(key, "cancelled", cfg)
        elif st.get("state") == "queued":
            self.drop_secrets(user, target)
            self.set_status(user, target, state="cancelled", reason="cancelled", ended_at=now())
        elif st.get("on_unlock") == "pending":
            self.set_status(user, target, on_unlock="cancelled")
        else:
            raise Reject("cancel: %s is not queued, running or waiting for unlock" % target)
        log("cancel %s/%s" % (user, target))

    # ------------------------------------------------------------------ deferred step (after unlock)
    def run_unlock_steps(self, cfg):
        """A job that succeeded may leave $JOB_ON_UNLOCK (on-unlock.sh): it runs once this user's
        session is unlocked (e.g. replacing a running app). One at a time; output on-unlock.log."""
        t = now()
        if self.unlock:
            u = self.unlock
            rc = u["proc"].poll()
            if rc is None:
                if t >= u["deadline"]:
                    killpg(u["proc"].pid, signal.SIGKILL)
                return
            killpg(u["proc"].pid, signal.SIGKILL)
            self.set_status(u["user"], u["id"], on_unlock="ok" if rc == 0 else "failed: exit %d" % (rc if rc >= 0 else 128 - rc), on_unlock_ended_at=t)
            log("on-unlock %s/%s exit %d" % (u["user"], u["id"], rc))
            self.unlock = None
            return
        pending = []
        for user in self.users(cfg):
            jd = self.p("users", user, "jobs")
            for job_id in os.listdir(jd) if os.path.isdir(jd) else []:
                st = self.status(user, job_id)
                if st.get("on_unlock") == "pending":
                    pending.append((st.get("ended_at", 0), user, job_id))
        if not pending or self.lockstate() != "unlocked":
            return
        _, user, job_id = min(pending)
        d = self.jobdir(user, job_id)
        cmd = ["/bin/bash", os.path.join(d, "on-unlock.sh")]
        ucfg = self.user_cfg(cfg, user)
        if ucfg["sandbox"] and shutil.which("sandbox-exec"):
            cmd = ["sandbox-exec", "-p", sandbox_profile(self.root, user, self.users(cfg), self.home)] + cmd
        env = {"HOME": self.home, "USER": os.environ.get("USER", ""), "PATH": DEFAULT_PATH, "LANG": "en_US.UTF-8",
               "JOB_ID": job_id, "JOB_USER": user, "JOB_DIR": d, "JOB_WORK": os.path.join(d, "work"),
               "JOB_CACHE": self.p("users", user, "cache"), "TMPDIR": os.environ.get("TMPDIR", "/tmp/")}
        with open(os.path.join(d, "on-unlock.log"), "ab") as logf:
            proc = subprocess.Popen(cmd, cwd=os.path.join(d, "work"), env=env, stdin=subprocess.DEVNULL,
                                    stdout=logf, stderr=subprocess.STDOUT, start_new_session=True, umask=0o022)
        self.unlock = {"proc": proc, "user": user, "id": job_id, "deadline": t + cfg["unlock_timeout"]}
        self.set_status(user, job_id, on_unlock="running", on_unlock_started_at=t)
        log("on-unlock %s/%s started" % (user, job_id))

    # ------------------------------------------------------------------ housekeeping
    def recover(self, cfg):
        """After a runner restart: jobs that were running are lost; no secrets outlive their job."""
        for user in self.users(cfg):
            jd = self.p("users", user, "jobs")
            for job_id in os.listdir(jd) if os.path.isdir(jd) else []:
                st = self.status(user, job_id)
                if st.get("state") == "running":
                    if st.get("pid"):
                        killpg(st["pid"], signal.SIGKILL)  # shortcut: pgid could be reused after a reboot; harmless then (own uid only)
                    self.set_status(user, job_id, state="failed", reason="runner restarted", ended_at=now())
                if st.get("on_unlock") == "running":
                    self.set_status(user, job_id, on_unlock="failed: runner restarted")
                if st.get("state") != "queued":
                    self.drop_secrets(user, job_id)

    def cleanup(self, cfg):
        """Retention: finished jobs and rejection notes older than retention_days are deleted."""
        cutoff = now() - cfg["retention_days"] * 86400
        for user in self.users(cfg):
            jd = self.p("users", user, "jobs")
            for job_id in os.listdir(jd) if os.path.isdir(jd) else []:
                st = self.status(user, job_id)
                if st.get("state") in TERMINAL and st.get("ended_at", 0) < cutoff and st.get("on_unlock") not in ("pending", "running"):
                    shutil.rmtree(os.path.join(jd, job_id), ignore_errors=True)
                    log("deleted old job %s/%s" % (user, job_id))
            rd = self.p("users", user, "rejected")
            for n in os.listdir(rd) if os.path.isdir(rd) else []:
                if (self._mtime(os.path.join(rd, n)) or 0) < cutoff:
                    os.unlink(os.path.join(rd, n))

    def tick(self):
        cfg = self.cfg()
        self.scan_inbox(cfg)
        self.poll(cfg)
        while len(self.running) < cfg["max_parallel"]:
            nxt = pick_next(self.queued(cfg), {k[0] for k in self.running}, self.last_start)
            if not nxt:
                break
            self.start(nxt[1], nxt[2], cfg)
        self.run_unlock_steps(cfg)
        if now() >= self.next_cleanup:
            self.next_cleanup = now() + 600
            self.cleanup(cfg)
        return cfg

    def serve(self):
        os.umask(0o077)  # job folders, logs and secrets are private; jobs themselves run with 022
        log("runner started (root %s, pid %d)" % (self.root, os.getpid()))
        self.recover(self.cfg())
        signal.signal(signal.SIGTERM, lambda *a: sys.exit(0))
        while True:
            cfg = self.tick()
            # Updated runner.py (an admin update job): exit when idle; launchd (KeepAlive) restarts it.
            if not self.running and not self.unlock and self._mtime(self.p("runner.py")) != self.code_mtime:
                log("runner.py changed: restarting")
                return
            time.sleep(cfg["poll_seconds"])


def main(argv):
    r = Runner(os.environ.get("BUILD_RUNNER_ROOT") or os.path.expanduser("~/build-runner"))
    cmd = argv[1] if len(argv) > 1 else "serve"
    if cmd == "serve":
        r.serve()
    elif cmd == "tick":  # ingest and run what is queued to the end, then exit
        r.tick()
        while r.running or r.unlock:
            time.sleep(0.2)
            r.tick()
    elif cmd == "add-user" and len(argv) >= 4:
        a = argv[4:]
        quota = int(a[a.index("--quota-gb") + 1]) if "--quota-gb" in a else None
        r.add_user(argv[2], argv[3], quota, sandbox="--no-sandbox" not in a, replace="--replace" in a)
    elif cmd == "lockstate":
        print(lockstate())
    elif cmd == "list":
        cfg = r.cfg()
        for user in r.users(cfg):
            jd = r.p("users", user, "jobs")
            for job_id in sorted(os.listdir(jd)) if os.path.isdir(jd) else []:
                st = r.status(user, job_id)
                print(user, job_id, st.get("state"), st.get("exit_code", ""), st.get("reason") or "", st.get("on_unlock", ""))
    else:
        print(__doc__)
        return 2
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
