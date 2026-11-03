import { useState } from "react";
import { useApp } from "../App";
import { api, invoke } from "../lib/ipc";
import { Button, Card, ErrorText, Label, PageHeader, TextArea, TextField, Toggle } from "../components/ui";

export function DownloadPage() {
  const { state, reload, toast } = useApp();
  const s = state.settings;
  const [title, setTitle] = useState<string>(s.page_title ?? "Remote support");
  const [intro, setIntro] = useState<string>(s.page_intro ?? "Download Tether to get help from our support team.");
  const [technician, setTechnician] = useState<boolean>(s.page_technician ?? true);
  const [customer, setCustomer] = useState<boolean>(s.page_customer ?? true);
  const [html, setHtml] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const render = async () => {
    setError(null);
    try {
      const h = await invoke<string>("page_render", { title, intro, technician, customer });
      setHtml(h);
      return h;
    } catch (e: any) {
      setError(e.message);
      return null;
    }
  };

  const publish = async () => {
    setBusy(true);
    try {
      const h = await render();
      if (!h) return;
      await invoke("page_publish", { html: h });
      await api.saveSettings({ ...s, page_title: title, page_intro: intro, page_technician: technician, page_customer: customer });
      await reload();
      toast("Download page published");
    } catch (e: any) {
      setError(e.message);
    } finally {
      setBusy(false);
    }
  };

  return (
    <>
      <PageHeader title="Download page" sub={`Served at ${s.server}/ — links always point to the current stable release.`}>
        <Button onClick={render}>Preview</Button>
        <Button variant="primary" disabled={busy || !title.trim() || (!technician && !customer)} onClick={publish}>
          {busy ? "Publishing…" : "Publish"}
        </Button>
      </PageHeader>
      <div className="grid grid-cols-[340px_minmax(0,1fr)] items-start gap-6">
        <Card className="flex flex-col gap-4">
          <Label text="Title">
            <TextField value={title} onChange={(e) => setTitle(e.target.value)} />
          </Label>
          <Label text="Introduction" hint="Separate paragraphs with an empty line.">
            <TextArea rows={6} value={intro} onChange={(e) => setIntro(e.target.value)} />
          </Label>
          <div className="flex items-center justify-between">
            <span className="text-caption">Technician installer</span>
            <Toggle label="Offer technician installer" checked={technician} onChange={setTechnician} />
          </div>
          <div className="flex items-center justify-between">
            <span className="text-caption">Customer module (TetherQS.exe)</span>
            <Toggle label="Offer customer module" checked={customer} onChange={setCustomer} />
          </div>
          <ErrorText error={error} />
        </Card>
        <div className="h-[560px] overflow-hidden rounded-2xl border border-hairline bg-tile">
          {html ? (
            <iframe title="Download page preview" sandbox="" srcDoc={html} className="h-full w-full" />
          ) : (
            <p className="grid h-full place-items-center text-caption text-muted">Preview appears here.</p>
          )}
        </div>
      </div>
    </>
  );
}
