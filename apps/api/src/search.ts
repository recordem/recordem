// Full-text search over a workspace's meetings ('simple': calls mix languages, so no stemming). A transcript, or a
// recording's notes, matches with every word of the query (for transcripts, through the GIN index
// `transcripts_search`); its hits are the lines, or parts of the notes, with any of them, those with all of them
// ranked first. Tiers before score: meeting titles, then notes, then transcript lines, then attendees.
import type { OpenAPIHono } from "@hono/zod-openapi";
import { routes } from "@recordem/contract";
import type { Db, Segment, SpeakerName } from "@recordem/db";
import { sql } from "drizzle-orm";
import type { Env } from "./auth";

type Row = {
  meeting_id: string; title: string | null; starts_at: Date | string; recording_id: string | null;
  field: "notes" | "transcript" | "title" | "attendees"; seg: Segment | null; speakers: SpeakerName[] | null;
  part: "summary" | "action_item" | "topic" | null; topic: string | null; text: string | null; rank: number;
};

/** How well a text matches, 0–1: all the words count twice as much as any of them. */
const score = (v: ReturnType<typeof sql>) => sql`least(0.999, (2 * ts_rank(${v}, q.every) + ts_rank(${v}, q.any)) / 3)::float8`;

export function mountSearch(app: OpenAPIHono<Env>, db: Db) {
  // Cursor: how many hits came before (ranked results have no key to resume from).
  app.openapi(routes.search, async (c) => {
    const { q, limit, cursor, from, to, meeting_id } = c.req.valid("query");
    const ws = c.get("actor").workspace;
    const offset = Math.max(0, Number(cursor) || 0);
    const meeting = sql`${from ? sql`and m.starts_at >= ${from}::timestamptz` : sql``}
      ${to ? sql`and m.starts_at < ${to}::timestamptz` : sql``}
      ${meeting_id ? sql`and m.id = ${meeting_id}::uuid` : sql``}`;
    const rows = await db.execute<Row>(sql`
      with q as (
        select websearch_to_tsquery('simple', ${q}) as every,
               coalesce((select string_agg(quote_literal(w), ' | ') from unnest(tsvector_to_array(to_tsvector('simple', ${q}))) w), '')::tsquery as any
      )
      select * from (
        select m.id as meeting_id, m.title, m.starts_at, null::uuid as recording_id, 'title' as field, null::jsonb as seg, null::jsonb as speakers,
               null as part, null as topic, null as text, 0::bigint as ord, 3 + ${score(sql`to_tsvector('simple', m.title)`)} as rank
        from q, meetings m
        where m.workspace_id = ${ws} ${meeting} and to_tsvector('simple', coalesce(m.title, '')) @@ q.every
        union all
        select m.id, m.title, m.starts_at, n.recording_id, 'notes', null, null, u.part, u.topic, u.text, u.ord, 2 + ${score(sql`u.v`)}
        from q, notes n
          join recordings r on r.id = n.recording_id
          join meetings m on m.id = r.meeting_id,
          lateral (
            select part, topic, text, ord, to_tsvector('simple', text) as v from (
              select 'summary' as part, null as topic, n.summary as text, 0::bigint as ord
              union all
              select 'action_item', null, (a->>'text') || coalesce(' (' || (a->>'owner') || ')', ''), i
              from jsonb_array_elements(n.action_items) with ordinality as x(a, i)
              union all
              select 'topic', t->>'title', t->>'title', 1000 * i
              from jsonb_array_elements(n.topics) with ordinality as y(t, i)
              union all
              select 'topic', t->>'title', p, 1000 * i + j
              from jsonb_array_elements(n.topics) with ordinality as y(t, i), jsonb_array_elements_text(t->'points') with ordinality as z(p, j)
            ) parts
          ) u
        where n.workspace_id = ${ws} and m.workspace_id = ${ws} ${meeting}
          and to_tsvector('simple', n.summary || ' ' || n.action_items::text || ' ' || n.topics::text) @@ q.every
          and u.v @@ q.any
        union all
        select m.id, m.title, m.starts_at, t.recording_id, 'transcript', e.seg, t.speakers, null, null, null, e.ord, 1 + ${score(sql`v.v`)}
        from q, transcripts t
          join recordings r on r.id = t.recording_id
          join meetings m on m.id = r.meeting_id,
          jsonb_array_elements(t.segments) with ordinality as e(seg, ord),
          lateral (select to_tsvector('simple', e.seg->>'text') as v) v
        where t.workspace_id = ${ws} and m.workspace_id = ${ws} ${meeting}
          and jsonb_to_tsvector('simple', jsonb_path_query_array(t.segments, '$[*].text'), '["string"]') @@ q.every
          and v.v @@ q.any
        union all
        select m.id, m.title, m.starts_at, r.id, 'attendees', null, null, null, null, null, 0, ${score(sql`jsonb_to_tsvector('simple', r.attendees, '["string"]')`)}
        from q, recordings r join meetings m on m.id = r.meeting_id
        where r.workspace_id = ${ws} ${meeting} and jsonb_to_tsvector('simple', r.attendees, '["string"]') @@ q.every
      ) h
      order by rank desc, starts_at desc, recording_id, ord
      limit ${limit + 1} offset ${offset}`);
    const page = [...rows].slice(0, limit);
    return c.json({
      items: page.map((h) => ({
        meeting: { id: h.meeting_id, title: h.title, starts_at: new Date(h.starts_at).toISOString() },
        recording_id: h.recording_id,
        field: h.field,
        segment: h.seg && {
          start_ms: h.seg.start_ms, end_ms: h.seg.end_ms, speaker: h.seg.speaker ?? null,
          speaker_name: h.speakers?.find((s) => s.label === h.seg!.speaker)?.name ?? null,
          text: h.seg.text,
        },
        notes: h.part && { part: h.part, topic: h.topic, text: h.text ?? "" },
        rank: Math.round(h.rank * 1e4) / 1e4,
      })),
      next: rows.length > limit ? String(offset + limit) : null,
    }, 200);
  });
}
