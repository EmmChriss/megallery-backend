ALTER TABLE collections ADD finalized boolean 
    NOT NULL
    DEFAULT FALSE;

-- remove default collection
ALTER TABLE images ALTER COLUMN collection_id DROP DEFAULT;
ALTER TABLE images ALTER COLUMN collection_id SET NOT NULL;