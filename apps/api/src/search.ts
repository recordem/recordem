// Full-text search over a workspace's transcripts, meeting titles and attendees ('simple': calls mix languages, so no
// stemming). A transcript matches with every word of the query (the GIN index `transcripts_search`); its hits are the
// lines with any of them, lines with all of them ranked first. Then meeting titles above everything, attendees below.
import type { OpenAPIHono } from "@hono/zod-openapi";
import { routes } from "@recordem/contract";
import type { Db, Segment, SpeakerName } from "@recordem/db";
import { sql } from "drizzle-orm";
import type { Env } from "./auth";

type Row = {
  meeting_id: string; title: string | null; starts_at: Date | string; recording_id: string | null;
  field: "transcript" | "title" | "attendees"; seg: Segment | null; speakers: SpeakerName[] | null; rank: number;
};

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
        select m.id as meeting_id, m.title, m.starts_at, t.recording_id, 'transcript' as field, e.seg, t.speakers, e.ord,
               (2 * ts_rank(v.v, q.every) + ts_rank(v.v, q.any))::float8 as rank
        from q, transcripts t
          join recordings r on r.id = t.recording_id
          join meetings m on m.id = r.meeting_id,
          jsonb_array_elements(t.segments) with ordinality as e(seg, ord),
          lateral (select to_tsvector('simple', e.seg->>'text') as v) v
        where t.workspace_id = ${ws} and m.workspace_id = ${ws} ${meeting}
          and jsonb_to_tsvector('simple', jsonb_path_query_array(t.segments, '$[*].text'), '["string"]') @@ q.every
          and v.v @@ q.any
        union all
        select m.id, m.title, m.starts_at, null, 'title', null, null, 0,
               (1 + ts_rank(to_tsvector('simple', m.title), q.every))::float8
        from q, meetings m
        where m.workspace_id = ${ws} ${meeting} and to_tsvector('simple', coalesce(m.title, '')) @@ q.every
        union all
        select m.id, m.title, m.starts_at, r.id, 'attendees', null, null, 0,
               (0.1 * ts_rank(jsonb_to_tsvector('simple', r.attendees, '["string"]'), q.every))::float8
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
        rank: Math.round(h.rank * 1e4) / 1e4,
      })),
      next: rows.length > limit ? String(offset + limit) : null,
    }, 200);
  });
}
