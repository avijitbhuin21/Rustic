-- Persist a sub-agent's full streaming activity, not just the rolled-up
-- cost/summary columns added in 006. Without these, closing the app while
-- a sub-agent was mid-run wiped the tool calls and assistant text the user
-- had been watching — they only existed in the frontend's in-memory store.
ALTER TABLE subagent_records ADD COLUMN output_text TEXT NOT NULL DEFAULT '';
ALTER TABLE subagent_records ADD COLUMN tool_calls_json TEXT NOT NULL DEFAULT '[]';
