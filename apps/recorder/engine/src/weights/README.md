# plda.bin

The PLDA of pyannote's community-1 speaker diarization pipeline, which `vbx.rs` uses to tell voices apart.

- Source: [pyannote/speaker-diarization-community-1](https://huggingface.co/pyannote/speaker-diarization-community-1)
  (`plda/xvec_transform.npz`, `plda/plda.npz`), © pyannote, licensed
  [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/).
- Changed: converted by `apps/recorder/eval/plda.py` — the PLDA's generalized eigendecomposition done ahead of time
  (as pyannote.audio's `vbx_setup` does at load), kept to 128 dimensions, stored as little-endian f32.
