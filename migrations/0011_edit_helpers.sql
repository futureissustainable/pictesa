-- Edit helpers (painted masks, expanded canvases) are stored with source 'edit' and point at the
-- image they were made from, so the library can hide them and the viewer can show their origin.
ALTER TABLE assets ADD COLUMN origin_id TEXT;
