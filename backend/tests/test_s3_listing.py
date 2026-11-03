"""Native aioboto3/HTTP inventory pagination, identity and truthful API failures."""

from collections import Counter
from unittest.mock import patch

from botocore.exceptions import ClientError
import pytest

from app.database import RemoteStorage
from app.remote_storage import storage_manager
from tests import test_s3_protocol
from tests.test_remote_object_ownership import test_engine  # noqa: F401

s3_service = test_s3_protocol.s3_service


def listing(backend, state, *, failing_page=None):
    backend.config.base_path = "vault"
    token = "owned+continuation/%one"
    keys = ["vault/reports/first.tar", "vault/reports/nested/second.tar"]
    state.listing_pages = {
        None: {"keys": keys[:1], "next": token},
        token: {"keys": keys[1:]},
    }
    if failing_page is not None:
        state.listing_pages[None if failing_page == 0 else token] = {"error": True}
    return token


async def list_through_api(backend, async_client, test_db):
    async with test_db() as session:
        record = RemoteStorage(
            name="owned-listing",
            storage_type="s3",
            base_path="vault",
            s3_bucket="owned-bucket",
            s3_endpoint_url=backend.config.s3_endpoint_url,
        )
        session.add(record)
        await session.commit()
        storage_id = record.id
    assert storage_id == backend.config.id
    with (
        patch.dict(storage_manager.backends, {storage_id: backend}, clear=True),
        patch.dict(storage_manager.configs, {storage_id: backend.config}, clear=True),
    ):
        return await async_client.get(
            f"/api/v1/storage/{storage_id}/files", params={"path": "reports"}
        )


@pytest.mark.asyncio
@pytest.mark.parametrize("caller", ["provider", "api"])
async def test_s3_paginator_returns_over_1000_distinct_relative_keys(
    s3_service, caller, async_client, test_db
):
    backend, state = s3_service
    token = listing(backend, state)
    names = [f"file-{i:04}.tar" for i in range(1001)] + ["a/same.tar", "b/same.tar"]
    keys = ["vault/reports/" + name for name in names]
    state.listing_pages = {
        None: {"keys": keys[:1000], "next": token},
        token: {"keys": keys[1000:]},
    }
    if caller == "provider":
        rows = await backend.list_files("reports")
    else:
        response = await list_through_api(backend, async_client, test_db)
        assert response.status_code == 200
        assert (
            set(response.json()) == {"files", "path"}
            and response.json()["path"] == "reports"
        )
        rows = response.json()["files"]
    assert [row["name"] for row in rows] == names
    assert Counter(row["name"] for row in rows) == Counter(names)
    assert all(set(row) == {"name", "size", "is_dir", "modified"} for row in rows)
    assert all(
        row["size"] == 17 and not row["is_dir"] and row["modified"] > 0 for row in rows
    )
    assert state.listing_requests == [
        ("vault/reports/", None),
        ("vault/reports/", token),
    ]


@pytest.mark.asyncio
async def test_s3_listing_handles_empty_final_page_without_losing_inventory(s3_service):
    backend, state = s3_service
    token = listing(backend, state)
    state.listing_pages[token] = {"keys": []}
    assert [row["name"] for row in await backend.list_files("reports")] == ["first.tar"]
    assert state.listing_requests == [
        ("vault/reports/", None),
        ("vault/reports/", token),
    ]


@pytest.mark.asyncio
async def test_truncated_page_without_token_is_not_complete_inventory(s3_service):
    backend, state = s3_service
    listing(backend, state)
    state.listing_pages[None] = {"keys": ["vault/reports/first.tar"], "truncated": True}
    with pytest.raises(RuntimeError, match="continuation"):
        await backend.list_files("reports")


@pytest.mark.asyncio
@pytest.mark.parametrize("failed_page", [0, 1])
async def test_s3_listing_error_does_not_return_empty_or_partial_success(
    s3_service, failed_page
):
    backend, state = s3_service
    token = listing(backend, state, failing_page=failed_page)
    with pytest.raises(ClientError, match="AccessDenied"):
        await backend.list_files("reports")
    assert [request[1] for request in state.listing_requests] == [None, token][
        : failed_page + 1
    ]


@pytest.mark.asyncio
@pytest.mark.parametrize("failed_page", [0, 1])
async def test_listing_api_reports_provider_error_instead_of_successful_empty_inventory(
    s3_service, failed_page, async_client, test_db
):
    backend, state = s3_service
    listing(backend, state, failing_page=failed_page)
    response = await list_through_api(backend, async_client, test_db)
    assert response.status_code == 502
    assert response.json() == {"detail": "Remote listing failed"}
