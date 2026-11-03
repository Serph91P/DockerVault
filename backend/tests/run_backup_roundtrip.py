"""Run real app roundtrips in one uniquely owned disposable image container."""

from pathlib import Path
import subprocess
import sys
import uuid


def run(image):
    name = f"dockervault-roundtrip-{uuid.uuid4().hex[:12]}"
    probe = Path(__file__).with_name("runtime_backup_roundtrip.py")
    started = False
    try:
        subprocess.run(
            [
                "docker",
                "run",
                "--rm",
                "-d",
                "--name",
                name,
                "--network",
                "none",
                "--label",
                "dockervault.fixture=contract-roundtrip",
                "-e",
                "DOCKERVAULT_ROUNDTRIP_OWNED=1",
                image,
            ],
            check=True,
            stdout=subprocess.DEVNULL,
        )
        started = True
        subprocess.run(
            ["docker", "cp", str(probe), f"{name}:/app/owned-roundtrip-probe.py"],
            check=True,
        )
        subprocess.run(
            ["docker", "exec", name, "python", "/app/owned-roundtrip-probe.py"],
            check=True,
            timeout=360,
        )
    except BaseException:
        if started:
            subprocess.run(["docker", "logs", "--tail", "120", name], check=False)
        raise
    finally:
        if started:
            # --rm also removes image-declared anonymous volumes; no shared resources.
            subprocess.run(
                ["docker", "stop", "--time", "15", name],
                check=True,
                stdout=subprocess.DEVNULL,
            )


if __name__ == "__main__":
    run(sys.argv[1] if len(sys.argv) > 1 else "dockervault:test")
