-- Every chat load, persist and task delete filters messages by task and
-- orders them by position. Without an index each of those scanned the whole
-- table, which holds every message of every chat.
CREATE INDEX IF NOT EXISTS idx_messages_task_sort ON messages(task_id, sort_order);
