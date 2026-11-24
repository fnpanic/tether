import { describe, expect, it, vi } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { AgentBody, agentRunner, type AgentResult } from "./AgentDialog";
import { invoke } from "../lib/ipc";
import type { AgentAnswer } from "../lib/agent";

vi.mock("../lib/ipc", () => ({ invoke: vi.fn() }));
const ipc = vi.mocked(invoke);

const ans = (state: string, p: Partial<AgentAnswer> = {}): AgentAnswer => ({ state, version: "", installed_by: "", detail: "", at: 0, ...p });
const html = (r: AgentResult, p: Partial<Parameters<typeof AgentBody>[0]> = {}) =>
  renderToStaticMarkup(<AgentBody r={r} busy={null} err="" ask={null} onOffer={() => {}} onConfirm={() => {}} onBack={() => {}} onClose={() => {}} {...p} />);
const buttons = (s: string) => [...s.matchAll(/<button[^>]*>([^<]*)<\/button>/g)].map((m) => m[1]);

describe("session agent dialog", () => {
  it("each state shows its sentence and its buttons, Close always", () => {
    expect(buttons(html({ a: ans("not-installed"), did: "status" }))).toEqual(["Install…", "Close"]);
    expect(buttons(html({ a: ans("installed", { version: "0.2.0" }), did: "status" }))).toEqual(["Remove…", "Close"]);
    expect(buttons(html({ a: ans("removed-locally", { at: 1_795_000_000 }), did: "status" }))).toEqual(["Install again…", "Close"]);
    expect(buttons(html({ a: ans("running"), did: "install" }))).toEqual(["Ask again", "Close"]);
    expect(buttons(html({ a: ans("not-allowed"), did: "status" }))).toEqual(["Close"]);
    expect(html({ a: ans("needs-service"), did: "status" })).toContain("Needs Tether&#x27;s service on the remote computer");
  });
  it("while asking: the progress line and no action buttons", () => {
    const s = html(null, { busy: "install" });
    expect(s).toContain("Installing on the remote computer… This can take up to three minutes.");
    expect(buttons(s)).toEqual(["Close"]);
  });
  it("text from the remote computer is plain text: no markup, no link", () => {
    const s = html({ a: ans("failed", { detail: '<a href="https://evil.example">click</a><img src=x onerror=alert(1)>', version: "<b>1</b>" }), did: "install" });
    expect(s).not.toMatch(/<a |<img|<b>/);
    expect(s).toContain("&lt;a href=");
    expect(html(null, { err: "<script>x</script>" })).not.toContain("<script>");
  });
  it("Install again warns with the date before the confirmation", () => {
    const s = html({ a: ans("removed-locally", { at: 1_795_000_000 }), did: "status" }, { ask: "reinstall" });
    expect(s).toContain('role="alert"');
    expect(s).toMatch(/Warning: Someone at the remote computer removed the health monitoring agent on .+\. Install it again only if they agree/);
    expect(buttons(s)).toEqual(["Cancel", "Install again"]);
  });
  it("Remove asks first", () => {
    expect(buttons(html({ a: ans("installed"), did: "status" }, { ask: "remove" }))).toEqual(["Cancel", "Remove"]);
  });
});

// The dialog's logic without a DOM: the runner is what AgentDialog drives its state with.
describe("session agent dialog: requests", () => {
  const collect = () => {
    const st = { r: null as AgentResult, busy: null as string | null, err: "" };
    return { st, on: { busy: (a: string | null) => void (st.busy = a), result: (r: AgentResult) => void (st.r = r), error: (e: string) => void (st.err = e) } };
  };
  const tick = () => new Promise((r) => setTimeout(r, 0));
  it("a rejected request shows the error line and brings the buttons back", async () => {
    ipc.mockRejectedValueOnce("the host did not answer");
    const { st, on } = collect();
    agentRunner("k", on).run("install");
    expect(st.busy).toBe("install");
    await tick();
    expect(st.busy).toBeNull();
    const s = html(st.r, { busy: st.busy as null, err: st.err });
    expect(s).toContain("Could not ask the remote computer: the host did not answer");
    expect(buttons(s)).toEqual(["Ask again", "Close"]);
  });
  it("an answer that arrives after the dialog was closed is dropped", async () => {
    let done!: (a: AgentAnswer) => void;
    ipc.mockReturnValueOnce(new Promise((r) => (done = r)));
    const { st, on } = collect();
    const run = agentRunner("k", on);
    run.run("status");
    run.stop();
    done(ans("installed"));
    await tick();
    expect(st).toEqual({ r: null, busy: "status", err: "" });
  });
});
