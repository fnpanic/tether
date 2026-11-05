import { describe, expect, it } from "vitest";
import { formatDuration, formatSize, peerLabel, reduceCard, retentionText, uploadText, type Recording } from "./recordings";

const rec = (name: string, o: Partial<Recording> = {}): Recording => ({ name, peer_id: "184392571", peer_name: "", started_ms: 0, duration_s: 0, size: 0, pending: false, ...o });

describe("reduceCard", () => {
  it("shows saved files, with upload state from policy", () => {
    expect(reduceCard(null, { saved: [rec("a.webm")], upload: "none" })).toEqual({ files: [rec("a.webm")], upload: "none" });
    expect(reduceCard(null, { saved: [rec("a.webm")], upload: "unc" })?.upload).toBe("uploading");
    const old = { files: [rec("x.webm")], upload: "none" as const };
    expect(reduceCard(old, { saved: [], upload: "none" })).toBe(old);
  });

  it("follows the upload result of its own files only", () => {
    const card = reduceCard(null, { saved: [rec("a.webm"), rec("b.webm")], upload: "https-put" })!;
    expect(reduceCard(card, { uploaded: ["old.webm"], failed: [] })).toBe(card);
    expect(reduceCard(card, { uploaded: ["a.webm"], failed: [] })).toBe(card);
    expect(reduceCard(card, { uploaded: ["a.webm", "b.webm"], failed: [] })?.upload).toBe("uploaded");
    expect(reduceCard(card, { uploaded: ["a.webm"], failed: [["b.webm", "403"]] })).toMatchObject({ upload: "failed", error: "403" });
    expect(reduceCard(card, { uploaded: [], failed: [["other.webm", "x"]] })).toBe(card);
    expect(reduceCard(null, { uploaded: ["a.webm"], failed: [] })).toBeNull();
  });
});

describe("formatting", () => {
  it("sizes", () => {
    expect(formatSize(900)).toBe("900 B");
    expect(formatSize(1536)).toBe("1.5 KB");
    expect(formatSize(48 * 1024 * 1024)).toBe("48 MB");
    expect(formatSize(3.2 * 1024 ** 3)).toBe("3.2 GB");
  });

  it("durations", () => {
    expect(formatDuration(0)).toBe("");
    expect(formatDuration(42)).toBe("0:42");
    expect(formatDuration(725)).toBe("12:05");
    expect(formatDuration(3729)).toBe("1:02:09");
  });

  it("peer label falls back to the formatted ID", () => {
    expect(peerLabel(rec("a.webm"))).toBe("184 392 571");
    expect(peerLabel(rec("a.webm", { peer_name: "Design-Studio" }))).toBe("Design-Studio");
  });

  it("policy in words", () => {
    expect(uploadText("none", "", true)).toBe("Recordings stay on this computer.");
    expect(uploadText("unc", "\\\\srv\\rec", true)).toBe("Copied to \\\\srv\\rec after each session.");
    expect(uploadText("https-put", "https://x/rec", false)).toContain("local copy is deleted");
    expect(retentionText("0")).toBe("Kept until you delete them.");
    expect(retentionText("")).toBe("Kept until you delete them.");
    expect(retentionText("1")).toBe("Deleted automatically after 1 day.");
    expect(retentionText("30")).toBe("Deleted automatically after 30 days.");
  });
});
