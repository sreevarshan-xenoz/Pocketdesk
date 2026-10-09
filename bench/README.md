# Encoder benchmark baselines

One pinned clip, one geometry, one bitrate, every run: `linux-link bench` measures the
encode half of the pipeline on the machine it is running on, and compares that measurement
to a record committed here.

The clip is generated in code (`core/src/streaming/bench.rs::scene_frame`), not stored —
a committed 300-frame 720p BGRA sequence would be ~1.1 GB of binary, and a generated clip is
byte-identical between runs. It is a static diagonal gradient with a moving block, because a
frame that never changes lets the encoder coast on zero-residual skip blocks and measures
nothing.

## What a record is

A `BenchRecord` JSON object: the workload that ran (backend, geometry, fps, bitrate, preset,
codec, frame count, host CPU + kernel) and the per-frame encode-time distribution over the
sampled frames (`count`, `mean`, `p50`, `p90`, `p95`, `p99`, `max`, all whole milliseconds).
The `backend` is read back from the encoder *after* any internal fallback, so a run that
asked for VAAPI and silently degraded to software is recorded as software.

Encode time only. Decode and render happen on the phone and are not in this file; a benchmark
that claimed to cover them would be lying about the half that matters most on a bad session.

### What it cannot measure yet: sidecar rungs

The clock wraps `encode_frame`. For the in-process encoders (`software` on Linux, `vaapi`) that
call *is* the encode. For every sidecar rung — NVENC, AMF, Quick Sync and Media Foundation, and
the software rung on Windows, all `ffmpeg-sidecar*` backends — it only writes the frame into the
FFmpeg child's stdin and collects whatever output has already arrived, so the recorded "encode
time" is a pipe write: on an RTX 3060 / Radeon box every one of them reports p50 0 ms. Such a
record cannot regress and so is not committed as a baseline. Measuring a sidecar honestly needs
the packets matched back to the frame that produced them (submit-to-first-slice latency), which
the per-NAL packetizer does not track yet.

## Reading a run

```
linux-link bench --target vaapi --repeat 3 --baseline bench/baselines --tolerance-pct 50
```

| exit | meaning                                                        |
|------|----------------------------------------------------------------|
| 0    | every percentile within the tolerance of the baseline           |
| 1    | a percentile moved beyond it — the regression this exists to catch |
| 3    | no baseline here matches this host + backend: nothing concluded |
| 4    | could not measure (no encoder, unwritable output)               |

Exit 3 is deliberately not exit 1. Encode throughput does not travel between CPUs, so grading
an unknown machine against a foreign baseline would either cry wolf or hide the real signal;
`SKIP` with the host it found is the honest answer, and CI treats 3 as a skip.

`--repeat N` keeps the fastest of N runs, because other work on the machine can only make an
encode slower. Unloaded, run-to-run spread on this workload is ~1 ms; a desktop with a load
average near 6 pushed a single run's p50 from 7 ms to 11-18 ms, and the best of seven runs
reproduced the 7 ms figure. That is why a baseline is never recorded from a single run.

## Adding a baseline for a machine

Run the benchmark there and let it name its own file:

```
linux-link bench --target software --record bench/baselines
linux-link bench --target vaapi   --record bench/baselines   # on a box with a VA node
```

The filename is the backend plus the host identity, so a machine accumulates one record per
rung and the next run on it finds its own without a human remembering a name. Commit the file;
the CI job's `encoder-baseline-software` artifact is the same record for the runner class, which
is how a hosted-runner baseline gets added without anyone owning a GitHub runner.

A record from a different schema version, geometry, bitrate, preset, codec, backend or host is
`not comparable` and is reported as such. There is no fuzzy matching, and there will not be:
the value of the check is that a red build means something.
