#!/bin/sh
# Demo windows for Docker / Xvfb: terminal + browser so the streamed desktop is not a blank black screen.
# Invoked via phantom-screen-server --post-start-command= (see Dockerfile CMD).

DISPLAY="${DISPLAY:-:99}"
export DISPLAY

# Brief pause so openbox is ready to manage new windows
sleep 0.5

# Firefox: top-left — internal page (no network). Geometry fits 1280x720+ virtual desktops.
firefox-esr --geometry 100x28+32+32 --new-window 'about:logo' 2>/dev/null &

# Terminal: lower half — readable on dark background
xterm -geometry 110x12+32+420 \
  -title 'xterm' \
  -fa 'DejaVu Sans Mono' \
  -fs 10 \
  -bg '#1e1e2e' \
  -fg '#cdd6f4' \
  -e 'printf "\033[1;36mPhantom Screen\033[0m — demo session\n\n"; exec bash' &

exit 0
