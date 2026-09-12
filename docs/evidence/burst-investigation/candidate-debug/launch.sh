#!/bin/sh
cd /tmp/ichiloto-renderer-burst-candidate-20260912/runtime || exit 1
stty cols 135 rows 36
stty -g > ../evidence/stty-before
ICHILOTO_ENGINE_TRACE=1 ICHILOTO_GPUI_TRACE=1 php /Users/andrewmasiye/Development/php/games/engines/ichiloto/2.0/console/bin/ichiloto play --directory=/tmp/ichiloto-renderer-burst-candidate-20260912/runtime --no-tmux --renderer=gpui
status=$?
stty -g > ../evidence/stty-after
printf '%s\n' "$status" > ../evidence/exit
exit "$status"
