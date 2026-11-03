import { useEffect, useState } from "react";
import { ask, open } from "@tauri-apps/plugin-dialog";
import { RefreshCw, Trash2 } from "lucide-react";
import { useApp } from "../App";
import { api, bytes, invoke, when } from "../lib/ipc";
import { Button, Card, ErrorText, IconButton, Label, PageHeader, Segmented, Table, TextArea, TextField, Toggle } from "../components/ui";

interface Release {
  channel: string;
  path: string;
  size: number;
  modified: number;
}
type Kind = "technician" | "customer";

export function Releases() {
  const { toast } = useApp();
  const [file, setFile] = useState("");
  const [kind, setKind] = useState<Kind>("technician");
  const [channel, setChannel] = useState("stable");
  const [version, setVersion] = useState("");
  const [notes, setNotes] = useState("");
  const [mandatory, setMandatory] = useState(false);
  const [minimumVersion, setMinimumVersion] = useState("");
  const [forced, setForced] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [list, setList] = useState<Release[] | null>(null);
  const [listError, setListError] = useState<string | null>(null);

  const load = () => api.admin<Release[]>("GET", "/api/admin/releases").then(setList, (e) => setListError(e.message));
  useEffect(() => void load(), []);

  const pick = async () => {
    const f = await open({ filters: [{ name: "Windows executable", extensions: ["exe"] }] });
    if (typeof f !== "string") return;
    setFile(f);
    const name = f.split(/[\\/]/).pop() ?? "";
    const m = name.match(/^Tether_(.+)_x64-setup\.exe$/i);
    if (m) {
      setKind("technician");
      setVersion(m[1]);
    } else if (/^TetherQS/i.test(name)) setKind("customer");
  };

  const publish = async () => {
    setBusy(true);
    setError(null);
    try {
      await invoke<string>("release_publish", {
        args: { file, kind, channel: channel.trim(), version: version.trim(), notes, mandatory, minimum_version: minimumVersion.trim(), forced },
      });
      toast(`Released ${version} to ${channel}`);
      setFile("");
      load();
    } catch (e: any) {
      setError(e.message);
    } finally {
      setBusy(false);
    }
  };

  const remove = async (r: Release) => {
    if (!(await ask(`Delete ${r.channel}/${r.path} from the server?`, { title: "Delete file", kind: "warning" }))) return;
    try {
      await api.admin("DELETE", `/api/admin/release/${r.channel}/${r.path}`);
      load();
    } catch (e: any) {
      setListError(e.message);
    }
  };

  const target = kind === "technician" ? `Tether_${version || "<version>"}_x64-setup.exe` : "TetherQS.exe";

  return (
    <>
      <PageHeader title="Releases" sub="Sign an installer with the update key and publish it to an update channel." />
      <Card>
        <div className="grid grid-cols-2 gap-x-6 gap-y-4">
          <Label text="File" hint={file ? `Uploaded as ${channel}/${kind}/windows-x86_64/${target}` : "Tether_<version>_x64-setup.exe (technician) or TetherQS.exe (customer)"}>
            <div className="flex gap-2">
              <TextField readOnly value={file} placeholder="No file chosen" />
              <Button onClick={pick}>Choose…</Button>
            </div>
          </Label>
          <Label text="App">
            <Segmented<Kind> value={kind} onChange={setKind} options={[{ value: "technician", label: "Technician app" }, { value: "customer", label: "Customer module" }]} />
          </Label>
          <Label text="Version">
            <TextField value={version} onChange={(e) => setVersion(e.target.value)} placeholder="0.3.1" />
          </Label>
          <Label text="Channel" hint="Devices follow the policy setting update.channel.">
            <TextField value={channel} onChange={(e) => setChannel(e.target.value)} />
          </Label>
          <div className="col-span-2">
            <Label text="Release notes">
              <TextArea rows={3} value={notes} onChange={(e) => setNotes(e.target.value)} />
            </Label>
          </div>
          <div className="flex items-center justify-between gap-4">
            <div>
              <p className="text-caption font-semibold">Mandatory</p>
              <p className="text-footnote text-muted">Must be installed before the app can be used for sessions.</p>
            </div>
            <Toggle label="Mandatory" checked={mandatory} onChange={setMandatory} />
          </div>
          <div className="flex items-center justify-between gap-4">
            <div>
              <p className="text-caption font-semibold">Forced</p>
              <p className="text-footnote text-muted">Install without asking after the policy countdown.</p>
            </div>
            <Toggle label="Forced" checked={forced} onChange={setForced} />
          </div>
          <Label text="Minimum version" hint="Older versions are blocked. Leave empty for none.">
            <TextField value={minimumVersion} onChange={(e) => setMinimumVersion(e.target.value)} placeholder="0.3.0" />
          </Label>
        </div>
        <div className="mt-6 flex items-center gap-4">
          <div className="flex-1">
            <ErrorText error={error} />
          </div>
          <Button variant="primary" size="lg" disabled={busy || !file || !version.trim()} onClick={publish}>
            {busy ? "Signing and uploading…" : "Sign and publish"}
          </Button>
        </div>
      </Card>

      <div className="mb-3 mt-10 flex items-center justify-between">
        <h2 className="text-headline font-semibold">Files on the server</h2>
        <IconButton label="Refresh" onClick={load}>
          <RefreshCw size={16} />
        </IconButton>
      </div>
      <div className="mb-3">
        <ErrorText error={listError} />
      </div>
      <Table head={["Channel", "Path", "Size", "Modified", ""]} empty={list && !list.length ? "No releases yet." : false}>
        {(list ?? [])
          .sort((a, b) => (a.channel + a.path).localeCompare(b.channel + b.path))
          .map((r) => (
            <tr key={r.channel + r.path}>
              <td>{r.channel}</td>
              <td className="font-mono text-footnote select-text">{r.path}</td>
              <td>{bytes(r.size)}</td>
              <td>{when(r.modified)}</td>
              <td className="w-12">
                <IconButton label={`Delete ${r.path}`} onClick={() => remove(r)}>
                  <Trash2 size={15} />
                </IconButton>
              </td>
            </tr>
          ))}
      </Table>
    </>
  );
}
