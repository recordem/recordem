import type { Segment } from "@recordem/sdk";

export const time = (iso: string) => new Date(iso).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
export const day = (d: Date) => d.toLocaleDateString([], { weekday: "long", day: "numeric", month: "long" });
export const monthTitle = (d: Date) => d.toLocaleDateString([], { month: "long", year: "numeric" });
export const dayKey = (d: Date) => `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;

export function duration(startIso: string, endIso: string | null) {
  if (!endIso) return null;
  const min = Math.round((new Date(endIso).getTime() - new Date(startIso).getTime()) / 60_000);
  return min < 60 ? `${min} min` : `${Math.floor(min / 60)} h ${String(min % 60).padStart(2, "0")}`;
}

export const clock = (ms: number) => {
  const s = Math.floor(ms / 1000);
  const h = Math.floor(s / 3600), m = Math.floor((s % 3600) / 60), r = s % 60;
  return `${h ? `${h}:${String(m).padStart(2, "0")}` : m}:${String(r).padStart(2, "0")}`;
};

/** "You" for the recorder's own mic; the other side is "Them" when it is one voice, numbered when several. */
export function speakerName(s: Segment, all: Segment[]) {
  if (s.channel === "mic") return "You";
  const voices = new Set(all.filter((x) => x.channel === "system").map((x) => x.speaker ?? "system"));
  if (voices.size <= 1) return "Them";
  return `Speaker ${(s.speaker ?? "").split(":")[1] ?? "?"}`;
}

export const callApp = (key: string | null) =>
  key?.startsWith("meet:") ? "Google Meet" : key?.startsWith("zoom:") ? "Zoom" : key?.startsWith("teams:") ? "Teams" : null;
