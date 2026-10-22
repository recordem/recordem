# /// script
# requires-python = ">=3.10"
# dependencies = ["numpy", "scipy", "huggingface_hub"]
# ///
"""pyannote's community-1 PLDA, for VBx clustering in the engine (engine/src/vbx.rs), as the binary it embeds.

    HF_TOKEN=… uv run apps/recorder/eval/plda.py --out apps/recorder/engine/src/weights/plda.bin

Downloads plda/xvec_transform.npz and plda/plda.npz from pyannote/speaker-diarization-community-1 (gated: accept its
conditions on Hugging Face first; CC BY 4.0) and does here, once, what pyannote's vbx_setup does at load time: the
generalized eigendecomposition that makes the PLDA's within- and across-speaker covariances diagonal. The engine then
only multiplies.

The file: three little-endian u32 (in, lda, features: 256, 128, 128), then f32 arrays in this order: mean1[in],
lda[in][lda], mean2[lda], mu[lda], tr[features][lda], phi[features].
"""

from __future__ import annotations

import argparse
import struct
from pathlib import Path

import numpy as np
from huggingface_hub import hf_hub_download
from scipy.linalg import eigh

REPO = "pyannote/speaker-diarization-community-1"


def main() -> None:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--out", type=Path, required=True)
    p.add_argument("--lda-dim", type=int, default=128)
    args = p.parse_args()

    x = np.load(hf_hub_download(REPO, "plda/xvec_transform.npz"))
    plda = np.load(hf_hub_download(REPO, "plda/plda.npz"))
    mean1, mean2, lda = x["mean1"], x["mean2"], x["lda"]
    mu, tr, psi = plda["mu"], plda["tr"], plda["psi"]

    # pyannote.audio's vbx_setup
    w = np.linalg.inv(tr.T.dot(tr))
    b = np.linalg.inv((tr.T / psi).dot(tr))
    acvar, wccn = eigh(b, w)
    psi = acvar[::-1]
    tr = wccn.T[::-1]

    d = args.lda_dim
    din, dlda = lda.shape
    # features are (x - mu) · trᵀ, keeping the first d
    arrays = [mean1, lda, mean2, mu, tr[:d], psi[:d]]
    args.out.parent.mkdir(parents=True, exist_ok=True)
    with args.out.open("wb") as f:
        f.write(struct.pack("<3I", din, dlda, d))
        for a in arrays:
            f.write(np.ascontiguousarray(a, dtype="<f4").tobytes())
    print(f"{args.out}: in {lda.shape[0]}, lda {lda.shape[1]}, features {d}")


if __name__ == "__main__":
    main()
