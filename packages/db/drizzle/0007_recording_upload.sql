ALTER TABLE "recordings" ADD COLUMN "for_user" text;--> statement-breakpoint
ALTER TABLE "recordings" ADD COLUMN "uploaded_at" timestamp with time zone;--> statement-breakpoint
ALTER TABLE "recordings" ADD COLUMN "failure" jsonb;