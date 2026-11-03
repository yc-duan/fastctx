#!/bin/bash
# Walks Fastctx on a Mac as the reports of its window describe: it opens,
# is clicked, put away for a minute and brought back from the Dock, closed
# with its red button and brought back, dragged by the sidebar's head, and
# started a second time. A screenshot at each step; each check says PASS or
# FAIL in walk.log and the walk goes on.
set -u
W=./walk
app=/Applications/Fastctx.app
mkdir -p shots
n=0
shot() { n=$((n + 1)); screencapture -x "shots/$(printf %02d "$n")-$1.png" || true; }
say() { echo "$*" | tee -a walk.log; }
check() { if eval "$2"; then say "PASS $1"; else say "FAIL $1"; fi; }
apps() { pgrep -x fastctx-app | wc -l | tr -d ' '; }
win() { $W window | head -1; }

say "$($W trusted)"
open "$app"
sleep 30
shot opened
read -r X Y WD HT < <(win)
say "window: ${X:-none} ${Y:-} ${WD:-} ${HT:-}"
check "a window opens" '[ -n "${X:-}" ]'
check "one app process" '[ "$(apps)" = 1 ]'

# A first opening asks for a language: English, a little below the middle.
$W click $((X + WD / 2)) $((Y + HT / 2 + 26))
sleep 5
shot language-chosen

# The sidebar's rows, as the interface lays them out: Agents, Activity,
# Machines, each 36 points apart from 52 down, their middle at 146 across.
$W click $((X + 146)) $((Y + 141))
sleep 3
shot click-machines
$W wobble $((X + 146)) $((Y + 105))
sleep 3
shot wobble-activity
read -r X2 Y2 _ _ < <(win)
check "a click that moves a little does not move the window" '[ "$X2 $Y2" = "$X $Y" ]'

$W key m
sleep 3
check "Command-M puts the window away" '! $W window >/dev/null'
shot minimized
sleep 60
open "$app"
sleep 5
shot back-from-dock
read -r X Y WD HT < <(win)
check "the Dock brings it back after a minute" '[ -n "${X:-}" ]'
$W click $((X + 146)) $((Y + 69))
sleep 3
shot click-agents-after-minimize

read -r X Y WD HT < <(win)
screencapture -x -R"$X,$Y,60,60" corner.png
red=$($W red corner.png "$X" "$Y" || true)
say "red button: ${red:-not found}"
if [ -n "$red" ]; then
  # shellcheck disable=SC2086
  $W click $red
else
  $W key w
fi
sleep 3
check "closing puts the window away" '! $W window >/dev/null'
check "closing leaves the app running" '[ "$(apps)" -ge 1 ]'
shot closed
open "$app"
sleep 5
shot back-after-close
check "the Dock brings it back after closing" '$W window >/dev/null'

read -r X Y WD HT < <(win)
$W drag $((X + 215)) $((Y + 30)) $((X + 415)) $((Y + 130))
sleep 2
read -r X2 Y2 _ _ < <(win)
say "dragged from $X $Y to ${X2:-} ${Y2:-}"
check "the sidebar's head drags the window" '[ -n "${X2:-}" ] && [ "$X2" != "$X" ]'
shot dragged

"$app/Contents/MacOS/fastctx-app" >second.log 2>&1 &
sleep 8
check "a second start leaves one app" '[ "$(apps)" = 1 ]'
check "and one window" '[ "$($W window | wc -l | tr -d " ")" = 1 ]'
shot second-start

$W key m
sleep 3
"$app/Contents/MacOS/fastctx-app" >>second.log 2>&1 &
sleep 8
check "a second start brings a put-away window back" '$W window >/dev/null'
check "still one app" '[ "$(apps)" = 1 ]'
shot second-start-shows

$W key q
sleep 5
check "Command-Q ends it" '[ "$(apps)" = 0 ]'
cat second.log
