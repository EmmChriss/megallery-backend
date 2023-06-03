-- add metadata column
ALTER TABLE images ADD metadata JSONB;

-- create an index for it
CREATE INDEX metadata_index ON images USING GIN(metadata);
