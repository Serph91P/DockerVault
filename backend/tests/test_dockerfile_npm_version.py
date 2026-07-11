import json
import re
from pathlib import Path


def test_dockerfile_npm_version_matches_package_manager():
    repository = Path(__file__).resolve().parents[2]
    dockerfile = (repository / "Dockerfile").read_text()
    package = json.loads((repository / "frontend/package.json").read_text())

    npm_install = re.search(
        r"^RUN npm install --global (npm@\d+\.\d+\.\d+)(?:\s|$)",
        dockerfile,
        re.MULTILINE,
    )

    assert npm_install is not None, "Dockerfile must install a literal npm@version"
    assert npm_install.group(1) == package["packageManager"]
