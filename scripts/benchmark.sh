#!/usr/bin/env bash
# Corex resource footprint: measures what the README states, and prints only what it measured.
#
# Usage: ./scripts/benchmark.sh [--build] [path/to/cx]
#
#   --build        build target/release/cx first
#   path/to/cx     measure this binary instead (e.g. the one from a GitHub release)
#
# Needs python3 for the timings and the idle measurements. The memory and CPU readings use
# /proc, so they are Linux only; elsewhere the script reports the RSS from `ps` and skips CPU.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
BIN_PATH="${ROOT_DIR}/target/release/cx"
BUILD=0

for arg in "$@"; do
    case "$arg" in
        --build) BUILD=1 ;;
        *) BIN_PATH="$arg" ;;
    esac
done

if [[ "$BUILD" == 1 ]] || [[ ! -f "$BIN_PATH" ]]; then
    echo "==> Building Corex in release mode..."
    cargo build --release --manifest-path "${ROOT_DIR}/Cargo.toml" --bin cx
    BIN_PATH="${ROOT_DIR}/target/release/cx"
fi

if ! command -v python3 >/dev/null 2>&1; then
    echo "python3 is required for the timing and idle measurements." >&2
    exit 1
fi

size_of() { stat -c%s "$1" 2>/dev/null || stat -f%z "$1"; }
mb() { awk "BEGIN {printf \"%.2f\", $1/1000000}"; }

echo "=========================================================="
echo "  Corex resource footprint"
echo "=========================================================="
echo "Date:    $(date -u +"%Y-%m-%d %H:%M:%SZ")"
echo "Version: $("$BIN_PATH" --version 2>&1 | head -1)"
echo "Binary:  ${BIN_PATH}"
echo "Kernel:  $(uname -srm)"
echo "CPU:     $( (lscpu 2>/dev/null | sed -n 's/^Model name:[[:space:]]*//p') || true)"
echo ""

# 1. Size and linking
RAW_SIZE=$(size_of "$BIN_PATH")
TMP_STRIPPED="$(mktemp)"
cp "$BIN_PATH" "$TMP_STRIPPED"
strip "$TMP_STRIPPED" 2>/dev/null || true
STRIPPED_SIZE=$(size_of "$TMP_STRIPPED")
rm -f "$TMP_STRIPPED"

echo "==> 1. Size and linking"
echo "    Binary:           ${RAW_SIZE} bytes ($(mb "$RAW_SIZE") MB)"
echo "    After strip:      ${STRIPPED_SIZE} bytes ($(mb "$STRIPPED_SIZE") MB)"
echo "    Type:             $(file -b "$BIN_PATH" 2>/dev/null | cut -d, -f1-2,5- | cut -c1-110 || echo unknown)"
if command -v readelf >/dev/null 2>&1 && readelf -h "$BIN_PATH" >/dev/null 2>&1; then
    LIBS=$(readelf -d "$BIN_PATH" 2>/dev/null | sed -n 's/.*Shared library: \[\(.*\)\]/\1/p' | tr '\n' ' ')
    GLIBC=$(readelf -W --dyn-syms "$BIN_PATH" 2>/dev/null | grep -o 'GLIBC_[0-9.]*' | sort -V -u | tail -1 || true)
    echo "    Shared libraries: ${LIBS:-none}"
    echo "    Newest glibc symbol version required: ${GLIBC:-none}"
elif command -v otool >/dev/null 2>&1; then
    echo "    Shared libraries: $(otool -L "$BIN_PATH" | tail -n +2 | awk '{print $1}' | tr '\n' ' ')"
fi
echo ""

# 2. Timings and idle footprint, measured on a clean HOME so nothing of the user's is read or written.
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
mkdir -p "$WORK/home" "$WORK/workspace"

python3 - "$BIN_PATH" "$WORK/home" "$WORK/workspace" <<'PY'
import os, pty, re, select, signal, statistics, struct, subprocess, sys, time

binary, home, workspace = sys.argv[1:4]
env = dict(os.environ, HOME=home, TERM="xterm-256color")
for key in ("COREX_API_KEY", "DEEPSEEK_API_KEY", "ANTHROPIC_API_KEY", "OPENAI_API_KEY",
            "GEMINI_API_KEY", "GROQ_API_KEY", "MISTRAL_API_KEY", "GITHUB_TOKEN", "GH_TOKEN"):
    env.pop(key, None)

def ms(values):
    return f"min {min(values):.1f} · median {statistics.median(values):.1f} · max {max(values):.1f} ms"

# 2a. Process start: `cx --version` end to end.
RUNS = 50
times = []
for _ in range(RUNS):
    start = time.perf_counter()
    subprocess.run([binary, "--version"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, env=env)
    times.append((time.perf_counter() - start) * 1000)
print(f"==> 2. Startup of `cx --version` ({RUNS} runs)")
print(f"    {ms(times)}")
print()

def spawn():
    """Starts the interactive TUI on a pseudo-terminal; returns (pid, fd)."""
    pid, fd = pty.fork()
    if pid == 0:
        os.chdir(workspace)
        os.execve(binary, ["cx"], env)
    import fcntl, termios
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 120, 0, 0))
    return pid, fd

def drain(fd, seconds, until=None):
    """Reads the terminal for `seconds`, or until `until` appears; returns what was read."""
    data, end = b"", time.time() + seconds
    while time.time() < end:
        ready, _, _ = select.select([fd], [], [], 0.05)
        if ready:
            try:
                data += os.read(fd, 65536)
            except OSError:
                break
            if until and until in data:
                break
    return data

def stop(pid):
    os.kill(pid, signal.SIGKILL)
    os.waitpid(pid, 0)

# 2b. Interactive TUI: from exec to the first frame with the banner on screen.
frames = []
for _ in range(10):
    start = time.perf_counter()
    pid, fd = spawn()
    drain(fd, 5, until=b"Corex")
    frames.append((time.perf_counter() - start) * 1000)
    stop(pid)
print("==> 3. Interactive TUI, time until the banner is drawn (10 runs)")
print(f"    {ms(frames)}")
print()

# 2c. Interactive TUI left idle.
def proc_status(pid):
    fields = {}
    with open(f"/proc/{pid}/status") as status:
        for line in status:
            key, _, value = line.partition(":")
            fields[key] = value.strip()
    return fields

def kb(field):
    return int(field.split()[0])

def describe(status):
    """`11676 kB (anonymous 1872 kB, mapped files and shared 9804 kB)`; anonymous is the heap and stacks."""
    total, anon = kb(status["VmRSS"]), kb(status["RssAnon"])
    return f"{total} kB (anonymous {anon} kB, mapped files and shared {total - anon} kB)"

def cpu_seconds(pid):
    with open(f"/proc/{pid}/stat") as stat:
        parts = stat.read().rsplit(")", 1)[1].split()
    return (int(parts[11]) + int(parts[12])) / os.sysconf("SC_CLK_TCK")

print("==> 4. Interactive TUI left idle (no API key, no input)")
pid, fd = spawn()
drain(fd, 4)
if os.path.isdir(f"/proc/{pid}"):
    status = proc_status(pid)
    print(f"    Resident memory after 4 s:  {describe(status)}")
    start_cpu, start_time = cpu_seconds(pid), time.time()
    drain(fd, 10)
    cpu = (cpu_seconds(pid) - start_cpu) / (time.time() - start_time) * 100
    status = proc_status(pid)
    print(f"    Resident memory after 14 s: {describe(status)}, peak {kb(status['VmHWM'])} kB")
    print(f"    CPU over the last 10 s: {cpu:.2f}%")
else:
    out = subprocess.run(["ps", "-o", "rss=", "-p", str(pid)], capture_output=True, text=True).stdout.strip()
    print(f"    Resident memory after 4 s: {out} kB (CPU not measured without /proc)")
stop(pid)
PY

echo ""
echo "Not measured here: memory while streaming a model response (needs a live provider),"
echo "prompt-cache hit rates (they depend on the provider and the conversation)."
