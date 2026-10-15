/** A call's identity from its link or window title: "meet:abc-defg-hij", "zoom:81234567890", "teams:<path>". */
export function callKey(s: string | null | undefined): string | null {
  if (!s) return null;
  const meet = s.match(/\b([a-z]{3}-[a-z]{4}-[a-z]{3})\b/);
  if (meet && (/meet\.google\.com/.test(s) || !/https?:/.test(s))) return `meet:${meet[1]}`;
  const zoom = s.match(/zoom\.us\/(?:j|w|wc(?:\/join)?|s)\/(\d{9,12})/);
  if (zoom) return `zoom:${zoom[1]}`;
  const teams = s.match(/teams\.(?:microsoft|live)\.com\/(?:l\/meetup-join\/[^?\s]+|meet\/\d+)/);
  if (teams) return `teams:${decodeURIComponent(teams[0])}`;
  return null;
}
