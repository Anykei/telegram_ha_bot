ALTER TABLE users ADD COLUMN admission_generation TEXT NOT NULL DEFAULT '';
ALTER TABLE users ADD COLUMN private_menu_verified INTEGER NOT NULL DEFAULT 0;
UPDATE users SET admission_generation = lower(hex(randomblob(16)));
CREATE TRIGGER users_new_admission AFTER INSERT ON users
WHEN NEW.admission_generation = ''
BEGIN
    UPDATE users SET admission_generation = lower(hex(randomblob(16))) WHERE id = NEW.id;
END;
