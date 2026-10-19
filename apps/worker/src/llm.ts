// A language model behind any OpenAI-compatible chat API (OpenRouter by default; a self-hosted server, OpenAI, …):
// LLM_BASE_URL, LLM_API_KEY and LLM_MODEL. Without a key there is no model, and the work that needs one is skipped.
const BASE_URL = process.env.LLM_BASE_URL ?? "https://openrouter.ai/api/v1";
const API_KEY = process.env.LLM_API_KEY ?? "";
export const MODEL = process.env.LLM_MODEL ?? "deepseek/deepseek-v4-pro";
export const enabled = API_KEY !== "";

/** A JSON Schema the answer must follow (strict: every property required, no others). */
export type Schema = { name: string; schema: Record<string, unknown> };

/** Asks the model and returns its answer as the schema's JSON. */
export async function ask<T>(system: string, user: string, schema: Schema): Promise<{ answer: T; usage: unknown }> {
  const body: Record<string, unknown> = {
    model: MODEL,
    messages: [{ role: "system", content: system }, { role: "user", content: user }],
    response_format: { type: "json_schema", json_schema: { name: schema.name, strict: true, schema: schema.schema } },
  };
  // Transcripts are people's words: on OpenRouter, only providers that don't keep or train on them.
  if (new URL(BASE_URL).hostname === "openrouter.ai") body.provider = { data_collection: "deny", require_parameters: true };
  const res = await fetch(`${BASE_URL}/chat/completions`, {
    method: "POST",
    headers: { authorization: `Bearer ${API_KEY}`, "content-type": "application/json" },
    body: JSON.stringify(body),
    signal: AbortSignal.timeout(300_000),
  });
  if (!res.ok) throw new Error(`${MODEL}: ${res.status} ${(await res.text()).slice(0, 500)}`);
  const out = (await res.json()) as { choices: { message: { content: string }; finish_reason: string }[]; usage: unknown };
  const choice = out.choices[0];
  if (choice?.finish_reason !== "stop") throw new Error(`${MODEL}: stopped with ${choice?.finish_reason}`);
  return { answer: JSON.parse(choice.message.content) as T, usage: out.usage };
}
