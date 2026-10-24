"""Offline postgame transcription using the user's existing Faster-Whisper installation."""
import json
import math
import os
from pathlib import Path
import subprocess
import sys


def timed_words(segments, duration):
    if not math.isfinite(duration) or duration < 0:
        raise ValueError("Invalid audio duration")
    result = []
    for segment in segments:
        if segment.text.strip() and not segment.words:
            raise ValueError("Whisper returned speech without requested word timestamps")
        for word in segment.words or []:
            start, end = float(word.start), float(word.end)
            if not (math.isfinite(start) and math.isfinite(end) and 0 <= start <= end):
                raise ValueError("Whisper returned invalid word timestamps")
            # Whisper's padded final window can extend beyond the actual recording.
            if start >= duration:
                continue
            if result and start < result[-1]["start"]:
                raise ValueError("Whisper returned out-of-order word timestamps")
            result.append({"start": start, "end": min(end, duration), "text": word.word})
    return {"duration_seconds": duration, "segments": result}


def transcribe(directory, cache):
    audio = Path(directory) / "audio.caf"
    if audio.is_symlink() or not audio.is_file():
        raise ValueError("Recording must be a regular audio.caf file")
    repository = Path(cache) / "models--Systran--faster-whisper-medium"
    revision = (repository / "refs" / "main").read_text().strip()
    if not revision or any(c not in "0123456789abcdef" for c in revision):
        raise ValueError("Invalid local Whisper cache revision")
    model_path = repository / "snapshots" / revision
    for name in ("model.bin", "config.json", "tokenizer.json", "vocabulary.txt"):
        if not (model_path / name).is_file():
            raise ValueError(f"Missing cached Whisper file: {name}; no download attempted")
    os.environ.update(
        HF_HUB_OFFLINE="1",
        TRANSFORMERS_OFFLINE="1",
        HF_HUB_DISABLE_IMPLICIT_TOKEN="1",
        HF_HUB_DISABLE_TELEMETRY="1",
    )
    import numpy as np
    import soundfile as sf
    from faster_whisper import WhisperModel

    audio_info = sf.info(str(audio))
    duration = audio_info.frames / audio_info.samplerate
    if duration == 0:
        return {"duration_seconds": 0.0, "segments": []}
    # The existing environment's PyAV API is incompatible with Whisper's file decoder.
    # Decode/resample through installed FFmpeg, keeping only mono 16 kHz audio in memory.
    decoded = subprocess.run(
        ["ffmpeg", "-nostdin", "-v", "error", "-protocol_whitelist", "file,pipe",
         "-f", "caf", "-i", str(audio), "-f", "f32le", "-ac", "1", "-ar", "16000", "pipe:1"],
        check=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
    )
    audio_samples = np.frombuffer(decoded.stdout, dtype="<f4")
    model = WhisperModel(
        str(model_path), device="cpu", compute_type="int8", cpu_threads=4,
        local_files_only=True,
    )
    segments, _ = model.transcribe(
        audio_samples, language="de", beam_size=5, condition_on_previous_text=False,
        word_timestamps=True, vad_filter=True,
    )
    return timed_words(segments, duration)


if __name__ == "__main__":
    try:
        if len(sys.argv) != 3:
            raise ValueError("Usage: transcribe.py RECORDING_DIRECTORY MODEL_CACHE")
        json.dump(transcribe(sys.argv[1], sys.argv[2]), sys.stdout, ensure_ascii=False, allow_nan=False)
        sys.stdout.write("\n")
    except Exception as error:
        print(f"Offline transcription failed: {error}", file=sys.stderr)
        sys.exit(1)
