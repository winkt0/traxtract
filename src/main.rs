use std::ffi::OsString;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use clap::Parser;


#[derive(Debug)]
struct Track {
    start: f64,          // start time in seconds
    title: Option<String>,
}

/// Split an audio file into individual tracks at the given timestamps.
///
/// Each timestamp marks the START of a track; the track runs until the next
/// timestamp (or the end of the file for the last one).
#[derive(Parser, Debug)]
#[command(name = "traxtract", version, about, long_about = None)]
struct Args {
    /// Path to the input audio file.
    input: PathBuf,

    /// Timestamps marking the start of each track.
    ///
    /// Entries are separated by NEWLINES, or by COMMAS if there are no
    /// newlines. Each entry is a timestamp optionally followed (or preceded)
    /// by a title. Examples:
    ///
    ///   "0:00 Intro, 3:45 Second Song, 7:12 Third Song"
    ///
    ///   "1:02:30 Encore"        (H:MM:SS)
    ///
    /// Timestamp formats: SS, MM:SS, or HH:MM:SS, with optional fractional
    /// seconds (e.g. 3:45.5). The first timestamp need not be 0:00 — anything
    /// before it is skipped.
    #[arg(required_unless_present = "tracklist_file")]
    timestamps: Option<String>,

    /// Read the tracklist from a file instead of passing it on the command line
    /// (handy for long, newline-separated lists pasted from a description).
    #[arg(short = 'f', long, value_name = "FILE")]
    tracklist_file: Option<PathBuf>,

    /// Output directory (created if missing). Default: "<input name> tracks".
    #[arg(short, long, value_name = "DIR")]
    output_dir: Option<PathBuf>,

    /// Re-encode instead of stream-copying. Slower and lossy, but gives
    /// sample-accurate cuts (a plain stream copy can only cut on keyframe
    /// boundaries, which for some formats means a fraction of a second of slop).
    #[arg(short, long)]
    reencode: bool,

    /// Output extension / container (e.g. mp3, flac, wav, m4a). Defaults to the
    /// input's extension. Choosing a different one forces --reencode.
    #[arg(short, long, value_name = "EXT")]
    ext: Option<String>,

    /// Extra argument passed straight through to ffmpeg (repeatable), inserted
    /// just before the output file. E.g. --ffmpeg-arg -q:a --ffmpeg-arg 2
    #[arg(long = "ffmpeg-arg", value_name = "ARG", allow_hyphen_values = true)]
    ffmpeg_arg: Vec<String>,

    /// Print the ffmpeg commands that would run, but don't execute them.
    #[arg(long)]
    dry_run: bool,

    /// Path to the ffmpeg binary.
    #[arg(long, default_value = "ffmpeg", value_name = "PATH")]
    ffmpeg: String,
}

fn main() {
    let args = Args::parse();
    if let Err(e) = run(&args) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn run(args: &Args) -> Result<(), String> {
    if !args.input.is_file() {
        return Err(format!("input file not found: {}", args.input.display()));
    }

    // Gather the raw tracklist text from either the file or the argument.
    let raw = match (&args.tracklist_file, &args.timestamps) {
        (Some(path), _) => std::fs::read_to_string(path)
            .map_err(|e| format!("could not read tracklist file {}: {e}", path.display()))?,
        (None, Some(s)) => s.clone(),
        (None, None) => return Err("no timestamps provided".into()),
    };

    let tracks = parse_tracklist(&raw)?;
    if tracks.is_empty() {
        return Err("no tracks could be parsed from the timestamps".into());
    }

    // Timestamps must be strictly increasing so every track has positive length.
    for w in tracks.windows(2) {
        if w[1].start <= w[0].start {
            return Err(format!(
                "timestamps must be strictly increasing (found {} then {})",
                fmt_time(w[0].start),
                fmt_time(w[1].start)
            ));
        }
    }
    if tracks[0].start > 0.0 {
        eprintln!(
            "note: first track starts at {}; audio before that will be skipped",
            fmt_time(tracks[0].start)
        );
    }

    // Figure out the output extension and whether we must re-encode.
    let input_ext = args
        .input
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    let ext = args
        .ext
        .clone()
        .unwrap_or_else(|| input_ext.to_string())
        .trim_start_matches('.')
        .to_string();
    if ext.is_empty() {
        return Err("could not determine an output extension; pass --ext".into());
    }
    let reencode = args.reencode || !ext.eq_ignore_ascii_case(input_ext);

    // Output directory.
    let out_dir = args.output_dir.clone().unwrap_or_else(|| {
        let stem = args
            .input
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("audio");
        PathBuf::from(format!("{stem} tracks"))
    });
    if !args.dry_run {
        std::fs::create_dir_all(&out_dir)
            .map_err(|e| format!("could not create output dir {}: {e}", out_dir.display()))?;
    }

    // Make sure ffmpeg is actually runnable before we start looping (nicer error).
    if !args.dry_run {
        preflight_ffmpeg(&args.ffmpeg)?;
    }

    let total = tracks.len();
    let width = total.to_string().len().max(2);

    for (i, track) in tracks.iter().enumerate() {
        let n = i + 1;
        let start = track.start;
        // Duration = gap to the next track's start; None for the last track (to EOF).
        let duration = tracks.get(i + 1).map(|next| next.start - start);

        let filename = match &track.title {
            Some(t) => format!("{n:0width$} - {}.{ext}", sanitize_filename(t)),
            None => format!("{n:0width$}.{ext}"),
        };
        let out_path = out_dir.join(&filename);

        let ff = build_ffmpeg_args(args, &ext, start, duration, track, n, reencode, &out_path);

        if args.dry_run {
            println!("{}", render_command(&args.ffmpeg, &ff));
            continue;
        }

        println!("[{n}/{total}] {filename}");
        let status = Command::new(&args.ffmpeg)
            .args(&ff)
            .stdin(Stdio::null())
            .status()
            .map_err(|e| format!("failed to launch ffmpeg ({}): {e}", args.ffmpeg))?;
        if !status.success() {
            return Err(format!(
                "ffmpeg failed on track {n} ({filename}); exit code {:?}",
                status.code()
            ));
        }
    }

    if !args.dry_run {
        println!("done — {total} track(s) written to {}", out_dir.display());
    }
    Ok(())
}

/// Assemble the full ffmpeg argument list for one track.
#[allow(clippy::too_many_arguments)]
fn build_ffmpeg_args(
    args: &Args,
    ext: &str,
    start: f64,
    duration: Option<f64>,
    track: &Track,
    track_no: usize,
    reencode: bool,
    out_path: &PathBuf,
) -> Vec<OsString> {
    let mut ff: Vec<OsString> = Vec::new();
    ff.push("-nostdin".into());
    ff.push("-y".into()); // overwrite existing output
    ff.push("-loglevel".into());
    ff.push("error".into());

    // Input-side seek (fast; before -i). With -c copy this snaps to the nearest
    // keyframe; with re-encoding ffmpeg seeks accurately.
    ff.push("-ss".into());
    ff.push(fmt_secs(start).into());
    ff.push("-i".into());
    ff.push(args.input.clone().into_os_string());

    // Output duration (from the seek point). Omitted for the final track.
    if let Some(d) = duration {
        ff.push("-t".into());
        ff.push(fmt_secs(d).into());
    }

    if reencode {
        // Let ffmpeg pick the default encoder for the container. Users can
        // override quality/codec via --ffmpeg-arg.
        let _ = ext;
    } else {
        ff.push("-c".into());
        ff.push("copy".into());
    }

    // Carry over album/artist/etc. from the source, then set per-track tags.
    ff.push("-map_metadata".into());
    ff.push("0".into());
    if let Some(t) = &track.title {
        ff.push("-metadata".into());
        ff.push(format!("title={t}").into());
    }
    ff.push("-metadata".into());
    ff.push(format!("track={track_no}").into());

    for extra in &args.ffmpeg_arg {
        ff.push(extra.into());
    }

    ff.push(out_path.clone().into_os_string());
    ff
}

/// Split raw tracklist text into entries and parse each one.
fn parse_tracklist(raw: &str) -> Result<Vec<Track>, String> {
    // If there are newlines, split on them (so titles may contain commas).
    // Otherwise fall back to comma separation for one-line CLI input.
    let entries: Vec<&str> = if raw.contains('\n') {
        raw.lines().collect()
    } else {
        raw.split(',').collect()
    };

    let mut tracks = Vec::new();
    for entry in entries {
        if entry.trim().is_empty() {
            continue;
        }
        tracks.push(parse_entry(entry)?);
    }
    Ok(tracks)
}

/// Parse one entry like "3:45 Song Name" or "Song Name 3:45".
fn parse_entry(entry: &str) -> Result<Track, String> {
    let entry = entry.trim();
    let tokens: Vec<&str> = entry.split_whitespace().collect();
    if tokens.is_empty() {
        return Err("empty tracklist entry".into());
    }

    // Preferred form: timestamp first.
    if let Ok(start) = parse_timestamp(tokens[0]) {
        return Ok(Track {
            start,
            title: clean_title(&tokens[1..].join(" ")),
        });
    }

    // Fallback: timestamp last, e.g. "Song Name 3:45".
    if tokens.len() >= 2 {
        if let Ok(start) = parse_timestamp(tokens[tokens.len() - 1]) {
            return Ok(Track {
                start,
                title: clean_title(&tokens[..tokens.len() - 1].join(" ")),
            });
        }
    }

    Err(format!("no valid timestamp found in entry: {entry:?}"))
}

/// Parse a timestamp in SS, MM:SS, or HH:MM:SS form (fractional seconds ok).
fn parse_timestamp(s: &str) -> Result<f64, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("empty timestamp".into());
    }
    let groups: Vec<&str> = s.split(':').collect();
    if groups.len() > 3 {
        return Err(format!("too many ':' groups in timestamp {s:?}"));
    }
    let mut total = 0f64;
    for g in &groups {
        let g = g.trim();
        let val: f64 = g
            .parse()
            .map_err(|_| format!("invalid number {g:?} in timestamp {s:?}"))?;
        if val < 0.0 {
            return Err(format!("negative value in timestamp {s:?}"));
        }
        total = total * 60.0 + val;
    }
    Ok(total)
}

/// Trim leading separators/whitespace off a title; return None if empty.
fn clean_title(s: &str) -> Option<String> {
    let t = s
        .trim()
        .trim_start_matches(|c: char| matches!(c, '-' | '–' | '—' | ':' | '.' | '|' | ')'))
        .trim();
    if t.is_empty() {
        None
    } else {
        Some(t.to_string())
    }
}

/// Replace characters that are illegal or awkward in filenames.
fn sanitize_filename(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => out.push('_'),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    let trimmed = out.trim().trim_matches('.').trim();
    if trimmed.is_empty() {
        "track".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Format seconds for ffmpeg (millisecond precision is plenty).
fn fmt_secs(secs: f64) -> String {
    format!("{secs:.3}")
}

/// Human-readable time for messages: M:SS or H:MM:SS.
fn fmt_time(secs: f64) -> String {
    let total = secs.round() as i64;
    let s = total % 60;
    let m = (total / 60) % 60;
    let h = total / 3600;
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// Confirm ffmpeg can be launched; give a friendly hint if not.
fn preflight_ffmpeg(bin: &str) -> Result<(), String> {
    match Command::new(bin)
        .arg("-version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
    {
        Ok(status) if status.success() => Ok(()),
        Ok(_) => Err(format!("'{bin} -version' returned an error; is ffmpeg installed?")),
        Err(e) => Err(format!(
            "could not run ffmpeg ('{bin}'): {e}. Install ffmpeg or pass --ffmpeg <path>."
        )),
    }
}

/// Render a runnable, copy-pasteable command line (for --dry-run).
fn render_command(bin: &str, args: &[OsString]) -> String {
    let mut parts = vec![shell_quote(bin)];
    for a in args {
        parts.push(shell_quote(&a.to_string_lossy()));
    }
    parts.join(" ")
}

fn shell_quote(s: &str) -> String {
    let safe = !s.is_empty()
        && s.chars()
            .all(|c| c.is_alphanumeric() || "-_./:=@%+,".contains(c));
    if safe {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}