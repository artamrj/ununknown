#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
WORK="${TMPDIR:-/tmp}/ununknown-e2e"
PORT="${UNUNKNOWN_E2E_PORT:-17331}"
CONTAINER="ununknown-e2e"
IMAGE="ununknown:e2e"
FFMPEG_IMAGE="jrottenberg/ffmpeg:7.1-alpine"

cleanup() {
  docker rm -f "$CONTAINER" >/dev/null 2>&1 || true
  rm -rf "$WORK"
}
trap cleanup EXIT
cleanup
mkdir -p "$WORK/input/nested" "$WORK/output" "$WORK/cache"

echo "Generating deliberately mistagged MP3 and FLAC fixtures..."
docker run --rm -v "$WORK/input:/data" "$FFMPEG_IMAGE" \
  -f lavfi -i "sine=frequency=440:duration=4" -metadata title="Wrong MP3" \
  -metadata artist="Wrong Artist" -metadata album="Wrong Album" -y /data/nested/wrong.mp3 >/dev/null 2>&1
docker run --rm -v "$WORK/input:/data" "$FFMPEG_IMAGE" \
  -f lavfi -i "sine=frequency=550:duration=4" -metadata title="Wrong FLAC" \
  -metadata artist="Wrong Artist" -metadata album="Wrong Album" -y /data/wrong.flac >/dev/null 2>&1

docker build -t "$IMAGE" "$ROOT" >/dev/null
docker run -d --name "$CONTAINER" -p "$PORT:7331" -e UNUNKNOWN_ACOUSTID_API_KEY=fixture-secret \
  -e 'UNUNKNOWN_MUSICBRAINZ_USER_AGENT=Ununknown/0.2.0 (fixtures@example.com)' \
  -v "$WORK/input:/music/input" -v "$WORK/output:/music/output" \
  -v "$WORK/cache:/cache" "$IMAGE" >/dev/null

for _ in $(seq 1 30); do
  curl -fsS "http://localhost:$PORT/api/health" >/dev/null 2>&1 && break
  sleep 1
done
curl -fsS "http://localhost:$PORT/api/health" >/dev/null

echo "Verifying browser settings persist across restart..."
python3 - "$PORT" <<'PY'
import json, sys, urllib.request
port=sys.argv[1]
settings=json.load(urllib.request.urlopen(f"http://localhost:{port}/api/settings"))
payload=json.dumps(settings).encode()
req=urllib.request.Request(f"http://localhost:{port}/api/settings",data=payload,headers={"Content-Type":"application/json"},method="PUT")
urllib.request.urlopen(req).read()
PY
docker restart "$CONTAINER" >/dev/null
for _ in $(seq 1 30); do
  curl -fsS "http://localhost:$PORT/api/health" >/dev/null 2>&1 && break
  sleep 1
done
curl -fsS "http://localhost:$PORT/api/settings" |
  python3 -c 'import json,sys; s=json.load(sys.stdin); assert s["acoustid_configured"] is True; assert "acoustid_api_key" not in s'
python3 - "$WORK/cache/ununknown.sqlite" <<'PY'
import sqlite3,sys
data=sqlite3.connect(sys.argv[1]).execute("select value from settings").fetchall()
assert "fixture-secret" not in repr(data), data
PY

echo "Scanning real generated audio..."
curl -fsS -X POST "http://localhost:$PORT/api/scan/start" >/dev/null
python3 - "$PORT" <<'PY'
import json, sys, time, urllib.request
port=sys.argv[1]
for _ in range(120):
    jobs=json.load(urllib.request.urlopen(f"http://localhost:{port}/api/jobs"))
    if jobs and jobs[0]["kind"]=="scan" and jobs[0]["status"]!="running":
        assert jobs[0]["status"]=="completed", jobs[0]
        break
    time.sleep(1)
else:
    raise SystemExit("scan timed out")
tracks=json.load(urllib.request.urlopen(f"http://localhost:{port}/api/tracks"))["items"]
assert len(tracks)==2, tracks
assert {t["current_artist"] for t in tracks}=={"Wrong Artist"}, tracks
PY

echo "Seeding deterministic candidates for synthetic audio..."
docker stop "$CONTAINER" >/dev/null
python3 - "$WORK/cache/ununknown.sqlite" "$WORK/candidates.json" <<'PY'
import json, sqlite3, sys
db, output=sys.argv[1:]
con=sqlite3.connect(db)
tracks=con.execute("select id, format from tracks order by id").fetchall()
selected=[]
for index,(track_id,fmt) in enumerate(tracks,1):
    title="Correct MP3" if fmt=="mp3" else "Correct FLAC"
    cur=con.execute("""insert into candidates(
      track_id,provider,title,artist,album,album_artist,track_number,track_total,
      disc_number,disc_total,year,score,raw_json
    ) values(?,?,?,?,?,?,?,?,?,?,?,?,?)""",
    (track_id,"fixture",title,"Fixture Artist","Fixture Album","Fixture Artist",
     index,2,1,1,"2026",100.0,"{}"))
    selected.append([track_id,cur.lastrowid])
con.commit()
with open(output,"w") as f: json.dump(selected,f)
PY
docker start "$CONTAINER" >/dev/null
for _ in $(seq 1 30); do
  curl -fsS "http://localhost:$PORT/api/health" >/dev/null 2>&1 && break
  sleep 1
done
python3 - "$WORK/candidates.json" "$PORT" <<'PY'
import json, sys, urllib.request
path,port=sys.argv[1:]
for track_id,candidate_id in json.load(open(path)):
    payload=json.dumps({"candidate_id":candidate_id}).encode()
    req=urllib.request.Request(
      f"http://localhost:{port}/api/tracks/{track_id}/select-candidate",
      data=payload, headers={"Content-Type":"application/json"}, method="POST")
    urllib.request.urlopen(req).read()
PY

PREVIEW="$(curl -fsS -X POST -H 'Content-Type: application/json' -d '{}' "http://localhost:$PORT/api/apply/preview")"
TOKEN="$(python3 -c 'import json,sys; print(json.load(sys.stdin)["preview_token"])' <<<"$PREVIEW")"
python3 -c 'import json,sys; p=json.load(sys.stdin); assert len(p["items"])==2, p' <<<"$PREVIEW"
curl -fsS -X POST -H 'Content-Type: application/json' \
  -d "{\"preview_token\":\"$TOKEN\"}" "http://localhost:$PORT/api/apply/start" >/dev/null

python3 - "$PORT" <<'PY'
import json, sys, time, urllib.request
port=sys.argv[1]
for _ in range(60):
    jobs=json.load(urllib.request.urlopen(f"http://localhost:{port}/api/jobs"))
    if jobs and jobs[0]["kind"]=="apply" and jobs[0]["status"]!="running":
        assert jobs[0]["status"]=="completed", jobs[0]
        break
    time.sleep(1)
else:
    raise SystemExit("apply timed out")
PY

MP3="$(find "$WORK/output" -type f -name '*Correct MP3.mp3' -print -quit)"
FLAC="$(find "$WORK/output" -type f -name '*Correct FLAC.flac' -print -quit)"
test -n "$MP3"
test -n "$FLAC"
test -f "$MP3"
test -f "$FLAC"
test -f "$WORK/input/nested/wrong.mp3"
test -f "$WORK/input/wrong.flac"

curl -fsS "http://localhost:$PORT/api/tracks" |
  python3 -c 'import json,sys; assert json.load(sys.stdin)["total"]==0, "applied workspace records were not deleted"'

verify() {
  local file="$1" title="$2"
  docker run --rm --entrypoint ffprobe -v "$WORK/output:/data" "$FFMPEG_IMAGE" \
    -v error -show_entries format_tags=title,artist,album -of json "/data/${file#"$WORK/output/"}" |
    python3 -c "import json,sys; t={k.lower():v for k,v in json.load(sys.stdin)['format']['tags'].items()}; assert t['title']=='$title',t; assert t['artist']=='Fixture Artist',t; assert t['album']=='Fixture Album',t"
}
verify "$MP3" "Correct MP3"
verify "$FLAC" "Correct FLAC"

echo "E2E passed: generated files scanned, previewed, copied, and retagged successfully."
