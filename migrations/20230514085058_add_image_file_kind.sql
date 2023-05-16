-- first add column, default values
ALTER TABLE image_files ADD kind int NOT NULL DEFAULT 1;

-- remove default
ALTER TABLE image_files ALTER COLUMN kind DROP DEFAULT;

-- recreate primary key
ALTER TABLE image_files DROP CONSTRAINT image_files_pkey;
ALTER TABLE image_files ADD PRIMARY KEY (image_id, width, height, kind);