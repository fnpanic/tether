import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { invoke, type AppState } from "../lib/ipc";
import type { SignIn } from "../lib/roles";
import { Button, ErrorText, Label, TextField } from "../components/ui";

/** First run (create or import keys), unlock, and staff sign-in where the relays offer it (a person
 * with a Tether Manage role works without the keys; only a key holder's keys sign). `onCancel`: back
 * to the app (a signed-in key holder who came here to unlock). */
export function Gate({ state, onDone, onCancel }: { state: AppState; onDone: () => void; onCancel?: () => void }) {
  const [mode, setMode] = useState<"unlock" | "create" | "import">(state.exists ? "unlock" : "create");
  const [pass, setPass] = useState("");
  const [pass2, setPass2] = useState("");
  const [file, setFile] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [signin, setSignin] = useState<SignIn>(state.signin);
  // Asks the relays whether they offer sign-in (and takes up a stored sign-in).
  useEffect(() => {
    if (onCancel) return;
    invoke<SignIn>("signin_status", { check: true }).then((s) => {
      setSignin(s);
      if (s.signed_in) onDone();
    }, () => {});
  }, []);
  const signIn = async () => {
    setError(null);
    setSignin((s) => ({ ...s, busy: true }));
    try {
      const s = await invoke<SignIn>("signin_start");
      setSignin(s);
      if (!s.manage.length) setError("Signed in, but your account has no Tether Manage role. Ask your administrator to assign one.");
      onDone();
    } catch (e: any) {
      setSignin((s) => ({ ...s, busy: false }));
      setError(e.message);
    }
  };

  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (mode === "create" && pass !== pass2) return setError("The passphrases do not match.");
    setBusy(true);
    setError(null);
    try {
      if (mode === "unlock") {
        await invoke("keys_unlock", { passphrase: pass });
        // Release indexes that expire within 15 days are re-signed (devices only warn after expiry).
        invoke("release_index_refresh").catch(() => {});
      }
      if (mode === "create") await invoke("keys_create", { passphrase: pass });
      if (mode === "import") await invoke("keys_import", { path: file, passphrase: pass });
      onDone();
    } catch (e: any) {
      setError(e.message);
    } finally {
      setBusy(false);
    }
  };

  const copy = {
    unlock: ["Unlock Tether Manage", "Enter the passphrase that protects your admin and update signing keys."],
    create: ["Set up Tether Manage", "Create the admin key (signs policies and server requests) and the update signing key. Both stay on this PC, encrypted with your passphrase."],
    import: ["Import a key backup", "Restore keys exported from Tether Manage on another PC."],
  }[mode];

  return (
    <div className="grid h-full place-items-center bg-parchment p-8">
      <form onSubmit={submit} className="animate-rise w-[440px] rounded-2xl border border-hairline bg-tile p-8">
        <h1 className="text-title font-semibold">{copy[0]}</h1>
        <p className="mt-2 text-callout text-muted">{copy[1]}</p>
        <div className="mt-6 flex flex-col gap-4">
          {mode === "import" && (
            <Label text="Backup file">
              <div className="flex gap-2">
                <TextField readOnly aria-label="Backup file" value={file} placeholder="No file chosen" />
                <Button
                  type="button"
                  onClick={async () => {
                    const f = await open({ filters: [{ name: "Tether Manage key backup", extensions: ["json", "tmkeys"] }] });
                    if (typeof f === "string") setFile(f);
                  }}
                >
                  Choose…
                </Button>
              </div>
            </Label>
          )}
          <Label text="Passphrase" hint={mode === "create" ? "At least 8 characters. There is no way to recover it: export a backup and keep the passphrase somewhere safe." : undefined}>
            <TextField type="password" autoComplete={mode === "create" ? "new-password" : "current-password"} autoFocus value={pass} onChange={(e) => setPass(e.target.value)} />
          </Label>
          {mode === "create" && (
            <Label text="Repeat passphrase">
              <TextField type="password" autoComplete="new-password" value={pass2} onChange={(e) => setPass2(e.target.value)} />
            </Label>
          )}
          <ErrorText error={error} />
          <Button variant="primary" size="lg" disabled={busy || !pass || (mode === "import" && !file)}>
            {busy ? "Working…" : mode === "unlock" ? "Unlock" : mode === "create" ? "Create keys" : "Import and unlock"}
          </Button>
          {onCancel && (
            <button type="button" className="text-caption text-accent" onClick={onCancel}>
              Back
            </button>
          )}
          {mode === "unlock" ? null : (
            <button type="button" className="text-caption text-accent" onClick={() => setMode(mode === "create" ? "import" : state.exists ? "unlock" : "create")}>
              {mode === "create" ? "Import a backup instead" : state.exists ? "Back to unlock" : "Create new keys instead"}
            </button>
          )}
        </div>
        {!onCancel && (signin.offered || signin.problem || signin.note) && (
          <div className="mt-6 border-t border-hairline pt-5">
            <h2 className="text-callout font-semibold">Sign in with your organisation</h2>
            <p className="mt-1 text-caption text-muted">Without the keys: what your Tether Manage role allows (read, operate, propose). Signing and publishing stay with a key holder's keys.</p>
            {signin.problem && <p className="mt-2 text-caption text-danger">{signin.problem}</p>}
            {signin.note && <p className="mt-2 text-caption text-muted">{signin.note}</p>}
            {signin.relays.filter((r) => !r.ok).map((r) => (
              <p key={r.relay} className="mt-1 text-caption text-warn">
                {r.relay}: {r.text}
              </p>
            ))}
            {signin.offered && (
              <div className="mt-3 flex gap-2">
                <Button type="button" disabled={signin.busy} onClick={signIn}>
                  {signin.busy ? "Finish in your web browser…" : "Sign in"}
                </Button>
                {signin.busy && (
                  <Button type="button" variant="ghost" onClick={() => invoke<SignIn>("signin_cancel").then(setSignin)}>
                    Cancel
                  </Button>
                )}
              </div>
            )}
          </div>
        )}
      </form>
    </div>
  );
}
