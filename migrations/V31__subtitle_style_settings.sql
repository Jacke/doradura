ALTER TABLE users ADD COLUMN subtitle_font_size TEXT NOT NULL DEFAULT 'medium';
ALTER TABLE users ADD COLUMN subtitle_text_color TEXT NOT NULL DEFAULT 'white';
ALTER TABLE users ADD COLUMN subtitle_outline_color TEXT NOT NULL DEFAULT 'black';
ALTER TABLE users ADD COLUMN subtitle_outline_width INTEGER NOT NULL DEFAULT 2;
ALTER TABLE users ADD COLUMN subtitle_shadow INTEGER NOT NULL DEFAULT 1;
ALTER TABLE users ADD COLUMN subtitle_position TEXT NOT NULL DEFAULT 'bottom';
