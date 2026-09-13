#!/bin/sh
cd /tmp/ichiloto-geometry-native || exit 1
stty cols 220 rows 60
stty size > parent-size
stty -g > stty-before
ICHILOTO_ENGINE_TRACE=1 ICHILOTO_GPUI_TRACE=1 php /Users/andrewmasiye/Development/php/games/engines/ichiloto/2.0/console/bin/ichiloto play --directory="$PWD" --no-tmux --renderer=gpui 2> stderr.log
status=$?
stty -g > stty-after
printf '%s\n' "$status" > exit-status
exit "$status"
