CREATE TABLE "rebuild_Settings" (
    "SettingId" TEXT NOT NULL CONSTRAINT "PK_Settings" PRIMARY KEY,
    "Value" TEXT NULL
);

INSERT INTO "rebuild_Settings" ("SettingId", "Value")
SELECT "SettingId", "Value"
FROM "Settings";

DROP TABLE "Settings";

ALTER TABLE "rebuild_Settings" RENAME TO "Settings";
