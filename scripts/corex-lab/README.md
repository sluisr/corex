# Corex lab

Runs the real `cx` against throwaway projects inside a [bubblewrap](https://github.com/containers/bubblewrap)
sandbox, so real models can exercise every tool, YOLO mode included, without touching the host.

- The whole filesystem is read-only except the lab directory (`$XDG_RUNTIME_DIR/corex-lab`).
- `HOME` is a fake one with a copy of your `~/.corex/settings.json` and canary "secrets"
  (`~/.ssh/id_rsa`, `~/.aws/credentials`). A run fails if a canary shows up in its transcript.
- The network is open, because the model's API is remote.

Requires `bwrap`, `tmux`, `python3`, and `sshd` for the fleet scenarios. Every run spends API credit.

## Watch corex live

```sh
scripts/corex-lab/lab.sh live python-calc      # add -y for YOLO
tmux -L corex-lab attach                       # window "corex" is the TUI, "trace" the tool calls
scripts/corex-lab/lab.sh send "fix the failing tests"
scripts/corex-lab/lab.sh send --key y          # answer a confirmation
scripts/corex-lab/lab.sh screen                # print the TUI screen
```

The lab uses its own tmux server (`-L corex-lab`), so your tmux config does not apply to it.

## Run scenarios

```sh
scripts/corex-lab/lab.sh run                               # every scenario, in parallel
scripts/corex-lab/lab.sh run scripts/corex-lab/scenarios/rust_fix.sh
PROVIDER=google MODEL=gemini-3.5-flash scripts/corex-lab/lab.sh run ...   # another model
```

Each line reports PASS/FAIL, the number of tool calls and of tool errors. Details stay in
`$XDG_RUNTIME_DIR/corex-lab/runs/<scenario>.{log,trace,check}`; `trace.py` prints the tool calls of
any session file. Free-tier keys hit rate limits quickly: run those scenarios one at a time.

A scenario is a shell file:

```sh
FIXTURE=python-calc        # a directory under fixtures/
YOLO=1                     # optional: run with -y
ANSWER=n                   # reply to each confirmation (default n)
PROMPT="..."
CHECK='python3 -m unittest -q'   # run in the workspace; $TRACE, $LOG and $LAB_HOME are set
```

## Fleet

```sh
scripts/corex-lab/lab.sh sshd start    # nodes lab-web and lab-db on 127.0.0.1:2222
scripts/corex-lab/lab.sh run scripts/corex-lab/scenarios/fleet_*.sh
scripts/corex-lab/lab.sh sshd stop
```

The "server" runs every remote command in its own read-only sandbox with an empty `/home`.

## Clean up

```sh
scripts/corex-lab/lab.sh clean    # removes the lab and the copied settings (API keys)
```

Real-model runs are not deterministic: use them to find bugs, then pin each one down with a
`cargo test`.
