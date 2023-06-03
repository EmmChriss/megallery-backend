-- set null values to empty json
UPDATE images SET metadata = '{}'::jsonb WHERE metadata IS NULL;

-- add not null constraint
ALTER TABLE images ALTER COLUMN metadata SET NOT NULL;