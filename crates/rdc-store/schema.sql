-- The current layout, statement for statement identical to a database that
-- went through every migration (hence the late position of Downloads.FileName).

CREATE TABLE "AspNetRoles" (
    "Id" TEXT NOT NULL CONSTRAINT "PK_AspNetRoles" PRIMARY KEY,
    "Name" TEXT NULL,
    "NormalizedName" TEXT NULL,
    "ConcurrencyStamp" TEXT NULL
);

CREATE TABLE "AspNetUsers" (
    "Id" TEXT NOT NULL CONSTRAINT "PK_AspNetUsers" PRIMARY KEY,
    "UserName" TEXT NULL,
    "NormalizedUserName" TEXT NULL,
    "Email" TEXT NULL,
    "NormalizedEmail" TEXT NULL,
    "EmailConfirmed" INTEGER NOT NULL,
    "PasswordHash" TEXT NULL,
    "SecurityStamp" TEXT NULL,
    "ConcurrencyStamp" TEXT NULL,
    "PhoneNumber" TEXT NULL,
    "PhoneNumberConfirmed" INTEGER NOT NULL,
    "TwoFactorEnabled" INTEGER NOT NULL,
    "LockoutEnd" TEXT NULL,
    "LockoutEnabled" INTEGER NOT NULL,
    "AccessFailedCount" INTEGER NOT NULL
);

CREATE TABLE "AspNetRoleClaims" (
    "Id" INTEGER NOT NULL CONSTRAINT "PK_AspNetRoleClaims" PRIMARY KEY AUTOINCREMENT,
    "RoleId" TEXT NOT NULL,
    "ClaimType" TEXT NULL,
    "ClaimValue" TEXT NULL,
    CONSTRAINT "FK_AspNetRoleClaims_AspNetRoles_RoleId" FOREIGN KEY ("RoleId") REFERENCES "AspNetRoles" ("Id") ON DELETE RESTRICT
);

CREATE TABLE "AspNetUserClaims" (
    "Id" INTEGER NOT NULL CONSTRAINT "PK_AspNetUserClaims" PRIMARY KEY AUTOINCREMENT,
    "UserId" TEXT NOT NULL,
    "ClaimType" TEXT NULL,
    "ClaimValue" TEXT NULL,
    CONSTRAINT "FK_AspNetUserClaims_AspNetUsers_UserId" FOREIGN KEY ("UserId") REFERENCES "AspNetUsers" ("Id") ON DELETE RESTRICT
);

CREATE TABLE "AspNetUserLogins" (
    "LoginProvider" TEXT NOT NULL,
    "ProviderKey" TEXT NOT NULL,
    "ProviderDisplayName" TEXT NULL,
    "UserId" TEXT NOT NULL,
    CONSTRAINT "PK_AspNetUserLogins" PRIMARY KEY ("LoginProvider", "ProviderKey"),
    CONSTRAINT "FK_AspNetUserLogins_AspNetUsers_UserId" FOREIGN KEY ("UserId") REFERENCES "AspNetUsers" ("Id") ON DELETE RESTRICT
);

CREATE TABLE "AspNetUserRoles" (
    "UserId" TEXT NOT NULL,
    "RoleId" TEXT NOT NULL,
    CONSTRAINT "PK_AspNetUserRoles" PRIMARY KEY ("UserId", "RoleId"),
    CONSTRAINT "FK_AspNetUserRoles_AspNetRoles_RoleId" FOREIGN KEY ("RoleId") REFERENCES "AspNetRoles" ("Id") ON DELETE RESTRICT,
    CONSTRAINT "FK_AspNetUserRoles_AspNetUsers_UserId" FOREIGN KEY ("UserId") REFERENCES "AspNetUsers" ("Id") ON DELETE RESTRICT
);

CREATE TABLE "AspNetUserTokens" (
    "UserId" TEXT NOT NULL,
    "LoginProvider" TEXT NOT NULL,
    "Name" TEXT NOT NULL,
    "Value" TEXT NULL,
    CONSTRAINT "PK_AspNetUserTokens" PRIMARY KEY ("UserId", "LoginProvider", "Name"),
    CONSTRAINT "FK_AspNetUserTokens_AspNetUsers_UserId" FOREIGN KEY ("UserId") REFERENCES "AspNetUsers" ("Id") ON DELETE RESTRICT
);

CREATE TABLE "Settings" (
    "SettingId" TEXT NOT NULL CONSTRAINT "PK_Settings" PRIMARY KEY,
    "Value" TEXT NULL
);

CREATE TABLE "Torrents" (
    "TorrentId" TEXT NOT NULL CONSTRAINT "PK_Torrents" PRIMARY KEY,
    "Added" TEXT NOT NULL,
    "Category" TEXT NULL,
    "ClientKind" INTEGER NULL,
    "Completed" TEXT NULL,
    "DeleteOnError" INTEGER NOT NULL,
    "DownloadAction" INTEGER NOT NULL,
    "DownloadClient" INTEGER NOT NULL,
    "DownloadManualFiles" TEXT NULL,
    "DownloadMinSize" INTEGER NOT NULL,
    "DownloadRetryAttempts" INTEGER NOT NULL,
    "Error" TEXT NULL,
    "ExcludeRegex" TEXT NULL,
    "FilesSelected" TEXT NULL,
    "FinishedAction" INTEGER NOT NULL,
    "FinishedActionDelay" INTEGER NOT NULL,
    "Hash" TEXT NOT NULL,
    "HostDownloadAction" INTEGER NOT NULL,
    "IncludeRegex" TEXT NULL,
    "IsFile" INTEGER NOT NULL,
    "Lifetime" INTEGER NOT NULL,
    "Priority" INTEGER NULL,
    "RdAdded" TEXT NULL,
    "RdEnded" TEXT NULL,
    "RdFiles" TEXT NULL,
    "RdHost" TEXT NULL,
    "RdId" TEXT NULL,
    "RdName" TEXT NULL,
    "RdProgress" INTEGER NULL,
    "RdSeeders" INTEGER NULL,
    "RdSize" INTEGER NULL,
    "RdSpeed" INTEGER NULL,
    "RdSplit" INTEGER NULL,
    "RdStatus" INTEGER NULL,
    "RdStatusRaw" TEXT NULL,
    "Retry" TEXT NULL,
    "RetryCount" INTEGER NOT NULL,
    "TorrentRetryAttempts" INTEGER NOT NULL,
    "Type" INTEGER NOT NULL
);

CREATE TABLE "Downloads" (
    "DownloadId" TEXT NOT NULL CONSTRAINT "PK_Downloads" PRIMARY KEY,
    "Added" TEXT NOT NULL,
    "Completed" TEXT NULL,
    "DownloadFinished" TEXT NULL,
    "DownloadQueued" TEXT NULL,
    "DownloadStarted" TEXT NULL,
    "Error" TEXT NULL,
    "Link" TEXT NULL,
    "Path" TEXT NOT NULL,
    "RemoteId" TEXT NULL,
    "RetryCount" INTEGER NOT NULL,
    "TorrentId" TEXT NOT NULL,
    "UnpackingFinished" TEXT NULL,
    "UnpackingQueued" TEXT NULL,
    "UnpackingStarted" TEXT NULL, "FileName" TEXT NULL,
    CONSTRAINT "FK_Downloads_Torrents_TorrentId" FOREIGN KEY ("TorrentId") REFERENCES "Torrents" ("TorrentId") ON DELETE RESTRICT
);

CREATE TABLE "TorrentPayloads" (
    "TorrentId" TEXT NOT NULL CONSTRAINT "PK_TorrentPayloads" PRIMARY KEY,
    "Content" TEXT NOT NULL,
    CONSTRAINT "FK_TorrentPayloads_Torrents_TorrentId" FOREIGN KEY ("TorrentId") REFERENCES "Torrents" ("TorrentId")
);

CREATE INDEX "IX_AspNetRoleClaims_RoleId" ON "AspNetRoleClaims" ("RoleId");
CREATE UNIQUE INDEX "RoleNameIndex" ON "AspNetRoles" ("NormalizedName");
CREATE INDEX "IX_AspNetUserClaims_UserId" ON "AspNetUserClaims" ("UserId");
CREATE INDEX "IX_AspNetUserLogins_UserId" ON "AspNetUserLogins" ("UserId");
CREATE INDEX "IX_AspNetUserRoles_RoleId" ON "AspNetUserRoles" ("RoleId");
CREATE INDEX "EmailIndex" ON "AspNetUsers" ("NormalizedEmail");
CREATE UNIQUE INDEX "UserNameIndex" ON "AspNetUsers" ("NormalizedUserName");
CREATE UNIQUE INDEX "IX_Downloads_TorrentId_Path" ON "Downloads" ("TorrentId", "Path");
CREATE INDEX "IX_Torrents_Hash" ON "Torrents" ("Hash");
