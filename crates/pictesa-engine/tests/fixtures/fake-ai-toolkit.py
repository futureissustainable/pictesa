#!/usr/bin/env python3
"""Contract fixture: accepts ai-toolkit's config path, emits tqdm CRs, saves final weights."""
import json
import os
from pathlib import Path
import sys
import subprocess
import time

config = json.loads(Path(sys.argv[1]).read_text())["config"]
process = config["process"][0]
root = Path(process["training_folder"]).parent
(root / "pid").write_text(str(os.getpid()))
worker = None
if process["trigger_word"] == "cancel_me":
    worker = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(60)"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    (root / "worker-pid").write_text(str(worker.pid))
images = Path(process["datasets"][0]["folder_path"])
assert len(list(images.glob("*.png"))) >= 3
assert all(p.read_text() == process["trigger_word"] for p in images.glob("*.txt"))
for fraction in (0.1, 0.3, 0.6, 0.9):
    done = int(process["train"]["steps"] * fraction)
    print(f'{config["name"]}: {int(fraction * 100)}%|### | {done}/{process["train"]["steps"]} [00:01<00:03, 1.0it/s]', end="\r", flush=True)
    time.sleep(2 if process["trigger_word"] == "cancel_me" else 0.7)
output = Path(process["training_folder"]) / config["name"]
output.mkdir(parents=True)
header = b'{"__metadata__":{"fixture":"not real weights"}}'
(output / f'{config["name"]}.safetensors').write_bytes(len(header).to_bytes(8, "little") + header)
if worker:
    worker.terminate()
    worker.wait()
print("100%|##########| 1000/1000 [00:03<00:00, 1.0it/s]", flush=True)
