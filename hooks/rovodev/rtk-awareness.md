<!-- rtk-instructions -->
## RTK — Token-Optimized Shell Commands

Always prefix shell commands with `rtk` to reduce token consumption (60-90% savings on common dev operations).

```bash
rtk git status        # rtk git log -10        rtk git diff
rtk cargo test        # rtk cargo build        rtk cargo clippy
rtk npm run build     # rtk pnpm install       rtk pytest -q
rtk grep "pat" src/   # rtk ls src/            rtk find . -name "*.rs"
rtk docker ps         # rtk kubectl get pods   rtk gh pr list
```

Even in command chains, prefix every command:

```bash
rtk git add . && rtk git commit -m "msg" && rtk git push
```

### Meta commands

```bash
rtk gain              # Token savings dashboard
rtk gain --history    # Per-command savings history
rtk proxy <cmd>       # Run raw command without filtering (debug)
```

### Verify install

```bash
rtk --version         # Should print: rtk X.Y.Z
rtk gain              # Should show dashboard (not "command not found")
```

> If `rtk gain` fails, you may have `reachingforthejack/rtk` (Rust Type Kit) installed instead. Reinstall from `rtk-ai/rtk`.
<!-- /rtk-instructions -->
