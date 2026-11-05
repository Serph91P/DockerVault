"""Explicit Docker identities resolve once; discovery is a synthetic test double."""

import tarfile
from types import SimpleNamespace
from unittest.mock import AsyncMock, patch

import pytest

from app.backup_engine import BackupEngine
from app.database import BackupTarget
from app import tar_worker


async def snapshot(
    tmp_path, *, kind="container", mounts=None, selected=None, rules=None
):
    source = tmp_path / "data"
    source.mkdir(exist_ok=True)
    (source / "keep.txt").write_bytes(b"retained fixture bytes")
    (source / "drop.txt").write_bytes(b"excluded fixture bytes")
    container = SimpleNamespace(
        id="claimed",
        name="service",
        mounts=mounts
        or [
            {"type": "volume", "name": "docker-data", "destination": "/var/lib/app"},
            {"type": "volume", "name": "docker-data", "destination": "/second"},
        ],
    )
    client = SimpleNamespace(
        list_volumes=AsyncMock(
            return_value=[SimpleNamespace(name="docker-data", mountpoint=str(source))]
        ),
        list_containers=AsyncMock(return_value=[container]),
        get_container_identity=AsyncMock(return_value="claimed"),
        get_stacks=AsyncMock(
            return_value=[
                SimpleNamespace(name="stack", containers=[container, container])
            ]
        ),
    )
    target = BackupTarget(
        name="fixture",
        target_type=kind,
        container_name="service",
        stack_name="stack",
        selected_volumes=selected or [],
        per_volume_rules=rules or {},
    )
    with patch("app.backup_engine.docker_client", client):
        sources = await BackupEngine()._resolve_archive_sources(target)
    return sources, client


@pytest.mark.asyncio
@pytest.mark.parametrize("kind", ["container", "stack"])
@pytest.mark.parametrize("implementation", ["worker", "fallback"])
@pytest.mark.parametrize("compress", [False, True])
@pytest.mark.parametrize(
    "override",
    [{"exclude_paths": ["drop.txt"]}, {}, {"include_paths": [], "exclude_paths": []}],
)
async def test_explicit_identity_applies_to_every_claimed_root(
    tmp_path, kind, implementation, compress, override
):
    sources, client = await snapshot(
        tmp_path, kind=kind, rules={"volume:docker-data": override}
    )
    # A changed discovery after claiming must never redirect the rules or the bytes.
    client.list_volumes.return_value = []
    archive = tmp_path / ("out.tar.gz" if compress else "out.tar")
    rules = dict(sources.per_volume_rules)
    if implementation == "worker":
        assert (
            tar_worker.create_tar(
                {
                    "dest": str(archive),
                    "sources": sources.paths,
                    "compress": compress,
                    "include_paths": [],
                    "exclude_paths": ["drop.txt"],
                    "per_volume_rules": rules,
                    "allowed_sources": [str(tmp_path / "data")],
                }
            )["status"]
            == "ok"
        )
    else:
        engine = BackupEngine()
        with patch.object(engine, "_sudo_available", return_value=False):
            engine._create_multi_source_tar(
                sources.paths, str(archive), compress, [], ["drop.txt"], rules
            )
    assert archive.read_bytes().startswith(b"\x1f\x8b") is compress
    with tarfile.open(archive) as tar:
        actual = {
            m.name: tar.extractfile(m).read() for m in tar.getmembers() if m.isfile()
        }
    expected = {}
    for root, _path in sources.paths:
        expected[root.lstrip("/") + "/keep.txt"] = b"retained fixture bytes"
        if override == {"include_paths": [], "exclude_paths": []}:
            expected[root.lstrip("/") + "/drop.txt"] = b"excluded fixture bytes"
    assert actual == expected
    client.list_volumes.assert_awaited_once()


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "selected,rules,mounts",
    [
        (["docker-data", "missing"], {}, None),
        ([], {"volume:missing": {"exclude_paths": []}}, None),
        (
            [],
            {
                "volume:docker-data": {"exclude_paths": []},
                "var/lib/app": {"exclude_paths": ["drop.txt"]},
            },
            None,
        ),
        (
            [],
            {},
            [
                {
                    "type": "volume",
                    "name": "docker-data",
                    "destination": "volume:docker-data",
                }
            ],
        ),
        (
            [],
            {},
            [
                {"type": "volume", "name": "docker-data", "destination": "same"},
                {"type": "volume", "name": "other", "destination": "same"},
            ],
        ),
    ],
)
async def test_ambiguous_or_missing_scope_fails_before_lifecycle(
    tmp_path, selected, rules, mounts
):
    with pytest.raises(ValueError):
        await snapshot(tmp_path, selected=selected, rules=rules, mounts=mounts)


@pytest.mark.asyncio
async def test_dormant_known_rule_and_legacy_empty_rule_are_preserved(tmp_path):
    mounts = [
        {"type": "volume", "name": "docker-data", "destination": "data"},
        {"type": "volume", "name": "dormant", "destination": "unused"},
    ]
    sources, _ = await snapshot(
        tmp_path,
        mounts=mounts,
        selected=["docker-data"],
        rules={"volume:dormant": {"exclude_paths": ["x"]}, "data": {}},
    )
    assert dict(sources.per_volume_rules) == {"data": {}}


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "rules,mounts",
    [
        (
            {"volume:docker-data": {"exclude_paths": ["drop.txt"]}, "var/lib/app": {}},
            None,
        ),
        (
            {},
            [
                {"type": "volume", "name": "docker-data", "destination": "/data"},
                {
                    "type": "volume",
                    "name": "docker-data",
                    "destination": "/data/nested",
                },
            ],
        ),
        (
            {},
            [
                {
                    "type": "volume",
                    "name": "docker-data",
                    "destination": "/data/../other",
                }
            ],
        ),
    ],
)
async def test_empty_legacy_conflict_and_overlapping_roots_fail_closed(
    tmp_path, rules, mounts
):
    with pytest.raises(ValueError):
        await snapshot(tmp_path, rules=rules, mounts=mounts)
