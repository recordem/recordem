CREATE TABLE "notes" (
	"recording_id" uuid PRIMARY KEY NOT NULL,
	"workspace_id" text NOT NULL,
	"summary" text NOT NULL,
	"action_items" jsonb NOT NULL,
	"topics" jsonb NOT NULL,
	"language" text,
	"model" text NOT NULL,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL
);
--> statement-breakpoint
ALTER TABLE "recordings" ADD COLUMN "attendees" jsonb DEFAULT '[]'::jsonb NOT NULL;--> statement-breakpoint
ALTER TABLE "notes" ADD CONSTRAINT "notes_recording_id_recordings_id_fk" FOREIGN KEY ("recording_id") REFERENCES "public"."recordings"("id") ON DELETE cascade ON UPDATE no action;