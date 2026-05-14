# Ultty Live Stress Repeat Report

| Field | Value |
|-------|-------|
| Date | 2026-05-14T17:41:33-04:00 |
| Commit | `d3ae46ef816b95f42a2c425d50257b6ec7ff114a` |
| Runs | 3 |
| Tests | all |
| Driver | `target/debug/ulttyctl new-tab env ULTTY_STRESS_LOG=<log> bash scripts/stress.sh` |

## Summary

| Workload | Runs | Mean | Stddev | Min | Max | Spread |
|----------|------|------|--------|-----|-----|--------|
| `1. throughput-4MiB` | 3 | 271.3 ms | 7.2 ms | 263 ms | 276 ms | 13 ms |
| `2. truecolor-grid-4096-cells` | 3 | 25.0 ms | 9.5 ms | 14 ms | 31 ms | 17 ms |
| `3. 50k-random-cursor-moves` | 3 | 188.7 ms | 16.0 ms | 173 ms | 205 ms | 32 ms |
| `4. 50k-truecolor-SGR-changes` | 3 | 285.0 ms | 52.0 ms | 228 ms | 330 ms | 102 ms |
| `5. 2000-unicode-emoji-lines` | 3 | 19.7 ms | 0.6 ms | 19 ms | 20 ms | 1 ms |
| `6. 500-lines-4000-cols` | 3 | 214.0 ms | 18.2 ms | 203 ms | 235 ms | 32 ms |
| `7. 200-altscreen-toggles` | 3 | 6.0 ms | 0.0 ms | 6 ms | 6 ms | 0 ms |
| `8. 20k-scrollback-lines` | 3 | 47.0 ms | 2.0 ms | 45 ms | 49 ms | 4 ms |

## Raw Runs

### Run 1

```text
  1. throughput-4MiB                        276 ms
  2. truecolor-grid-4096-cells               31 ms
  3. 50k-random-cursor-moves                173 ms
  4. 50k-truecolor-SGR-changes              330 ms
  5. 2000-unicode-emoji-lines                20 ms
  6. 500-lines-4000-cols                    204 ms
  7. 200-altscreen-toggles                    6 ms
  8. 20k-scrollback-lines                    49 ms
```

### Run 2

```text
  1. throughput-4MiB                        263 ms
  2. truecolor-grid-4096-cells               30 ms
  3. 50k-random-cursor-moves                188 ms
  4. 50k-truecolor-SGR-changes              297 ms
  5. 2000-unicode-emoji-lines                19 ms
  6. 500-lines-4000-cols                    235 ms
  7. 200-altscreen-toggles                    6 ms
  8. 20k-scrollback-lines                    47 ms
```

### Run 3

```text
  1. throughput-4MiB                        275 ms
  2. truecolor-grid-4096-cells               14 ms
  3. 50k-random-cursor-moves                205 ms
  4. 50k-truecolor-SGR-changes              228 ms
  5. 2000-unicode-emoji-lines                20 ms
  6. 500-lines-4000-cols                    203 ms
  7. 200-altscreen-toggles                    6 ms
  8. 20k-scrollback-lines                    45 ms
```

