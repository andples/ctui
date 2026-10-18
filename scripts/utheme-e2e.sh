#!/usr/bin/env bash
# End-to-end check of following utheme live (utheme's CLIENT-PROTOCOL.md,
# section 8), without utheme itself: this script writes generation folders
# and swaps the `current` link the way utheme does, runs the app in a private
# tmux server and reads the colours it paints.
#
#   scripts/utheme-e2e.sh <app> <config-file> <quit-key> -- <command...>
#
# <config-file> must already say theme = "utheme"; the script checks it is
# byte-identical afterwards. On macOS the app must follow every switch; on
# any other OS it must ignore utheme completely (no heartbeat, no recolour).
# E2E_EXPECT=follow|ignore overrides the choice by OS. Needs tmux and perl.
# Exits non-zero if any check fails.
set -u

app=$1 config=$2 quit=$3
shift 4
cmd=$(printf '%q ' "$@")

work=$(mktemp -d)
export UTHEME_HOME=$work/uth
data=$UTHEME_HOME/.local/share/utheme
clients=$UTHEME_HOME/.local/state/utheme/clients
sock=utheme-e2e-$$
fails=0

ok() { echo "ok   $*"; }
bad() { echo "FAIL $*"; fails=$((fails + 1)); }
now_ms() { perl -MTime::HiRes=time -e 'printf "%d\n", time * 1000'; }
t() { tmux -L "$sock" "$@"; }
cleanup() { t kill-server 2>/dev/null; rm -rf "$work"; }
trap cleanup EXIT

# gen <id> <background rrggbb>: a generation folder like utheme writes.
gen() {
    local d=$data/generations/$1
    mkdir -p "$d"
    printf '{"protocol": 1, "id": "%s", "name": "Test %s", "mode": "dark", "generation": "%s", "directory": "/x"}\n' "$1" "$1" "$1" >"$d/theme.json"
    printf '{"background": "#%s", "foreground": "#D8DEE9", "accent": "#88c0d0", "muted": "#4c566a", "red": "#bf616a", "orange": "#d08770", "yellow": "#ebcb8b", "green": "#a3be8c", "cyan": "#8fbcbb", "blue": "#81a1c1", "magenta": "#b48ead", "mode": "dark"}\n' "$2" >"$d/palette.json"
    echo "$1" >"$d/generation"
}
# switch <id>: point `current` at it with one atomic rename, as utheme does.
switch() {
    ln -s "$data/generations/$1" "$data/.current-tmp"
    perl -e 'rename($ARGV[0], $ARGV[1]) or die "rename: $!"' "$data/.current-tmp" "$data/current"
}
# The SGR background code for rrggbb, as tmux reports it.
sgr_bg() { printf '48;2;%d;%d;%d' "0x${1:0:2}" "0x${1:2:2}" "0x${1:4:2}"; }
shows() { t capture-pane -p -e -t e2e 2>/dev/null | grep -q "$(sgr_bg "$1")"; }
# wait_for <ms> <test...>: ms until the test passed, or nothing on timeout.
wait_for() {
    local limit=$1 start
    shift
    start=$(now_ms)
    while [ $(($(now_ms) - start)) -lt "$limit" ]; do
        if "$@"; then
            echo $(($(now_ms) - start))
            return 0
        fi
        sleep 0.05
    done
    return 1
}
beat() { ls "$clients"/"$app"-*.json 2>/dev/null | head -n 1; }
beat_says() { local f; f=$(beat); [ -n "$f" ] && grep -q "\"$1\"" "$f"; }
no_beat() { [ -z "$(beat)" ]; }
gone() { ! t has-session -t e2e 2>/dev/null; }
start() { t new -d -s e2e -x 100 -y 30 "UTHEME_HOME=$(printf '%q' "$UTHEME_HOME") $cmd"; }

mkdir -p "$data"
gen g1 102030
gen g2 203040
for i in 0 1 2 3 4 5 6 7 8 9; do gen "r$i" "4${i}4${i}4${i}"; done
switch g1
cp "$config" "$work/config.before"

expect=${E2E_EXPECT:-$([ "$(uname)" = Darwin ] && echo follow || echo ignore)}
if [ "$expect" = ignore ]; then
    echo "# $(uname): $app must ignore utheme"
    start
    sleep 2
    shows 102030 && bad "showed utheme's colours" || ok "doesn't show utheme's colours"
    rm "$data/current" && switch g2 && sleep 1.5
    shows 203040 && bad "recoloured on a switch" || ok "doesn't recolour on a switch"
    [ -e "$UTHEME_HOME/.local/state" ] && bad "wrote under \$UTHEME_HOME/.local/state" || ok "wrote no heartbeat"
    t send-keys -t e2e "$quit"
    wait_for 3000 gone >/dev/null && ok "quit" || bad "didn't quit"
    cmp -s "$config" "$work/config.before" && ok "config unchanged" || bad "config changed"
    echo "# $fails failure(s)"
    exit $((fails > 0))
fi

echo "# $(uname): $app must follow utheme"
start
ms=$(wait_for 3000 shows 102030) && ok "starts in utheme's theme (${ms} ms)" || bad "didn't start in utheme's theme"
ms=$(wait_for 2000 beat_says g1) && ok "heartbeat: $(cat "$(beat)")" || bad "no heartbeat for g1"

switch g2
ms=$(wait_for 3000 shows 203040) || ms=
if [ -n "$ms" ] && [ "$ms" -le 1200 ]; then ok "recoloured ${ms} ms after a switch"; else bad "recolour took ${ms:-over 3000} ms"; fi
wait_for 1000 beat_says g2 >/dev/null && ok "heartbeat says g2" || bad "heartbeat doesn't say g2"

for i in 0 1 2 3 4 5 6 7 8 9; do
    switch "r$i"
    sleep 0.02
done
ms=$(wait_for 3000 shows 494949) && ok "10 quick switches end on the last (${ms} ms)" || bad "didn't end on the last of 10 switches"
sleep 1.2
shows 494949 && beat_says r9 && ok "still on r9 a second later" || bad "moved off r9"

rm "$data/current"
wait_for 3000 no_beat >/dev/null && ok "uninstall: heartbeat removed" || bad "heartbeat left after uninstall"
shows 494949 && bad "still showing utheme after uninstall" || ok "uninstall: fell back"
switch g1
wait_for 3000 shows 102030 >/dev/null && ok "set up again: follows again" || bad "didn't follow after setup"

t send-keys -t e2e "$quit"
wait_for 3000 gone >/dev/null && ok "quit" || bad "didn't quit"
no_beat && ok "quit: heartbeat removed" || bad "heartbeat left after quit"

start
wait_for 3000 beat_says g1 >/dev/null
pid=$(basename "$(beat)" .json)
pid=${pid##*-}
kill -9 "$pid" 2>/dev/null && wait_for 2000 gone >/dev/null
if ! kill -0 "$pid" 2>/dev/null && [ -n "$(beat)" ]; then
    ok "kill -9: heartbeat of dead pid $pid left for utheme to clear"
else
    bad "kill -9 didn't behave as expected"
fi

cmp -s "$config" "$work/config.before" && ok "config byte-identical" || bad "config changed"
others=$(find "$UTHEME_HOME" \( -type f -o -type l \) -not -path "$clients/*" |
    grep -vE '/generations/[^/]+/(theme\.json|palette\.json|generation)$|/current$')
[ -z "$others" ] && ok "wrote nothing but its heartbeat" || bad "wrote other files: $others"
echo "# $fails failure(s)"
exit $((fails > 0))
