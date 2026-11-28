# WebKitGTK 4.1 (the engine Tauri 2 uses on Linux) and Debian's Chromium as a
# same-machine baseline, under Xvfb. See README.md next to this file.
FROM debian:trixie-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      libwebkit2gtk-4.1-0 chromium xvfb xdotool python3 procps \
      fonts-dejavu-core ca-certificates dbus-x11 \
 && rm -rf /var/lib/apt/lists/*
WORKDIR /bench
ENTRYPOINT ["python3", "/bench/harness/run.py"]
