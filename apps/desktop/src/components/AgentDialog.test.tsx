import { describe, expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { AgentBody, type AgentResult } from "./AgentDialog";
import type { AgentAnswer } from "../lib/agent";

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
