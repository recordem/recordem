# /// script
# requires-python = ">=3.10"
# dependencies = ["datasets>=3.0,<4", "soundfile", "numpy", "librosa"]
# ///
"""Public meetings with who-spoke-when references, as fixtures for the engine's eval binary (engine/src/eval.rs).

    uv run apps/recorder/eval/fetch_public.py --out ~/recordem-eval/public --limit 3

Each recording becomes <out>/<dataset>-<split>-<n>/system.wav (16 kHz mono) + reference.rttm: the whole meeting as the
other side of a call, which is what the diarization sees. Streamed, so only the recordings asked for are downloaded.
AMI and VoxConverse are CC BY 4.0 (diarizers-community on Hugging Face); evaluation only, never committed or shipped.
"""

from __future__ import annotations

import argparse
from pathlib import Path

import librosa
import numpy as np
import soundfile as sf
from datasets import load_dataset

RATE = 16_000
# AMI "ihm": the headsets mixed, closer to a call's audio than "sdm" (one microphone in the room).
DATASETS = ["diarizers-community/ami@ihm:validation", "diarizers-community/voxconverse:dev"]


def fetch(spec: str, out: Path, limit: int) -> int:
    name, _, split = spec.partition(":")
    name, _, config = name.partition("@")
    rows = load_dataset(name, config or None, split=split or "validation", streaming=True)
    prefix = f"{name.split('/')[-1]}-{config + '-' if config else ''}{split}"
    written = 0
    for index, row in enumerate(rows):
        if written >= limit:
            break
        dir = out / f"{prefix}-{index:03d}"
        if (dir / "system.wav").exists():
            written += 1
            continue
        audio = np.asarray(row["audio"]["array"], dtype=np.float32)
        if audio.ndim > 1:
            audio = audio.mean(axis=1)
        if row["audio"]["sampling_rate"] != RATE:
            audio = librosa.resample(audio, orig_sr=row["audio"]["sampling_rate"], target_sr=RATE)
        dir.mkdir(parents=True, exist_ok=True)
        sf.write(dir / "system.wav", audio, RATE, subtype="PCM_16")
        lines = [
            f"SPEAKER {dir.name} 1 {float(s):.3f} {float(e) - float(s):.3f} <NA> <NA> {who} <NA> <NA>"
            for s, e, who in zip(row["timestamps_start"], row["timestamps_end"], row["speakers"])
            if float(e) > float(s)
        ]
        (dir / "reference.rttm").write_text("\n".join(lines) + "\n")
        (dir / "SOURCE").write_text(f"{name} {split} #{index} (CC BY 4.0)\n")
        print(f"{dir.name}: {len(audio) / RATE / 60:.1f} min, {len(set(row['speakers']))} speakers")
        written += 1
    return written


def main() -> None:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--out", type=Path, required=True)
    p.add_argument("--dataset", action="append", help=f"hub id[@config][:split]; default: {' '.join(DATASETS)}")
    p.add_argument("--limit", type=int, default=3, help="recordings per dataset")
    args = p.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)
    for spec in args.dataset or DATASETS:
        print(f"{spec}: {fetch(spec, args.out, args.limit)} recordings in {args.out}")


if __name__ == "__main__":
    main()
