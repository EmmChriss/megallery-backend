-- add file extension row
ALTER TABLE image_files ADD extension text DEFAULT NULL;

-- set calculated values
UPDATE image_files SET extension = substring(file_name from '\.([^\.]*)$');

-- remove file_name column
ALTER TABLE image_files DROP file_name;

-- remove nullable constraint
ALTER TABLE image_files ALTER COLUMN extension SET NOT NULL;