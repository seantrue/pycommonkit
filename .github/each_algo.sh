#!/bin/sh
# each_algo.sh CMD...: run CMD once per hash algorithm, md5 then b3, each in a fresh run folder
# (the harness rule, D25 E): COMMONKIT_TEST_RUN holds an empty COMMONKIT_STORE_ROOT and a
# COMMONKIT_MARK_DIR, and COMMONKIT_HASH names the algorithm. Fails when CMD fails, when it
# leaves a bad-key or foreign-digest marker (contract §11), or unless every `<pid>.started`
# marker names the algorithm and there is at least one (plan §3.1: each program says it
# hashed with the algorithm asked for; a child resolving another, by default say, is a leak).
# A test that must resolve something else sends its markers to a scratch folder, so the
# waiver shows in the test. The `.started` root isn't checked: commonkit's own tests build
# stores on tempfile roots outside the run. Used by CI; locally, plum's hf2_env.py does the
# same.
set -e
for algo in md5 b3; do
  run=$(mktemp -d "${TMPDIR:-/tmp}/ck-run.XXXXXX")
  mkdir "$run/store"
  echo "== $algo in $run: $*"
  COMMONKIT_TEST_RUN=$run COMMONKIT_STORE_ROOT=$run/store COMMONKIT_MARK_DIR=$run/marks \
    COMMONKIT_HASH=$algo "$@"
  bad=$(find "$run/marks" -name '*.shm_bad_key' -o -name '*.shm_foreign_digest' 2>/dev/null || true)
  if [ -n "$bad" ]; then
    echo "markers left under $algo:"; for f in $bad; do cat "$f"; done
    exit 1
  fi
  named=0
  wrong=""
  for f in "$run/marks"/*.started; do
    [ -f "$f" ] || continue
    if grep -q "\"algo\": \"$algo\"" "$f"; then named=$((named + 1)); else wrong="$wrong $f"; fi
  done
  if [ -n "$wrong" ]; then
    echo "under $algo, a .started marker names another algorithm:"; for f in $wrong; do cat "$f"; done
    exit 1
  fi
  if [ "$named" -eq 0 ]; then
    echo "no .started marker under $run/marks names $algo: nothing built a store on the harness's terms"
    exit 1
  fi
  echo "== $algo: $named .started marker(s) name $algo"
done
