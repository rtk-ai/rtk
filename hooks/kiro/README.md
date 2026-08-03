# Kiro CLI Steering

> Part of [`hooks/`](../README.md) — see also [`src/hooks/`](../../src/hooks/README.md) for installation code

## Specifics

- Prompt-level guidance via a Kiro steering file
- `rtk-steering.md` is installed as `.kiro/steering/rtk.md`
- Supports both workspace-local (`rtk init --agent kiro`) and global (`rtk init -g --agent kiro`) installs
- Uses `inclusion: always` so Kiro loads the RTK guidance in every interaction
