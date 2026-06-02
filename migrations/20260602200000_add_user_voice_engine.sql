ALTER TABLE user_profiles
ADD COLUMN voice_command_engine TEXT NOT NULL DEFAULT 'local_parser'
CHECK(voice_command_engine IN ('local_parser', 'ha_conversation_readonly', 'ha_conversation_full'));
