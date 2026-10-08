ALTER TABLE guild_settings RENAME COLUMN model TO team_model;
ALTER TABLE guild_settings ADD COLUMN member_model TEXT;
UPDATE guild_settings SET member_model = team_model;
