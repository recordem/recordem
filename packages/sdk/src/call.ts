// One request to the API: the token in a header, the query without its empty values, a JSON or binary body. A 2xx
// answer is the result (JSON, a Blob, or nothing for 204); anything else throws a RecordemError with the API's own
// error code and message.

/** The API answered with an error: `status` (401, 403, 404, …), `code` (the API's `error`, e.g. "not_found"), `message`. */
export class RecordemError extends Error {
  constructor(readonly status: number, readonly code: string, message: string) {
    super(message);
    this.name = "RecordemError";
  }
}

export type Call = <T>(method: string, path: string, as: "json" | "blob" | "none", query?: object,
  body?: unknown, type?: string) => Promise<T>;

export function caller(baseUrl: string, token: () => Promise<string> | string, f: typeof fetch = fetch): Call {
  return async (method, path, as, query, body, type) => {
    const q = new URLSearchParams();
    for (const [k, v] of Object.entries(query ?? {})) if (v !== undefined && v !== null) q.set(k, String(v));
    const binary = type !== undefined;
    const res = await f(`${baseUrl}${path}${q.size ? `?${q}` : ""}`, {
      method,
      headers: {
        authorization: `Bearer ${await token()}`,
        ...(body === undefined ? {} : { "content-type": binary ? type : "application/json" }),
      },
      body: body === undefined ? undefined : binary ? (body as BodyInit) : JSON.stringify(body),
    });
    if (!res.ok) {
      const e = await res.json().catch(() => null) as { error?: string; message?: string } | null;
      throw new RecordemError(res.status, e?.error ?? `http_${res.status}`, e?.message ?? res.statusText);
    }
    if (as === "none" || res.status === 204) return undefined as never;
    return (as === "blob" ? res.blob() : res.json()) as never;
  };
}
