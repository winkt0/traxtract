# traxtract
Rust CLI that splits one audio file (a concatenated album, mixtape, DJ
set, or playlist) into individual tracks at timestamps you provide. It uses
`ffmpeg` under the hood, so any format ffmpeg reads works: mp3, flac, m4a, wav,
ogg, opus, …

## Requirements

- Rust / Cargo (to build)
- `ffmpeg` on your `PATH` (or pass `--ffmpeg /path/to/ffmpeg`)

## Build / Installation

```sh
git clone https://github.com/winkt0/traxtract
cd traxtract
cargo build --release
```
The binary is then at target/release/traxtract

Or, you can just install with cargo:

```sh
cargo install --git https://github.com/winkt0/traxtract
```

## Usage

```
traxtract <INPUT> <TIMESTAMPS> [options]
```

Each timestamp marks the **start** of a track; the track runs until the next
timestamp, and the last track runs to the end of the file.

```sh
# Comma-separated, with titles
traxtract set.mp3 "0:00 Intro, 3:45 Second Song, 7:12 Third Song"

# Bare timestamps (files are just numbered 01, 02, …)
traxtract album.flac "0, 3:45, 7:12, 10:30"
```

Timestamp formats: `SS`, `MM:SS`, or `HH:MM:SS`, with optional fractional
seconds (`3:45.5`). Titles may come before or after the timestamp
(`3:45 Song` or `Song 3:45`).

### Long tracklists from a file

If titles contain commas, use one entry per line (commas are only treated as
separators when the input has no newlines):

```sh
traxtract set.mp3 -f tracklist.txt
```

```
# tracklist.txt
0:00 Hello, Goodbye
3:45 Money, Money, Money
7:12 Encore
```

### Options

| Flag | Meaning |
|------|---------|
| `-o, --output-dir <DIR>` | Output directory (default: `"<input name> tracks"`) |
| `-f, --tracklist-file <FILE>` | Read timestamps/titles from a file |
| `-r, --reencode` | Re-encode for sample-accurate cuts (default is a fast, lossless stream copy) |
| `-e, --ext <EXT>` | Output container, e.g. `mp3`, `flac` (differing from input forces `--reencode`) |
| `--ffmpeg-arg <ARG>` | Pass an extra arg to ffmpeg (repeatable), e.g. `--ffmpeg-arg -q:a --ffmpeg-arg 2` |
| `--dry-run` | Print the ffmpeg commands without running them |
| `--ffmpeg <PATH>` | Path to the ffmpeg binary |

## Notes on accuracy

By default the tool stream-copies (`-c copy`) which entails no re-encoding, so it's fast and
lossless, but cuts can only land on keyframe boundaries — for some lossy formats
that's up to a fraction of a second of slop. Use `--reencode` if you need
cuts exactly on the timestamp. Track title and number metadata are written to
each output, and album/artist tags are carried over from the source.