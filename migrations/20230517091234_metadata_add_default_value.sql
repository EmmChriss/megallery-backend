-- add default empty metadata
ALTER TABLE images ALTER COLUMN metadata SET DEFAULT '{}'::jsonb;
