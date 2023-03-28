CREATE EXTENSION IF NOT EXISTS pgcrypto;
CREATE TABLE images (
    id UUID
        PRIMARY KEY
        DEFAULT gen_random_uuid(),
    name TEXT
        NOT NULL,
    image BYTEA
        NOT NULL
);