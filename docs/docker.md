# Docker

Run Ununknown without installing Rust, Node, or `fpcalc` on your host.

The image includes Chromaprint, so `fpcalc` is available inside the container. You do not need `brew install chromaprint` on the host when using Docker.

## Local dev/test

Build from the current checkout:

```bash
mkdir -p .local/cache .local/input .local/output
docker compose -f docker-compose.local.yml up --build
```

Open `http://localhost:7331`.

Put music files in `.local/input`. Corrected files are written to `.local/output` when copy mode is used. SQLite data and provider cache are stored in `.local/cache`.

Optional environment variables:

```bash
export UNUNKNOWN_ACOUSTID_API_KEY=your_key_here
export UNUNKNOWN_MUSICBRAINZ_USER_AGENT='Ununknown/0.4.5 (https://github.com/artamrj/ununknown)'
docker compose -f docker-compose.local.yml up --build
```

Useful local checks:

```bash
docker compose -f docker-compose.local.yml build
docker compose -f docker-compose.local.yml exec ununknown which fpcalc
docker compose -f docker-compose.local.yml logs -f ununknown
```

`docker-compose.yml` is kept as a convenience alias for local use. Prefer `docker-compose.local.yml` in scripts and docs so local and deploy commands stay explicit.

## Deploy on server, NAS, or VPS

Use the GHCR image. This Compose file does not build locally.

```bash
mkdir -p data/cache data/input data/output
docker compose -f docker-compose.deploy.yml pull
docker compose -f docker-compose.deploy.yml up -d
```

Open `http://SERVER_IP:7331`.

By default, deploy mode stores data beside the Compose file:

- `./data/cache` -> `/cache`
- `./data/input` -> `/music/input`
- `./data/output` -> `/music/output`

Override host paths for NAS or VPS layouts:

```bash
export UNUNKNOWN_CACHE_DIR=/volume1/docker/ununknown/cache
export UNUNKNOWN_INPUT_DIR_HOST=/volume1/music/input
export UNUNKNOWN_OUTPUT_DIR_HOST=/volume1/music/output
export UNUNKNOWN_ACOUSTID_API_KEY=your_key_here
export UNUNKNOWN_MUSICBRAINZ_USER_AGENT='Ununknown/0.4.5 (https://github.com/artamrj/ununknown)'
docker compose -f docker-compose.deploy.yml up -d
```

Deploy mode expects this image to already exist:

```text
ghcr.io/artamrj/ununknown:latest
```

This repository setup does not add GitHub Actions publishing automation.
