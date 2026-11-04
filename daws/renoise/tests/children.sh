# Sourced by run.sh and check.sh: `run <command>` runs a program that goes when this script goes, so a time limit or a
# Ctrl-C on the script leaves nothing running (a kill -9 can't be caught, so aim one at the program itself). The script
# calls stop_child in its own EXIT trap.
child=
run() {
  "$@" &
  child=$!
  status=0
  wait "$child" || status=$?
  child=
  return "$status"
}
# What a program started goes first (lua-language-server checks in a worker of its own; check.sh is run.sh's child).
stop_tree() {
  for kid in $(pgrep -P "$1" 2>/dev/null); do
    stop_tree "$kid"
  done
  kill "$1" 2>/dev/null || true
}
stop_child() {
  if [ -n "$child" ]; then
    stop_tree "$child"
    wait "$child" 2>/dev/null || true
    child=
  fi
}
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 131' QUIT
trap 'exit 142' ALRM
trap 'exit 143' TERM
