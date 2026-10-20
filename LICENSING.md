# Licensing

| Path | License | Why |
|------|---------|-----|
| `apps/api`, `apps/auth`, `apps/worker`, `apps/web`, `apps/mcp` | AGPL-3.0 (`LICENSE`) | Anyone may self-host or modify; whoever offers a modified server as a service publishes the changes |
| `apps/recorder` | MIT | A desktop client people install; free to fork or embed |
| `packages/contract`, `packages/sdk`, `packages/cli`, `skills` | MIT | Integrations must not inherit the AGPL |

Each MIT path carries its own `LICENSE` file and `"license": "MIT"` in its manifest. A file's license is the one of
the folder it lives in.

**Contributions** are accepted under a CLA (to be set up with CLA Assistant before the first external pull request),
so the project can keep offering the hosted version and change licensing terms without asking every contributor again.

**Hosted version.** recordem.dev runs this same code. What only the hosted version needs (billing, plan limits,
abuse controls) lives in a separate private repository and plugs in through the server's extension points — there is
no closed folder in this repository.

**Third-party code.** The recorder uses Anarlog's MIT crates (`audio`, `audio-actual`, `detect`, `pyannote-local`)
pinned as git dependencies; nothing under their `enterprise/` folder (commercial license) may be used. whisper.cpp,
the Whisper weights and Silero VAD are MIT. Check the license of the pyannote model weights before shipping them in a
release.
