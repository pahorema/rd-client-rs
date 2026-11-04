# rd-client

A self-hosted web client that manages torrents on debrid services (Real-Debrid,
AllDebrid, Premiumize, TorBox, DebridLink and HIGHWAY) and downloads the finished
files to your machine. It exposes a qBittorrent-compatible API so Sonarr, Radarr
and similar tools can use it as a regular torrent client.

[![Rust](https://github.com/pahorema/rd-client-rs/actions/workflows/rust.yml/badge.svg)](https://github.com/pahorema/rd-client-rs/actions/workflows/rust.yml)
[![Web](https://github.com/pahorema/rd-client-rs/actions/workflows/web.yml/badge.svg)](https://github.com/pahorema/rd-client-rs/actions/workflows/web.yml)
[![Docker](https://github.com/pahorema/rd-client-rs/actions/workflows/docker.yml/badge.svg)](https://github.com/pahorema/rd-client-rs/actions/workflows/docker.yml)

## Features

- Add torrents from magnet links or `.torrent` files, from the web UI, a watch
  folder or the qBittorrent API.
- Choose which files to download: everything, only files already available on
  the provider, or a manual selection; filter by size and include/exclude
  regular expressions.
- Download finished files with the internal multi-connection downloader, an
  [aria2](https://aria2.github.io/) instance, Synology Download Station, or as
  symlinks into a mounted provider drive (rclone and similar).
- Unpack RAR (including multi-volume sets) and ZIP archives after download.
- Retry failed torrents and downloads automatically, remove torrents from the
  provider and/or the client when they finish, run a program on completion.
- Pause downloads when free disk space runs low and resume them afterwards.
- Live updates in the web UI; light and dark themes.
- Single static binary with the web UI embedded; small multi-arch Docker image.

## Running with Docker

```sh
docker run -d --name rd-client \
  -p 6500:6500 \
  -e PUID=1000 -e PGID=1000 -e TZ=Europe/Rome \
  -v /path/to/config:/data/db \
  -v /path/to/downloads:/data/downloads \
  ghcr.io/pahorema/rd-client-rs:latest
```

Or with Compose:

```yaml
services:
  rd-client:
    image: ghcr.io/pahorema/rd-client-rs:latest
    container_name: rd-client
    restart: unless-stopped
    ports:
      - 6500:6500
    environment:
      PUID: 1000
      PGID: 1000
      TZ: Europe/Rome
    volumes:
      - /path/to/config:/data/db
      - /path/to/downloads:/data/downloads
```

Open `http://<host>:6500`, create the account, then enter the API key of your
debrid provider. Everything else is configured on the Settings page.

Images are published for `linux/amd64` and `linux/arm64`. Tags: `latest` and
`X.Y.Z` / `X.Y` for releases, `edge` for the main branch.

### Container environment

| Variable | Default | Meaning |
|---|---|---|
| `PUID`, `PGID` | `1000` | User and group the server runs as. |
| `UMASK` | unset | File creation mask, for example `002`. |
| `TZ` | `UTC` | Time zone, for example `Europe/Rome`. |
| `SKIP_CHOWN` | `false` | Set to `true` to skip the ownership fix of `/data` at startup (useful for large download folders). |
| `FILE__<NAME>` | – | Set `<NAME>` to the contents of a file, for Docker secrets. |

## Configuration

Process options are read from the command line or the environment; everything
else lives in the database and is edited in the web UI.

| Option | Variable | Default | Meaning |
|---|---|---|---|
| `--port` | `PORT` | `6500` | HTTP port, on every interface. |
| `--base-path` | `BASE_PATH` | – | Also serve the app under a URL prefix, for example `/rd-client`, behind a reverse proxy. |
| `--database` | `DATABASE_PATH` | `/data/db/rd-client.db` | SQLite database file. |
| `--log-file` | `LOG_PATH` | `rd-client.log` beside the database | Log file. |
| `--log-max-size` | `LOG_MAX_SIZE` | `5242880` | Log size in bytes before rotation. |
| `--log-max-files` | `LOG_MAX_FILES` | `5` | Log files kept, including the active one. |

`rd-client healthcheck` checks the running server; the Docker image uses it as
its health check.

## Sonarr, Radarr and other tools

Add a **qBittorrent** download client:

- Host: the rd-client host, port `6500` (plus the base path, if you set one, as
  the URL base).
- Username and password: the rd-client account (any value when authentication
  is disabled).
- Category: for example `sonarr` or `radarr`; files are downloaded to
  `<Download path>/<category>/<torrent name>`.

In Settings → Download Client, **Download path** is where rd-client writes the
files (inside the container, `/data/downloads`), and **Mapped path** is the same
folder as seen by Sonarr/Radarr. When the two tools run on different hosts or
containers, set the mapped path to the folder they see, or add a remote path
mapping in Sonarr/Radarr.

## Building from source

Requirements: Rust (the version in `rust-toolchain.toml`), a C++ compiler (for
the bundled RAR library) and [Bun](https://bun.sh/).

```sh
cd web && bun install --frozen-lockfile && bun run build && cd ..
cargo build --release -p rdc-server
./target/release/rd-client --database ./data/rd-client.db
```

The web UI in `web/build` is embedded into the binary at compile time. During UI
development, `bun run dev` in `web/` serves the UI with hot reload and proxies
the API to a server on port 6500.

Release binaries for Linux (static), macOS and Windows are attached to each
[GitHub release](https://github.com/pahorema/rd-client-rs/releases).

## Credits

The feature set and behavior follow [rdt-client](https://github.com/rogerfar/rdt-client)
by Roger Far. rd-client reads and upgrades its database in place: to reuse an
existing database, point `DATABASE_PATH` at it, for example
`DATABASE_PATH=/data/db/rdtclient.db`.

## License

[MIT](LICENSE)
