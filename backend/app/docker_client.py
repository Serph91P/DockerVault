"""
Docker client wrapper for safe container and volume management.
"""

import asyncio
import logging
from dataclasses import dataclass
from typing import Any, Dict, List, Optional

import docker

from app.config import settings

logger = logging.getLogger(__name__)


@dataclass
class ContainerInfo:
    """Container information."""

    id: str
    name: str
    image: str
    status: str
    state: str
    created: str
    labels: Dict[str, str]
    mounts: List[Dict[str, Any]]
    networks: List[str]
    compose_project: Optional[str] = None
    compose_service: Optional[str] = None
    depends_on: List[str] = None
    compose_depends_on: List[str] = None  # Dependencies from compose config

    def __post_init__(self):
        if self.depends_on is None:
            self.depends_on = []
        if self.compose_depends_on is None:
            self.compose_depends_on = []


@dataclass
class VolumeInfo:
    """Volume information."""

    name: str
    driver: str
    mountpoint: str
    labels: Dict[str, str]
    created_at: str
    used_by: List[str]  # Container names using this volume


@dataclass
class StackInfo:
    """Docker Compose stack information."""

    name: str
    containers: List[ContainerInfo]
    volumes: List[str]
    networks: List[str]
    stop_order: List[str] = None  # Container names in order to stop
    start_order: List[str] = None  # Container names in order to start

    def __post_init__(self):
        if self.stop_order is None:
            self.stop_order = []
        if self.start_order is None:
            self.start_order = []


class DockerClientWrapper:
    """Wrapper for Docker SDK with async support."""

    def __init__(self):
        self._client: Optional[docker.DockerClient] = None

    @property
    def client(self) -> docker.DockerClient:
        """Get or create Docker client."""
        if self._client is None:
            self._client = docker.DockerClient(
                base_url=f"unix://{settings.DOCKER_SOCKET}"
            )
        return self._client

    async def ping(self) -> bool:
        """Check if Docker is accessible."""
        try:
            loop = asyncio.get_event_loop()
            await loop.run_in_executor(None, self.client.ping)
            return True
        except Exception as e:
            logger.error(f"Docker ping failed: {e}")
            return False

    async def list_containers(self, all: bool = True) -> List[ContainerInfo]:
        """List all containers with details."""
        loop = asyncio.get_event_loop()
        containers = await loop.run_in_executor(
            None, lambda: self.client.containers.list(all=all)
        )

        result = []
        for container in containers:
            attrs = container.attrs
            labels = attrs.get("Config", {}).get("Labels", {})

            # Extract compose information
            compose_project = labels.get("com.docker.compose.project")
            compose_service = labels.get("com.docker.compose.service")

            # Extract mounts
            mounts = []
            for mount in attrs.get("Mounts", []):
                mounts.append(
                    {
                        "type": mount.get("Type"),
                        "source": mount.get("Source"),
                        "destination": mount.get("Destination"),
                        "name": mount.get("Name"),
                        "mode": mount.get("Mode"),
                        "rw": mount.get("RW"),
                    }
                )

            # Extract networks
            networks = list(attrs.get("NetworkSettings", {}).get("Networks", {}).keys())

            # Extract depends_on from labels (if using our custom labels)
            depends_on = labels.get("backup.depends_on", "").split(",")
            depends_on = [d.strip() for d in depends_on if d.strip()]

            # Try to extract depends_on from compose config (Docker stores this)
            compose_depends_on = []
            config_labels = labels.get("com.docker.compose.depends_on", "")
            if config_labels:
                # Format: "service1:condition,service2:condition"
                for dep in config_labels.split(","):
                    if ":" in dep:
                        service_name = dep.split(":")[0].strip()
                        if service_name:
                            compose_depends_on.append(service_name)
                    elif dep.strip():
                        compose_depends_on.append(dep.strip())

            result.append(
                ContainerInfo(
                    id=container.id,
                    name=container.name,
                    image=attrs.get("Config", {}).get("Image", ""),
                    status=container.status,
                    state=attrs.get("State", {}).get("Status", ""),
                    created=attrs.get("Created", ""),
                    labels=labels,
                    mounts=mounts,
                    networks=networks,
                    compose_project=compose_project,
                    compose_service=compose_service,
                    depends_on=depends_on,
                    compose_depends_on=compose_depends_on,
                )
            )

        return result

    async def list_volumes(self) -> List[VolumeInfo]:
        """List all volumes with usage information."""
        loop = asyncio.get_event_loop()
        volumes = await loop.run_in_executor(None, lambda: self.client.volumes.list())

        # Get container volume usage
        containers = await self.list_containers()
        volume_usage: Dict[str, List[str]] = {}

        for container in containers:
            for mount in container.mounts:
                if mount.get("type") == "volume" and mount.get("name"):
                    volume_name = mount["name"]
                    if volume_name not in volume_usage:
                        volume_usage[volume_name] = []
                    volume_usage[volume_name].append(container.name)

        result = []
        for volume in volumes:
            attrs = volume.attrs
            result.append(
                VolumeInfo(
                    name=volume.name,
                    driver=attrs.get("Driver", "local"),
                    mountpoint=attrs.get("Mountpoint", ""),
                    labels=attrs.get("Labels", {}) or {},
                    created_at=attrs.get("CreatedAt", ""),
                    used_by=volume_usage.get(volume.name, []),
                )
            )

        return result

    async def get_stacks(self) -> List[StackInfo]:
        """Get Docker Compose stacks with dependency order."""
        containers = await self.list_containers()

        # Group by compose project
        stacks: Dict[str, List[ContainerInfo]] = {}
        for container in containers:
            if container.compose_project:
                if container.compose_project not in stacks:
                    stacks[container.compose_project] = []
                stacks[container.compose_project].append(container)

        result = []
        for stack_name, stack_containers in stacks.items():
            # Collect volumes and networks
            volumes = set()
            networks = set()

            for container in stack_containers:
                for mount in container.mounts:
                    if mount.get("type") == "volume" and mount.get("name"):
                        volumes.add(mount["name"])
                networks.update(container.networks)

            # Calculate dependency order
            stop_order, start_order = self._calculate_stack_dependency_order(
                stack_containers
            )

            result.append(
                StackInfo(
                    name=stack_name,
                    containers=stack_containers,
                    volumes=list(volumes),
                    networks=list(networks),
                    stop_order=stop_order,
                    start_order=start_order,
                )
            )

        return result

    async def stop_container(
        self, container_id_or_name: str, timeout: int = 30
    ) -> bool:
        """Stop a container safely."""
        try:
            loop = asyncio.get_event_loop()
            container = await loop.run_in_executor(
                None, lambda: self.client.containers.get(container_id_or_name)
            )
            await loop.run_in_executor(None, lambda: container.stop(timeout=timeout))
            logger.info(f"Stopped container: {container_id_or_name}")
            return True
        except Exception as e:
            logger.error(f"Failed to stop container {container_id_or_name}: {e}")
            return False

    async def start_container(self, container_id_or_name: str) -> bool:
        """Start a container."""
        try:
            loop = asyncio.get_event_loop()
            container = await loop.run_in_executor(
                None, lambda: self.client.containers.get(container_id_or_name)
            )
            await loop.run_in_executor(None, container.start)
            logger.info(f"Started container: {container_id_or_name}")
            return True
        except Exception as e:
            logger.error(f"Failed to start container {container_id_or_name}: {e}")
            return False

    async def get_container_state(self, container_id_or_name: str) -> Optional[str]:
        """Get container state."""
        try:
            loop = asyncio.get_event_loop()
            container = await loop.run_in_executor(
                None, lambda: self.client.containers.get(container_id_or_name)
            )
            return container.status
        except Exception as e:
            logger.error(f"Failed to get container state {container_id_or_name}: {e}")
            return None

    async def get_volume_size(self, volume_name: str) -> Optional[int]:
        """Get approximate volume size in bytes."""
        try:
            loop = asyncio.get_event_loop()
            volume = await loop.run_in_executor(
                None, lambda: self.client.volumes.get(volume_name)
            )
            # Docker doesn't provide volume size directly
            # We'd need to run a container to calculate it
            mountpoint = volume.attrs.get("Mountpoint", "")
            if mountpoint:
                # Return None for now, size calculation done during backup
                return None
            return None
        except Exception as e:
            logger.error(f"Failed to get volume size {volume_name}: {e}")
            return None

    async def get_dependency_order(self, container_names: List[str]) -> List[str]:
        """
        Get containers in dependency order for safe stop/start.
        Returns containers ordered so dependent containers come first (for stopping).
        """
        containers = await self.list_containers()
        container_map = {c.name: c for c in containers}

        # Build dependency graph
        graph: Dict[str, List[str]] = {}
        for name in container_names:
            if name in container_map:
                container = container_map[name]
                graph[name] = container.depends_on

        # Topological sort (Kahn's algorithm)
        in_degree = {name: 0 for name in graph}
        for name, deps in graph.items():
            for dep in deps:
                if dep in in_degree:
                    in_degree[dep] += 1

        queue = [name for name, degree in in_degree.items() if degree == 0]
        result = []

        while queue:
            name = queue.pop(0)
            result.append(name)
            for dep in graph.get(name, []):
                if dep in in_degree:
                    in_degree[dep] -= 1
                    if in_degree[dep] == 0:
                        queue.append(dep)

        # For stopping, we want dependent containers first
        return result

    def _calculate_stack_dependency_order(
        self, containers: List[ContainerInfo]
    ) -> tuple[List[str], List[str]]:
        """
        Calculate stop and start order for stack containers based on dependencies.

        Uses topological sort (Kahn's algorithm) to determine correct order.
        Dependencies come from compose_depends_on (from docker-compose.yml)
        and depends_on (from custom labels).

        Returns:
            tuple: (stop_order, start_order) - container names in order
        """
        # Build service name -> container name mapping
        service_to_container: Dict[str, str] = {}
        container_to_service: Dict[str, str] = {}
        for c in containers:
            if c.compose_service:
                service_to_container[c.compose_service] = c.name
                container_to_service[c.name] = c.compose_service

        # Build dependency graph: container_name -> [dependent_container_names]
        # i.e., which containers depend on this one
        dependents: Dict[str, List[str]] = {c.name: [] for c in containers}
        dependencies: Dict[str, List[str]] = {c.name: [] for c in containers}

        for container in containers:
            # Combine compose_depends_on and custom depends_on
            all_deps = set(container.compose_depends_on + container.depends_on)

            for dep_service in all_deps:
                # dep_service could be a service name or container name
                dep_container = service_to_container.get(dep_service, dep_service)

                if dep_container in dependents:
                    # This container depends on dep_container
                    dependents[dep_container].append(container.name)
                    dependencies[container.name].append(dep_container)

        # Calculate start order (dependencies first) using Kahn's algorithm
        in_degree = {name: len(deps) for name, deps in dependencies.items()}
        queue = [name for name, degree in in_degree.items() if degree == 0]
        start_order = []

        while queue:
            name = queue.pop(0)
            start_order.append(name)
            for dependent in dependents.get(name, []):
                in_degree[dependent] -= 1
                if in_degree[dependent] == 0:
                    queue.append(dependent)

        # Handle any remaining containers (cycle or unresolved)
        remaining = [c.name for c in containers if c.name not in start_order]
        start_order.extend(remaining)

        # Stop order is reverse of start order
        stop_order = list(reversed(start_order))

        logger.debug(
            "Calculated dependency order for stack",
            extra={
                "container_count": len(containers),
                "stop_order": stop_order,
                "start_order": start_order,
            },
        )

        return stop_order, start_order

    def close(self):
        """Close Docker client."""
        if self._client:
            self._client.close()
            self._client = None


# Global Docker client instance
docker_client = DockerClientWrapper()
