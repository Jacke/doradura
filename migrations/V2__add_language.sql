-- Add user language preference
ALTER TABLE users ADD COLUMN language TEXT DEFAULT 'ru';
