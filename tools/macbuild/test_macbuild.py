"""Unit tests of the Mac build runner and its client. POSIX + ssh-keygen; on Windows run them in WSL:
    wsl -d Ubuntu-22.04 -- python3 -m unittest discover -s tools/macbuild -v
(macOS-only parts - sandbox-exec, caffeinate, ioreg, ditto - are skipped or fed canned data)."""
import io
import json
import os
import plistlib
import shutil
import subprocess
import sys
import tarfile
import tempfile
import time
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path[:0] = [HERE, os.path.join(HERE, "mac")]
import macbuild  # noqa: E402
import runner  # noqa: E402


class Base(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="mbtest-")
        self.root = os.path.join(self.tmp, "mac", "build-runner")
        macbuild.DIR = os.path.join(self.tmp, "pc")
        self.r = runner.Runner(self.root)
        os.makedirs(self.root)
        self.r.root = os.path.realpath(self.root)
        for u in ("admin", "alice", "bob"):
            self.keygen(u)
            self.r.add_user(u, open(macbuild.keyfile(u) + ".pub").read(), sandbox=False)
        cfg = runner.read_json(self.r.p("config.json"))
        cfg.update(kill_grace_seconds=1, poll_seconds=0)
        runner.write_json(self.r.p("config.json"), cfg)
        self.r.lockstate = lambda: "locked"

    def tearDown(self):
        for key, r in list(self.r.running.items()):
            runner.killpg(r["proc"].pid, 9)
        shutil.rmtree(self.tmp, ignore_errors=True)

    def keygen(self, user):
        kf = macbuild.keyfile(user)
        os.makedirs(os.path.dirname(kf), exist_ok=True)
        subprocess.run(["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-f", kf], check=True)

    def send(self, user, cmd=("sh", "-c", "echo hi"), sign_as=None, kind="job", work=None, secrets=(), **kw):
        """Pack + sign like the client, drop into the inbox like Tether file transfer."""
        m = kw.pop("manifest", None) or macbuild.manifest(user, cmd=list(cmd), timeout=kw.pop("timeout", 60), **kw)
        path = macbuild.pack(sign_as or user, m, work, secrets, kind)
        shutil.copy(path, self.r.p("inbox", user, os.path.basename(path)))
        return m["id"]

    def st(self, user, job_id):
        return self.r.status(user, job_id)

    def rejected(self, user, job_id, kind="job"):
        return runner.read_json(self.r.p("users", user, "rejected", "%s.%s.json" % (job_id, kind)))

    def run_until(self, cond, timeout=20):
        end = time.time() + timeout
        while time.time() < end:
            self.r.tick()
            if cond():
                return
            time.sleep(0.1)
        self.fail("condition not reached")


class Manifest(unittest.TestCase):
    cfg = dict(runner.DEFAULTS)
    jid = "20261107T120000Z-0123456789abcdef"

    def m(self, **kw):
        t = 1000000
        d = {"v": 1, "user": "alice", "id": self.jid, "action": "run", "created": t, "expires": t + 3600, "cmd": ["true"]}
        d.update(kw)
        return d

    def check(self, m, t=1000000, kind="job"):
        return runner.check_manifest(m, "alice", self.jid, kind, self.cfg, t)

    def test_ok_and_defaults(self):
        out = self.check(self.m(timeout=10 ** 9))
        self.assertEqual(out["timeout"], self.cfg["max_timeout"])
        self.assertEqual(out["env"], {})

    def test_rejects(self):
        bad = [self.m(v=2), self.m(user="bob"), self.m(id="20261107T120000Z-ffffffffffffffff"),
               self.m(expires=999999), self.m(created=1000000 + 3600, expires=1000000 + 7200),
               self.m(expires=1000000 + 49 * 3600), self.m(cmd=[]), self.m(cmd="rm -rf /"),
               self.m(env={"A B": "x"}), self.m(artifacts=["../x"]), self.m(artifacts=["/etc/passwd"]),
               self.m(timeout=0), self.m(action="cancel")]
        for m in bad:
            with self.assertRaises(runner.Reject, msg=m):
                self.check(m)

    def test_cancel(self):
        out = self.check(self.m(action="cancel", target=self.jid), kind="cancel")
        self.assertEqual(out["target"], self.jid)
        with self.assertRaises(runner.Reject):
            self.check(self.m(action="cancel", target="../x"), kind="cancel")


class Fairness(unittest.TestCase):
    def test_round_robin(self):
        q = [(1, "a", "a1"), (2, "a", "a2"), (3, "a", "a3"), (4, "b", "b1"), (5, "c", "c1")]
        last, order, t = {}, [], 10
        while q:
            n = runner.pick_next(q, set(), last)
            order.append(n[2])
            q.remove(n)
            last[n[1]] = t
            t += 1
        self.assertEqual(order, ["a1", "b1", "c1", "a2", "a3"])

    def test_busy_user_skipped(self):
        q = [(1, "a", "a1"), (2, "b", "b1")]
        self.assertEqual(runner.pick_next(q, {"a"}, {})[2], "b1")
        self.assertIsNone(runner.pick_next(q, {"a", "b"}, {}))


class Lockstate(unittest.TestCase):
    def raw(self, **u):
        e = {"kCGSSessionUserIDKey": 501, "kCGSSessionOnConsoleKey": True}
        e.update(u)
        return plistlib.dumps({"IORegistryEntryName": "Root", "IOConsoleUsers": [e]})

    def test_states(self):
        self.assertEqual(runner.parse_lockstate(self.raw(), 501), "unlocked")
        self.assertEqual(runner.parse_lockstate(self.raw(CGSSessionScreenIsLocked=True), 501), "locked")
        self.assertEqual(runner.parse_lockstate(self.raw(kCGSSessionOnConsoleKey=False), 501), "locked")
        self.assertEqual(runner.parse_lockstate(self.raw(), 502), "unknown")
        self.assertEqual(runner.parse_lockstate(b"garbage", 501), "unknown")


class Sandbox(unittest.TestCase):
    def test_profile(self):
        p = runner.sandbox_profile("/U/build-runner", "alice", ["alice", "bob"], "/U")
        self.assertIn('(subpath "/U/build-runner/users/bob")', p)
        self.assertIn('(subpath "/U/build-runner/inbox/bob")', p)
        self.assertNotIn("users/alice", p)
        self.assertIn('(subpath "/U/build-runner/keys")', p)
        self.assertIn('(subpath "/U/Library/LaunchAgents")', p)


class Jobs(Base):
    def test_run_ok_with_log_secrets_and_artifacts(self):
        sec = os.path.join(self.tmp, "token")
        open(sec, "w").write("s3cret")
        work = os.path.join(self.tmp, "w")
        os.makedirs(work)
        open(os.path.join(work, "in.txt"), "w").write("x")
        jid = self.send("alice", ["sh", "-c", 'cat "$JOB_SECRETS/tok"; mkdir out; echo built > out/a.bin; cat in.txt'],
                        work=work, secrets=[(sec, "tok")], artifacts=["out/*"])
        self.run_until(lambda: self.st("alice", jid).get("state") == "ok")
        d = self.r.jobdir("alice", jid)
        self.assertEqual(open(os.path.join(d, "job.log")).read(), "s3cretx")
        self.assertFalse(os.path.exists(os.path.join(d, "secrets")), "secrets must be deleted after the job")
        self.assertEqual(open(os.path.join(d, "artifacts", "a.bin")).read(), "built\n")
        st = self.st("alice", jid)
        self.assertEqual((st["exit_code"], st["artifacts"]), (0, ["a.bin"]))
        self.assertFalse(os.listdir(self.r.p("inbox", "alice")), "inbox file removed")

    def test_failure_exit_code(self):
        jid = self.send("alice", ["sh", "-c", "exit 3"])
        self.run_until(lambda: self.st("alice", jid).get("state") == "failed")
        self.assertEqual(self.st("alice", jid)["exit_code"], 3)

    def test_timeout_kills_process_group(self):
        mark = os.path.join(self.tmp, "child-alive")
        jid = self.send("alice", ["sh", "-c", "(sleep 3; touch %s) & sleep 30" % mark], timeout=1)
        self.run_until(lambda: self.st("alice", jid).get("state") == "failed")
        self.assertEqual(self.st("alice", jid)["reason"], "timeout")
        time.sleep(3.5)
        self.assertFalse(os.path.exists(mark), "background child must die with the job")

    def test_cancel_running_and_queued(self):
        j1 = self.send("alice", ["sleep", "30"])
        time.sleep(1.1)  # ids start with the second: j1 is ingested (and started) first
        j2 = self.send("alice", ["true"])
        self.run_until(lambda: self.st("alice", j1).get("state") == "running")
        self.send("alice", kind="cancel", manifest=macbuild.manifest("alice", action="cancel", target=j2))
        self.send("alice", kind="cancel", manifest=macbuild.manifest("alice", action="cancel", target=j1))
        self.run_until(lambda: self.st("alice", j1).get("state") == "cancelled")
        self.assertEqual(self.st("alice", j2)["state"], "cancelled")
        self.assertFalse(os.path.exists(os.path.join(self.r.jobdir("alice", j2), "secrets")))

    def test_one_at_a_time_fair_across_users(self):
        a = [self.send("alice", ["sleep", "0.3"])]
        time.sleep(1.1)  # ids start with the second: keep the inbox order defined
        a.append(self.send("alice", ["sleep", "0.3"]))
        time.sleep(1.1)
        b = self.send("bob", ["sleep", "0.3"])
        starts = {}

        def done():
            for u, j in [("alice", a[0]), ("alice", a[1]), ("bob", b)]:
                st = self.st(u, j)
                if st.get("started_at") and j not in starts:
                    starts[j] = (time.time(), len([1 for r in self.r.running]))
            return all(self.st(u, j).get("state") == "ok" for u, j in [("alice", a[0]), ("alice", a[1]), ("bob", b)])

        self.r.tick()  # ingest everything first
        self.run_until(done)
        order = sorted(starts, key=lambda j: starts[j][0])
        self.assertEqual(order, [a[0], b, a[1]])
        self.assertTrue(all(n == 1 for _, n in starts.values()), "max_parallel 1")

    def test_quota(self):
        cfg = runner.read_json(self.r.p("config.json"))
        cfg["users"]["alice"]["quota_gb"] = 0
        runner.write_json(self.r.p("config.json"), cfg)
        open(os.path.join(self.r.p("users", "alice", "cache"), "big"), "wb").write(b"x" * 8192)
        jid = self.send("alice")
        self.run_until(lambda: self.st("alice", jid).get("state") == "failed")
        self.assertIn("quota", self.st("alice", jid)["reason"])

    def test_retention_cleanup(self):
        jid = self.send("alice")
        self.run_until(lambda: self.st("alice", jid).get("state") == "ok")
        self.r.cleanup(self.r.cfg())
        self.assertTrue(os.path.isdir(self.r.jobdir("alice", jid)))
        self.r.set_status("alice", jid, ended_at=int(time.time()) - 8 * 86400)
        self.r.cleanup(self.r.cfg())
        self.assertFalse(os.path.exists(self.r.jobdir("alice", jid)))

    def test_on_unlock_waits_for_unlock(self):
        jid = self.send("alice", ["sh", "-c", 'printf "echo swapped > %s/swapped\\n" "$JOB_DIR" > "$JOB_ON_UNLOCK"'])
        self.run_until(lambda: self.st("alice", jid).get("on_unlock") == "pending")
        for _ in range(3):
            self.r.tick()
        self.assertEqual(self.st("alice", jid)["on_unlock"], "pending", "must not run while locked")
        self.r.lockstate = lambda: "unlocked"
        self.run_until(lambda: self.st("alice", jid).get("on_unlock") == "ok")
        self.assertTrue(os.path.exists(os.path.join(self.r.jobdir("alice", jid), "swapped")))

    def test_recover_after_restart(self):
        jid = self.send("alice", ["sleep", "30"], secrets=[(__file__, "s")])
        self.run_until(lambda: self.st("alice", jid).get("state") == "running")
        proc = next(iter(self.r.running.values()))["proc"]
        self.r.running.clear()  # runner died; its child is still there
        r2 = runner.Runner(self.root)
        r2.recover(r2.cfg())
        self.assertEqual(self.st("alice", jid)["reason"], "runner restarted")
        self.assertFalse(os.path.exists(os.path.join(self.r.jobdir("alice", jid), "secrets")))
        self.assertEqual(proc.wait(timeout=10), -9, "the job's group is killed")

    def test_recover_spares_a_reused_pid(self):
        # The recorded pid now belongs to a process started an hour after the job (reboot, wrap).
        other = subprocess.Popen(["sleep", "30"], start_new_session=True)
        try:
            jid = self.send("alice", ["sleep", "30"])
            self.run_until(lambda: self.st("alice", jid).get("state") == "running")
            job = next(iter(self.r.running.values()))["proc"]
            self.r.running.clear()
            self.r.set_status("alice", jid, pid=other.pid, started_at=int(time.time()) - 3600)
            r2 = runner.Runner(self.root)
            r2.recover(r2.cfg())
            self.assertEqual(self.st("alice", jid)["reason"], "runner restarted")
            self.assertIsNone(other.poll(), "an unrelated process must survive")
            runner.killpg(job.pid, 9)
        finally:
            other.kill()
            other.wait()

    def test_started_near(self):
        p = subprocess.Popen(["sleep", "30"])
        try:
            self.assertTrue(runner.started_near(p.pid, time.time()))
            self.assertFalse(runner.started_near(p.pid, time.time() - 3600))
            self.assertFalse(runner.started_near(999999999, time.time()))
        finally:
            p.kill()
            p.wait()


class Security(Base):
    def test_unsigned_and_tampered(self):
        open(self.r.p("inbox", "alice", "20261107T120000Z-0123456789abcdef.job"), "wb").write(b"not a tar")
        jid = self.send("alice")
        p = self.r.p("inbox", "alice", jid + ".job")
        with tarfile.open(p) as t:
            data = t.extractfile("bundle.tgz").read()
            sig = t.extractfile("bundle.sig").read()
        evil = macbuild.bundle_bytes(json.loads(json.dumps(macbuild.manifest("alice", cmd=["touch", "/tmp/pwned"], timeout=5))))
        with tarfile.open(p, "w:") as t:  # same signature, different bundle
            for n, b in (("bundle.tgz", evil), ("bundle.sig", sig)):
                ti = tarfile.TarInfo(n)
                ti.size = len(b)
                t.addfile(ti, io.BytesIO(b))
        self.r.tick()
        self.assertIn("bad container", self.rejected("alice", "20261107T120000Z-0123456789abcdef")["reason"])
        self.assertIn("signature", self.rejected("alice", jid)["reason"])
        self.assertFalse(os.path.exists(self.r.jobdir("alice", jid)))
        self.assertTrue(data)

    def test_wrong_key_and_namespace(self):
        j1 = self.send("alice", sign_as="bob")  # bob's key, alice's inbox and manifest
        j2 = self.send("admin", sign_as="alice", manifest=macbuild.manifest("admin", cmd=["true"]))
        self.r.tick()
        self.assertIn("signature", self.rejected("alice", j1)["reason"])
        self.assertIn("signature", self.rejected("admin", j2)["reason"])
        # a user key line cannot sign admin jobs, even if listed under "admin" for the job namespace
        self.assertIn('alice namespaces="macbuild-job"', open(self.r.p("keys", "allowed_signers")).read())

    def test_failed_ingest_leaves_no_job_folder(self):
        jid = self.send("alice", secrets=[(__file__, "s")])
        ok = self.r.set_status

        def full(user, job_id, /, **kw):
            self.assertTrue(os.path.exists(os.path.join(self.r.jobdir(user, job_id), "secrets", "s")))
            raise OSError("disk full")
        self.r.set_status = full
        self.r.tick()
        self.r.set_status = ok
        self.assertIn("disk full", self.rejected("alice", jid)["reason"])
        self.assertFalse(os.path.exists(self.r.jobdir("alice", jid)), "no folder with secrets/ and no status")

    def test_job_moved_to_other_inbox(self):
        jid = self.send("alice")
        os.replace(self.r.p("inbox", "alice", jid + ".job"), self.r.p("inbox", "bob", jid + ".job"))
        self.r.tick()
        self.assertIn("signature", self.rejected("bob", jid)["reason"])

    def test_replay(self):
        jid = self.send("alice")
        saved = open(self.r.p("inbox", "alice", jid + ".job"), "rb").read()
        self.run_until(lambda: self.st("alice", jid).get("state") == "ok")
        shutil.rmtree(self.r.jobdir("alice", jid))  # even after retention removed the job
        open(self.r.p("inbox", "alice", jid + ".job"), "wb").write(saved)
        self.r.tick()
        self.assertIn("replay", self.rejected("alice", jid)["reason"])
        self.assertFalse(os.path.exists(self.r.jobdir("alice", jid)))

    def test_expired(self):
        m = macbuild.manifest("alice", cmd=["true"])
        m["created"] -= 7200
        m["expires"] = m["created"] + 3600
        jid = self.send("alice", manifest=m)
        self.r.tick()
        self.assertIn("expired", self.rejected("alice", jid)["reason"])

    def test_unsafe_bundle_entries(self):
        for name, mk in (("traversal", lambda t: self._file(t, "work/../../evil")),
                         ("symlink", lambda t: self._link(t)),
                         ("toplevel", lambda t: self._file(t, "evil.sh"))):
            m = macbuild.manifest("alice", cmd=["true"])
            buf = io.BytesIO()
            with tarfile.open(fileobj=buf, mode="w:gz") as t:
                self._file(t, "manifest.json", json.dumps(m).encode())
                mk(t)
            data = buf.getvalue()
            sig = macbuild.sign("alice", data)
            p = self.r.p("inbox", "alice", m["id"] + ".job")
            with tarfile.open(p, "w:") as t:
                for n, b in (("bundle.tgz", data), ("bundle.sig", sig)):
                    self._file(t, n, b)
            self.r.tick()
            self.assertIsNotNone(self.rejected("alice", m["id"]), name)
            self.assertFalse(os.path.exists(self.r.jobdir("alice", m["id"])), name)
        self.assertFalse(os.path.exists(os.path.join(self.tmp, "mac", "evil")))

    @staticmethod
    def _file(t, name, data=b"x"):
        ti = tarfile.TarInfo(name)
        ti.size = len(data)
        t.addfile(ti, io.BytesIO(data))

    @staticmethod
    def _link(t):
        ti = tarfile.TarInfo("work/link")
        ti.type, ti.linkname = tarfile.SYMTYPE, "/etc/passwd"
        t.addfile(ti)

    def test_unknown_user_ignored(self):
        os.makedirs(self.r.p("inbox", "mallory"))
        open(self.r.p("inbox", "mallory", "x.job"), "w").write("x")
        self.r.tick()
        self.assertTrue(os.path.exists(self.r.p("inbox", "mallory", "x.job")))
        self.assertFalse(os.path.exists(self.r.p("users", "mallory")))

    def test_partial_upload_ignored(self):
        open(self.r.p("inbox", "alice", "20261107T120000Z-0123456789abcdef.job.download"), "w").write("x")
        self.r.tick()
        self.assertTrue(os.path.exists(self.r.p("inbox", "alice", "20261107T120000Z-0123456789abcdef.job.download")))

    def test_register_via_admin_job(self):
        self.keygen("carol")
        pub = open(macbuild.keyfile("carol") + ".pub").read().strip()
        m = macbuild.manifest("admin", cmd=["/bin/sh", "-c", 'exec "$JOB_PYTHON" "$JOB_RUNNER" add-user "$@"', "sh",
                                            "carol", pub, "--quota-gb", "5", "--no-sandbox"], timeout=30)
        shutil.copy(runner.__file__, self.r.p("runner.py"))
        jid = self.send("admin", manifest=m)
        self.run_until(lambda: self.st("admin", jid).get("state") in runner.TERMINAL)
        self.assertEqual(self.st("admin", jid)["state"], "ok", open(os.path.join(self.r.jobdir("admin", jid), "job.log")).read())
        self.assertEqual(self.r.cfg()["users"]["carol"]["quota_gb"], 5)
        j2 = self.send("carol")
        self.run_until(lambda: self.st("carol", j2).get("state") == "ok")


class ClientEndToEnd(unittest.TestCase):
    """macbuild.py CLI -> fake put/get (copies, like Tether file transfer) -> runner tick -> CLI."""

    def test_cli(self):
        tmp = tempfile.mkdtemp(prefix="mbcli-")
        try:
            mac = os.path.join(tmp, "Users", "me")
            fake = os.path.join(tmp, "fake.sh")
            open(fake, "w").write('#!/bin/bash\nmap() { echo "%s${1#/Users/me}"; }\n'
                                  'case "$MODE" in put) cp "$1" "$(map "$2")" ;; get) cp "$(map "$1")" "$2" ;; esac '
                                  '&& echo \'"name":"job_done"\'\n' % mac)
            env = dict(os.environ, MACBUILD_DIR=os.path.join(tmp, "pc"), MACBUILD_MAC_HOME="/Users/me",
                       MACBUILD_LOCK=os.path.join(tmp, "lock"), BUILD_RUNNER_ROOT=os.path.join(mac, "build-runner"))
            cli = [sys.executable, os.path.join(HERE, "macbuild.py")]
            run = lambda *a, mode="put": subprocess.run(cli + list(a), env=dict(env, MODE=mode, MACBUILD_PUT=fake, MACBUILD_GET=fake),
                                                        stdout=subprocess.PIPE, stderr=subprocess.STDOUT, check=True).stdout.decode()
            for u in ("admin", "dave"):
                run("keygen", "--user", u)
            os.makedirs(os.path.join(mac, "build-runner"))
            rn = [sys.executable, os.path.join(HERE, "mac", "runner.py")]
            for u in ("admin", "dave"):
                subprocess.run(rn + ["add-user", u, open(os.path.join(tmp, "pc", u, "id_ed25519.pub")).read()], env=env, check=True,
                               stdout=subprocess.DEVNULL)
            c = runner.read_json(os.path.join(mac, "build-runner", "config.json"))
            c["users"]["dave"]["sandbox"] = False
            runner.write_json(os.path.join(mac, "build-runner", "config.json"), c)
            work = os.path.join(tmp, "work")
            os.makedirs(work)
            open(os.path.join(work, "build.sh"), "w").write("echo building; echo art > result.txt\n")
            jid = run("submit", "--user", "dave", "--work", work, "--artifact", "result.txt", "--", "bash", "build.sh").strip().splitlines()[-1]
            self.assertFalse(os.path.exists(os.path.join(tmp, "pc", "dave", "jobs", jid, jid + ".job")), "local .job deleted after upload")
            for _ in range(50):
                subprocess.run(rn + ["tick"], env=env, check=True, stdout=subprocess.DEVNULL)
                if '"ok"' in run("status", "--user", "dave", jid, mode="get"):
                    break
                time.sleep(0.2)
            self.assertIn('"state": "ok"', run("status", "--user", "dave", jid, mode="get"))
            self.assertIn("building", run("log", "--user", "dave", jid, mode="get"))
            out = os.path.join(tmp, "result.txt")
            run("fetch", "--user", "dave", jid, "result.txt", "--out", out, mode="get")
            self.assertEqual(open(out).read(), "art\n")
        finally:
            shutil.rmtree(tmp, ignore_errors=True)


if __name__ == "__main__":
    unittest.main()
