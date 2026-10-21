"""The soundtrack: F major, 112 BPM, music and effects written as one piece.

Effects are pitched in the key and go through the same reverb as the music,
mixed under it. Writes soundtrack.wav (44.1 kHz stereo, 16-bit).
"""
import numpy as np
import wave

SR = 44100
DUR = 31.6
N = int(SR * DUR)
BPM = 112
BEAT = 60 / BPM
BAR = 4 * BEAT
rng = np.random.default_rng(7)


def hz(midi):
    return 440.0 * 2 ** ((midi - 69) / 12)


def env_adsr(n, a, d, s, r, sr=SR):
    a, d, r = int(a * sr), int(d * sr), int(r * sr)
    sus = max(n - a - d - r, 0)
    e = np.concatenate([
        np.linspace(0, 1, max(a, 1)),
        np.linspace(1, s, max(d, 1)),
        np.full(sus, s),
        np.linspace(s, 0, max(r, 1)),
    ])
    return e[:n] if len(e) >= n else np.pad(e, (0, n - len(e)))


def add(buf, start, sig, gain=1.0, pan=0.0):
    i = int(start * SR)
    if i >= N:
        return
    sig = sig[: N - i]
    l, r = np.cos((pan + 1) * np.pi / 4), np.sin((pan + 1) * np.pi / 4)
    buf[0, i:i + len(sig)] += sig * gain * l
    buf[1, i:i + len(sig)] += sig * gain * r


def onepole(x, cutoff):
    """A one-pole lowpass; `cutoff` in Hz, scalar or per-sample."""
    cutoff = np.broadcast_to(np.asarray(cutoff, dtype=float), x.shape)
    a = np.exp(-2 * np.pi * cutoff / SR)
    y = np.empty_like(x)
    acc = 0.0
    for i in range(len(x)):
        acc = (1 - a[i]) * x[i] + a[i] * acc
        y[i] = acc
    return y


def pad_voice(midi, dur):
    n = int(dur * SR)
    t = np.arange(n) / SR
    f = hz(midi)
    sig = np.zeros(n)
    for det in (-0.12, 0.0, 0.11):
        ff = f * 2 ** (det / 12)
        for h, amp in ((1, 1.0), (2, 0.35), (3, 0.18), (4, 0.08)):
            sig += amp * np.sin(2 * np.pi * ff * h * t + rng.uniform(0, 6.28))
    return sig / 6 * env_adsr(n, 0.6, 0.4, 0.8, 0.8)


def pluck(midi, dur=0.5, bright=1.0):
    n = int(dur * SR)
    t = np.arange(n) / SR
    f = hz(midi)
    sig = (np.sin(2 * np.pi * f * t) + 0.4 * bright * np.sin(2 * np.pi * 2 * f * t)
           + 0.15 * bright * np.sin(2 * np.pi * 3 * f * t))
    return sig * np.exp(-t * 7) * np.minimum(1, t * 400)


def bell(midi, dur=1.6):
    n = int(dur * SR)
    t = np.arange(n) / SR
    f = hz(midi)
    sig = (np.sin(2 * np.pi * f * t) + 0.5 * np.sin(2 * np.pi * 2.76 * f * t) * np.exp(-t * 6)
           + 0.25 * np.sin(2 * np.pi * 5.4 * f * t) * np.exp(-t * 9))
    return sig * np.exp(-t * 2.6) * np.minimum(1, t * 600)


def kick(dur=0.45):
    n = int(dur * SR)
    t = np.arange(n) / SR
    f = 45 + 75 * np.exp(-t * 28)
    phase = 2 * np.pi * np.cumsum(f) / SR
    return np.sin(phase) * np.exp(-t * 9)


def shaker(dur=0.09):
    n = int(dur * SR)
    t = np.arange(n) / SR
    x = rng.standard_normal(n)
    x = x - onepole(x, 5000)  # keep the top
    return x * np.exp(-t * 55)


def whoosh(dur=0.7, rise=True):
    n = int(dur * SR)
    t = np.arange(n) / SR
    x = rng.standard_normal(n)
    sweep = np.linspace(300, 3200, n) if rise else np.linspace(3200, 300, n)
    y = onepole(x, sweep)
    return y * np.sin(np.pi * t / dur) ** 2


def thud(dur=0.6):
    n = int(dur * SR)
    t = np.arange(n) / SR
    f = hz(41) * (1 + 0.8 * np.exp(-t * 20))  # F2, falling in
    return np.sin(2 * np.pi * np.cumsum(f) / SR) * np.exp(-t * 6)


music = np.zeros((2, N))
fx = np.zeros((2, N))

# Chords, a bar each: F, C, Dm, Bb.
CHORDS = [
    [53, 57, 60, 65],   # F
    [48, 55, 60, 64],   # C
    [50, 57, 62, 65],   # Dm
    [46, 53, 58, 62],   # Bb
]
ROOTS = [41, 36, 38, 34]
ARP = [[65, 69, 72, 77], [64, 67, 72, 76], [62, 65, 69, 74], [62, 65, 70, 74]]
GROOVE = 3.55   # the reveal: bass, kick and arpeggio come in
bars = int(DUR / BAR) + 1
for b in range(bars):
    start = b * BAR
    c = b % 4
    # The outro lands back on F.
    if start >= 28.6 - 0.01 and start < 29.9:
        c = 0
    for note in CHORDS[c]:
        add(music, start, pad_voice(note, BAR + 0.8), 0.22, pan=rng.uniform(-.4, .4))
    if start + BAR > GROOVE:
        for beat in (0, 2):
            at = start + beat * BEAT
            if GROOVE <= at < 30.8:
                n = int(BEAT * 1.8 * SR); tt = np.arange(n) / SR
                bass = np.sin(2 * np.pi * hz(ROOTS[c]) * tt) + 0.25 * np.sin(2 * np.pi * 2 * hz(ROOTS[c]) * tt)
                add(music, at, bass * np.exp(-tt * 2.2) * np.minimum(1, tt * 200), 0.28)
    for step in range(8):
        at = start + step * BEAT / 2
        if at < 0.25 or at > 30.9:
            continue
        note = ARP[c][[0, 1, 2, 3, 2, 1, 2, 3][step]]
        quiet = 0.45 if at < GROOVE else 1.0
        add(music, at, pluck(note), 0.075 * quiet, pan=(-.35 if step % 2 else .35))
    for beat in range(4):
        at = start + beat * BEAT
        if GROOVE <= at < 30.8:
            add(music, at, kick(), 0.42)
        off = at + BEAT / 2
        if 6.2 <= off < 30.8:
            add(music, off, shaker(), 0.05, pan=.25)

# Effects, in key, on the same reverb bus.
add(fx, 1.92, thud(), 0.55)
for note, dt in ((77, 0.0), (81, 0.06), (84, 0.12)):   # F5 A5 C6: the window lands
    add(fx, 1.95 + dt, bell(note), 0.11, pan=.1)
for at in (6.15, 10.55, 16.15, 20.55, 25.35, 28.5):
    add(fx, at - 0.35, whoosh(0.7), 0.05)
add(fx, 3.35, whoosh(0.6), 0.04)
for at, note in ((6.95, 84), (9.5, 81),
                 (11.0, 84), (12.0, 81), (13.2, 88), (14.2, 81), (14.45, 81),   # the phone on the computer
                 (17.35, 84), (18.2, 88), (18.55, 89),                          # both ways
                 (22.2, 84), (23.3, 88),                                        # the computer in the hand
                 (25.9, 84), (26.25, 89), (26.6, 93)):                          # from anywhere
    add(fx, at, pluck(note, 0.35, bright=.6), 0.07, pan=rng.uniform(-.3, .3))
add(fx, 12.8, bell(89, 1.0), 0.07)                   # answered on the phone
add(fx, 19.15, pluck(62, 0.5, bright=.3), 0.09)     # the break: a low D
add(fx, 20.15, bell(89, 1.2), 0.09)                  # saved: F6
for note in (65, 69, 72, 77):                          # the outro swell
    add(fx, 28.6, pad_voice(note + 12, 2.6), 0.07)

dry = music + fx

# Reverb: a decaying noise tail, the same space for everything.
ir_len = int(1.8 * SR)
tt = np.arange(ir_len) / SR
irs = [rng.standard_normal(ir_len) * np.exp(-tt * 3.2) for _ in range(2)]
for ir in irs:
    ir[: int(0.012 * SR)] = 0
wet = np.zeros_like(dry)
for ch in range(2):
    size = 1 << int(np.ceil(np.log2(N + ir_len)))
    out = np.fft.irfft(np.fft.rfft(dry[ch], size) * np.fft.rfft(irs[ch], size), size)[:N]
    wet[ch] = onepole(out, 6000)
wet *= 0.18 / (np.max(np.abs(wet)) + 1e-9) * np.max(np.abs(dry))
mix = dry + wet

# Fade in the first 40 ms and out over the last 0.8 s; gentle saturation.
fade = np.ones(N)
fade[: int(0.04 * SR)] = np.linspace(0, 1, int(0.04 * SR))
fade[-int(0.8 * SR):] = np.linspace(1, 0, int(0.8 * SR)) ** 1.5
mix *= fade
mix = np.tanh(mix * 1.4) / np.tanh(1.4)
mix *= 10 ** (-1 / 20) / np.max(np.abs(mix))

pcm = (mix.T * 32767).astype(np.int16)
with wave.open('soundtrack.wav', 'wb') as w:
    w.setnchannels(2)
    w.setsampwidth(2)
    w.setframerate(SR)
    w.writeframes(pcm.tobytes())
print('soundtrack.wav', N / SR, 's')
