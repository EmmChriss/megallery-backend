CREATE TABLE collections(
    id UUID
        PRIMARY KEY
        DEFAULT gen_random_uuid(),
    name TEXT
        NOT NULL
);

INSERT INTO collections (id, name) VALUES ('52edbd3b-0f3f-468f-a90d-eafab093281e', 'Default');

ALTER TABLE images ADD collection_id UUID
    DEFAULT '52edbd3b-0f3f-468f-a90d-eafab093281e'
    REFERENCES collections(id)
        ON DELETE SET DEFAULT
        ON UPDATE SET DEFAULT;