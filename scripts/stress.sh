#!/bin/bash
# Stress test for the ultty terminal (ANSI side).
# Pounds the parser/grid/renderer with throughput, color, cursor, unicode,
# wrap, alt-screen, and scrollback workloads. Prints timings to stderr.
#
# Usage (in a real terminal):
#   bash scripts/stress.sh            # all tests
#   bash scripts/stress.sh 1 3 5      # selected tests by number
#
# Drive from outside (no focus steal) via ulttyctl:
#   ulttyctl new-tab bash scripts/stress.sh
#   ULTTY_STRESS_LOG=/tmp/stress.log → also dumps timings + DONE sentinel
#                                       so a watcher can poll for completion.

set -u
LC_ALL=C.UTF-8

now_ns() { date +%s%N; }

LOG_FILE="${ULTTY_STRESS_LOG:-}"
[ -n "$LOG_FILE" ] && : > "$LOG_FILE"

bench() {
  local name="$1"; shift
  local t1 t2 line
  t1=$(now_ns)
  "$@"
  t2=$(now_ns)
  line=$(printf "  %-38s %6d ms" "$name" $(( (t2 - t1) / 1000000 )))
  printf "%s\n" "$line" >&2
  [ -n "$LOG_FILE" ] && printf "%s\n" "$line" >> "$LOG_FILE"
}

reset() { printf "\033[0m"; }

# ---- tests ----

t1_throughput() {
  # 4 MiB of plain ASCII through the parser
  yes "the quick brown fox jumps over the lazy dog 0123456789 ABCDEF" | head -c $((4*1024*1024))
  reset; echo
}

t2_truecolor_grid() {
  # 64x64 = 4096 truecolor cells, full SGR per cell
  awk 'BEGIN {
    for (r=0; r<256; r+=4) {
      for (g=0; g<256; g+=4) {
        printf "\033[48;2;%d;%d;0m ", r, g
      }
      printf "\033[0m\n"
    }
  }'
  reset
}

t3_cursor_moves() {
  # 50k random absolute cursor positions
  awk 'BEGIN {
    srand()
    for (i=0; i<50000; i++) {
      printf "\033[%d;%dH*", int(rand()*24)+1, int(rand()*80)+1
    }
    printf "\033[0m\n"
  }'
  reset
}

t4_sgr_thrash() {
  # 50k truecolor changes back-to-back
  awk 'BEGIN {
    srand()
    for (i=0; i<50000; i++) {
      printf "\033[38;2;%d;%d;%dmX",
        int(rand()*256), int(rand()*256), int(rand()*256)
    }
    printf "\033[0m\n"
  }'
  reset
}

t5_unicode_wall() {
  # 2000 lines of mixed unicode + emoji + combining marks (~172 KiB)
  awk 'BEGIN {
    line = "日本語テスト 🚀🎨🔥💀 αβγδε é̃ 𝕳𝖊𝖑𝖑𝖔 ｟ ｠ ﷽ "
    for (i=0; i<2000; i++) print line
  }'
  reset
}

t6_long_lines() {
  # 500 very long lines forcing wrap
  awk 'BEGIN {
    for (i=0; i<500; i++) {
      s=""
      for (j=0; j<4000; j++) s = s "x"
      print s
    }
  }'
  reset
}

t7_alt_screen_flicker() {
  # rapid alt-screen toggles + clears
  for i in $(seq 1 200); do
    printf "\033[?1049h\033[2J\033[H[alt %d]" "$i"
    printf "\033[?1049l"
  done
  reset; echo
}

t8_scrollback_spam() {
  # 20k short lines into scrollback
  awk 'BEGIN { for (i=0; i<20000; i++) printf "line %d\n", i }'
}

tests=(
  "throughput-4MiB:t1_throughput"
  "truecolor-grid-4096-cells:t2_truecolor_grid"
  "50k-random-cursor-moves:t3_cursor_moves"
  "50k-truecolor-SGR-changes:t4_sgr_thrash"
  "2000-unicode-emoji-lines:t5_unicode_wall"
  "500-lines-4000-cols:t6_long_lines"
  "200-altscreen-toggles:t7_alt_screen_flicker"
  "20k-scrollback-lines:t8_scrollback_spam"
)

echo "=== ultty stress test ===" >&2
echo "(timings measure wall clock from generator to terminal drain)" >&2
echo >&2

selected=("$@")
for idx in "${!tests[@]}"; do
  num=$((idx+1))
  if [ ${#selected[@]} -gt 0 ]; then
    skip=1
    for s in "${selected[@]}"; do [ "$s" = "$num" ] && skip=0; done
    [ $skip -eq 1 ] && continue
  fi
  entry="${tests[$idx]}"
  name="${entry%%:*}"
  fn="${entry##*:}"
  bench "$num. $name" $fn
done

echo >&2
echo "done." >&2
[ -n "$LOG_FILE" ] && echo "DONE" >> "$LOG_FILE"
