"""
Docker API endpoints.
"""

import logging
from typing import List, Optional

from fastapi import APIRouter, HTTPException, Query, Request
from pydantic import BaseModel

from app.docker_client import docker_client
from app.rate_limit import limiter

logger = logging.getLogger(__name__)

router = APIRouter()


class ContainerResponse(BaseModel):
    """Container response model."""

    id: str
    name: str
    image: str
    status: str
    state: str
    created: str
    labels: dict
    mounts: List[dict]
    networks: List[str]
    compose_project: Optional[str] = None
    compose_service: Optional[str] = None
    depends_on: List[str] = []

    class Config:
        from_attributes = True


class VolumeResponse(BaseModel):
    """Volume response model."""

    name: str
    driver: str
    mountpoint: str
    labels: dict
    created_at: str
    used_by: List[str]

    class Config:
        from_attributes = True


class StackResponse(BaseModel):
    """Stack response model."""

    name: str
    containers: List[ContainerResponse]
    volumes: List[str]
    networks: List[str]
    stop_order: List[str] = []
    start_order: List[str] = []


class StackDependencyResponse(BaseModel):
    """Stack dependency analysis response."""

    stack_name: str
    containers: List[str]
    stop_order: List[str]
    start_order: List[str]
    dependencies: dict  # service -> [depends_on services]


@router.get("/health")
async def docker_health():
    """Check Docker connection health."""
    is_healthy = await docker_client.ping()
    if not is_healthy:
        raise HTTPException(status_code=503, detail="Docker is not accessible")
    return {"status": "healthy"}


@router.get("/containers", response_model=List[ContainerResponse])
async def list_containers(all: bool = True):
    """List all Docker containers."""
    containers = await docker_client.list_containers(all=all)
    return [
        ContainerResponse(
            id=c.id,
            name=c.name,
            image=c.image,
            status=c.status,
            state=c.state,
            created=c.created,
            labels=c.labels,
            mounts=c.mounts,
            networks=c.networks,
            compose_project=c.compose_project,
            compose_service=c.compose_service,
            depends_on=c.depends_on,
        )
        for c in containers
    ]


@router.get("/containers/{container_id}")
async def get_container(container_id: str):
    """Get a specific container."""
    containers = await docker_client.list_containers()
    container = next(
        (c for c in containers if c.id == container_id or c.name == container_id), None
    )
    if not container:
        raise HTTPException(status_code=404, detail="Container not found")
    return ContainerResponse(
        id=container.id,
        name=container.name,
        image=container.image,
        status=container.status,
        state=container.state,
        created=container.created,
        labels=container.labels,
        mounts=container.mounts,
        networks=container.networks,
        compose_project=container.compose_project,
        compose_service=container.compose_service,
        depends_on=container.depends_on,
    )


@router.post("/containers/{container_id}/stop")
@limiter.limit("10/minute")
async def stop_container(
    request: Request,
    container_id: str,
    timeout: int = Query(default=30, ge=1, le=300),
):
    """Stop a container."""
    logger.info("Stopping container %s (timeout=%ds)", container_id, timeout)
    success = await docker_client.stop_container(container_id, timeout=timeout)
    if not success:
        raise HTTPException(status_code=500, detail="Failed to stop container")
    return {"status": "stopped"}


@router.post("/containers/{container_id}/start")
@limiter.limit("10/minute")
async def start_container(request: Request, container_id: str):
    """Start a container."""
    logger.info("Starting container %s", container_id)
    success = await docker_client.start_container(container_id)
    if not success:
        raise HTTPException(status_code=500, detail="Failed to start container")
    return {"status": "started"}


@router.get("/volumes", response_model=List[VolumeResponse])
async def list_volumes():
    """List all Docker volumes."""
    volumes = await docker_client.list_volumes()
    return [
        VolumeResponse(
            name=v.name,
            driver=v.driver,
            mountpoint=v.mountpoint,
            labels=v.labels,
            created_at=v.created_at,
            used_by=v.used_by,
        )
        for v in volumes
    ]


@router.get("/stacks", response_model=List[StackResponse])
async def list_stacks():
    """List all Docker Compose stacks."""
    stacks = await docker_client.get_stacks()
    return [
        StackResponse(
            name=s.name,
            containers=[
                ContainerResponse(
                    id=c.id,
                    name=c.name,
                    image=c.image,
                    status=c.status,
                    state=c.state,
                    created=c.created,
                    labels=c.labels,
                    mounts=c.mounts,
                    networks=c.networks,
                    compose_project=c.compose_project,
                    compose_service=c.compose_service,
                    depends_on=c.depends_on,
                )
                for c in s.containers
            ],
            volumes=s.volumes,
            networks=s.networks,
            stop_order=s.stop_order,
            start_order=s.start_order,
        )
        for s in stacks
    ]


@router.get("/stacks/{stack_name}/dependencies", response_model=StackDependencyResponse)
async def get_stack_dependencies(stack_name: str):
    """Get dependency analysis for a specific stack."""
    stacks = await docker_client.get_stacks()
    stack = next((s for s in stacks if s.name == stack_name), None)

    if not stack:
        raise HTTPException(status_code=404, detail=f"Stack '{stack_name}' not found")

    # Build dependencies dict: container_name -> [depends_on]
    dependencies = {}
    for container in stack.containers:
        deps = list(set(container.compose_depends_on + container.depends_on))
        if deps:
            dependencies[container.name] = deps

    return StackDependencyResponse(
        stack_name=stack_name,
        containers=[c.name for c in stack.containers],
        stop_order=stack.stop_order,
        start_order=stack.start_order,
        dependencies=dependencies,
    )
