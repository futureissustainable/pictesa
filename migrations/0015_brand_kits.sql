-- Brand kits are characters of kind 'brand': their images are brand references, and their colors
-- (a JSON list of #rrggbb) and words join the prompt of every run they are attached to.
ALTER TABLE characters ADD COLUMN kind TEXT NOT NULL DEFAULT 'character';
ALTER TABLE characters ADD COLUMN colors TEXT NOT NULL DEFAULT '[]';
ALTER TABLE characters ADD COLUMN words TEXT NOT NULL DEFAULT '';
