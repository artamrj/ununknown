# Docker Compose And Deployment

Ununknown 0.4 preserves saved settings during upgrade but clears temporary matched previews and provider cache when migrations require it.

## Requirements

- Docker Engine or Docker Desktop
- Docker Compose v2
- A machine or NAS that can mount the music folders

## Install

Create a directory for Ununknown:

```bash
mkdir -p ununknown/music/input ununknown/music/output ununknown/cache
cd ununknown
curl -O https://raw.githubusercontent.com/artamrj/ununknown/main/docker-compose.yml
docker compose up -d
```

The standard Compose file pulls `ghcr.io/artamrj/ununknown:latest`. Open <http://localhost:7331>.

`latest` follows the newest successful `main` build. To pin a deployment to this release, change the image to the plain project version tag:

```yaml
services:
  ununknown:
    image: ghcr.io/artamrj/ununknown:0.4.5
```

Ununknown uses one version source: `Cargo.toml`. The frontend `package.json` version must match it, and Docker tags use plain semver such as `0.4.5`, not `v0.4.5`.

Ununknown has no TOML configuration file. Normal configuration is done in the web UI and saved in `/cache/ununknown.sqlite`.

Provider configuration is not saved in SQLite. Supply both providers through the environment:

```bash
export UNUNKNOWN_ACOUSTID_API_KEY="your-application-key"
export UNUNKNOWN_MUSICBRAINZ_USER_AGENT="Ununknown/0.4.5 (you@example.com)"
docker compose up -d
```

Or place them in a `.env` file beside `docker-compose.yml`:

```dotenv
UNUNKNOWN_ACOUSTID_API_KEY=your-application-key
UNUNKNOWN_MUSICBRAINZ_USER_AGENT=Ununknown/0.4.5 (you@example.com)
```

AcoustID is optional. MusicBrainz requires a meaningful email address or website but no API key.

## Volumes

The default Compose file mounts:

```text
./music/input  -> /music/input
./music/output -> /music/output
./cache        -> /cache
```

For existing NAS folders, edit only the host side:

```yaml
volumes:
  - /volume1/music/to-fix:/music/input
  - /volume1/music/fixed:/music/output
  - /volume1/docker/ununknown:/cache
```

The container paths must stay `/music/input`, `/music/output`, and `/cache`. Ensure Docker can read input and write output/cache.

## Operations

Start and stop:

```bash
docker compose up -d
docker compose down
```

Update:

```bash
docker compose pull
docker compose up -d
```

Logs and health:

```bash
docker compose logs -f
curl http://localhost:7331/api/health
```

The health response should be `{"status":"ok"}`.

## Local Source Build

Developers can build the checked-out source using:

```bash
docker compose -f docker-compose.dev.yml up --build
```

The Docker build installs the Debian package providing `fpcalc` and runs `fpcalc -version`, so the build fails if Chromaprint is unavailable.

## Troubleshooting

- **Permission denied:** verify the Docker user can read input and write output/cache.
- **Settings disappear:** ensure `/cache` is mounted persistently.
- **Port already used:** change `7331:7331` to another host port such as `17331:7331`.
- **Image pull denied/not found:** confirm the GHCR package is public, then run `docker compose pull`.
- **No files found:** verify files are visible inside the container with `docker compose exec ununknown find /music/input -type f`.

Always test using copied audio before enabling in-place writes.
