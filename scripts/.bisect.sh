set -e
cd /mnt/c/work/play/fontconf
export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_TARGET_DIR=/tmp/fctbisect
cargo build -q --release --example bench 2>/dev/null || { echo "BUILD FAILED"; exit 1; }
python3 - <<PY
import subprocess,time
best=1e9
for _ in range(4):
    t=time.time(); subprocess.run(["/tmp/fctbisect/release/examples/bench","prepare","3000"],stdout=subprocess.DEVNULL); best=min(best,time.time()-t)
print(f"prepare x3000: {best*1000:.0f} ms")
PY
