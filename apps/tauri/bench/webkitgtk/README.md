# WebKitGTK bench

Runs the same bench pages as `bench/run.mjs` (stream, transcript, canvas) in
WebKitGTK 4.1, the engine Tauri 2 uses on Linux, and in Chromium as a
baseline on the same machine. Results and caveats from the first run are in
`plans/research/tauri-spike.md` (§2.7).

The pages are loaded in WebKitGTK's `MiniBrowser`, not in the Tauri shell:
it is the same library (`libwebkit2gtk-4.1`) that wry links, so engine
behaviour (JS, layout, rAF scheduling, iframe throttling) is the same, but
Tauri's own WebView settings (hardware acceleration policy, user agent) are
not applied. Running `antre-shell --bench` natively on Linux is the more
faithful check; see "On a Linux machine" below.

The harness (`harness/run.py`) serves `apps/tauri/dist`, injects a small
script that POSTs `window.__benchResult` back, samples each engine's process
RSS from `ps`, and writes one JSON per engine/page/run. `--click-on STATUS`
sends a real X11 click (xdotool) into the page when its status line first
shows STATUS, to check whether a user gesture lifts iframe throttling.
`--tag NAME` suffixes the result files (for runs with different flags or
environment, e.g. `-e WEBKIT_DISABLE_COMPOSITING_MODE=1`); `summarize.mjs`
shows each tag as its own column.

## In a container (macOS or Linux host)

```bash
cd apps/tauri
node bench/build.mjs                       # bundle the pages into dist/
docker build -t antre-webkitgtk-bench bench/webkitgtk
OUT="$PWD/bench/results/$(date +%F)-webkitgtk"; mkdir -p "$OUT"
docker run --rm --shm-size=1g \
  -v "$PWD/dist:/bench/dist:ro" \
  -v "$PWD/bench/webkitgtk/harness:/bench/harness:ro" \
  -v "$OUT:/bench/out" \
  antre-webkitgtk-bench --runs 3
docker run --rm --shm-size=1g \
  -v "$PWD/dist:/bench/dist:ro" \
  -v "$PWD/bench/webkitgtk/harness:/bench/harness:ro" \
  -v "$OUT:/bench/out" \
  antre-webkitgtk-bench --pages canvas --runs 3 --click-on "first mount"
node bench/summarize.mjs "$OUT" --labels "webkitgtk=WebKitGTK 2.54,chromium=Chromium 156,webkitgtk+click=WebKitGTK + click,chromium+click=Chromium + click"
docker image rm antre-webkitgtk-bench      # ~1.5 GB
```

In a container everything renders in software under Xvfb (no GPU, device
pixel ratio 1, a 60 Hz virtual display): compare the two engines with each
other, not with the macOS numbers.

## On a Linux machine

This is what the container cannot tell: GPU compositing, the real display's
refresh rate, Wayland vs X11, and the distro's own WebKitGTK build.

1. Install the engine and tools (Debian/Ubuntu names; Fedora: `webkit2gtk4.1`,
   `chromium`, `xdotool`):

   ```bash
   sudo apt install libwebkit2gtk-4.1-0 chromium xdotool python3
   ```

2. Run the harness against your session's display instead of Xvfb. On
   Wayland, run it inside an X11 session or accept that xdotool cannot size
   the window or click (the viewport is reported in each result):

   ```bash
   cd apps/tauri
   OUT="$PWD/bench/results/$(date +%F)-linux-native"
   BENCH_DIST="$PWD/dist" BENCH_OUT="$OUT" \
     python3 bench/webkitgtk/harness/run.py --use-display --runs 3
   BENCH_DIST="$PWD/dist" BENCH_OUT="$OUT" \
     python3 bench/webkitgtk/harness/run.py --use-display --pages canvas --runs 3 --click-on "first mount"
   node bench/summarize.mjs "$OUT" --labels "webkitgtk=WebKitGTK,chromium=Chromium"
   ```

   Keep the window focused and on screen, as on macOS: an occluded WebView
   throttles timers and rAF.

3. The faithful version, through the Tauri shell itself:

   ```bash
   sudo apt install libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev build-essential
   cd apps/tauri && cargo build
   ./target/debug/antre-shell --bench stream --out /tmp/stream.json
   ```

   The JSON is what the harness stores under `result`. Memory is not in
   it: `bench/run.mjs` samples RSS by macOS process names
   (`com.apple.WebKit.*`), so on Linux watch `WebKitWebProcess` with `ps`, or
   teach `runTauri` the Linux names first.

Also worth a run on Linux, since they are common Tauri-on-Linux workarounds:
`WEBKIT_DISABLE_DMABUF_RENDERER=1` and `WEBKIT_DISABLE_COMPOSITING_MODE=1`
(set them in the environment of the harness or of `antre-shell`).
