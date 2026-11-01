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
stop_child() {
  if [ -n "$child" ]; then
    kill "$child" 2>/dev/null || true
    wait "$child" 2>/dev/null || true
    child=
  fi
}
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 142' ALRM
trap 'exit 143' TERM
