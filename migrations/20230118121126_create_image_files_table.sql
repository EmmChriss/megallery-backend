CREATE TABLE image_files (
    image_id UUID
        NOT NULL,
    width INT
        NOT NULL,
    height INT
        NOT NULL,
    file_name TEXT
        NOT NULL,
        
    PRIMARY KEY(image_id, width, height),
    CONSTRAINT fk_image
        FOREIGN KEY(image_id) 
        REFERENCES images(id)
        ON DELETE CASCADE
        ON UPDATE CASCADE
);