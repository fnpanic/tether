import { describe, expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { AgentCardBody, AgentConsentBody, CARD_TEXT } from "./AgentHere";

const buttons = (s: string) => [...s.matchAll(/<button[^>]*>([^<]*)<\/button>/g)].map((m) => m[1]);
const d = { id: 3, title: "Install the health monitoring program?", paragraphs: ["“Sam” (support of Example) asks to install Tether Monitor.", "It runs in the background <b>x</b>."], yes: "Install", no: "Don't install" };

describe("this computer: the health agent through the elevation prompt", () => {
  it("the pre-dialog shows the backend's texts as plain text, the decline first", () => {
    const s = renderToStaticMarkup(<AgentConsentBody d={d} onAnswer={() => {}} />);
    expect(s).toContain("Install the health monitoring program?");
    expect(s).toContain("&lt;b&gt;x&lt;/b&gt;");
    expect(s).not.toContain("<b>");
    expect(buttons(s)).toEqual(["Don&#x27;t install", "Install"]);
    expect(s).toContain("data-autofocus");
  });
  it("the home view's card (spec 7.2): Install… and Not now", () => {
    const s = renderToStaticMarkup(<AgentCardBody busy={false} result="" onInstall={() => {}} onLater={() => {}} />);
    expect(s).toContain(CARD_TEXT);
    expect(CARD_TEXT).toBe("Your organisation asks to install its health monitoring program on this computer.");
    expect(buttons(s)).toEqual(["Install…", "Not now"]);
    const r = renderToStaticMarkup(<AgentCardBody busy={false} result="The person at the computer declined." onInstall={() => {}} onLater={() => {}} />);
    expect(r).toContain('role="status"');
    expect(buttons(renderToStaticMarkup(<AgentCardBody busy={true} result="" onInstall={() => {}} onLater={() => {}} />))).toEqual(["Installing…", "Not now"]);
  });
});
