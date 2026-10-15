ALTER TABLE "recordings" ADD COLUMN "participants" jsonb;--> statement-breakpoint
ALTER TABLE "transcripts" ADD COLUMN "speakers" jsonb DEFAULT '[]'::jsonb NOT NULL;