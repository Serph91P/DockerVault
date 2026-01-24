"""
Docker client wrapper for safe container and volume management.
"""

import asyncio
from typing import Optional, List, Dict, Any
from dataclasses import dataclass
import docker
from docker.models.containers import Container
from docker.models.volumes import Volume
import logging

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
    
    def __post_init__(self):
        if self.depends_on is None:
            self.depends_on = []


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
            None, 
            lambda: self.client.containers.list(all=all)
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
                mounts.append({
                    "type": mount.get("Type"),
                    "source": mount.get("Source"),
                    "destination": mount.get("Destination"),
                    "name": mount.get("Name"),
                    "mode": mount.get("Mode"),
                    "rw": mount.get("RW"),
                })
            
            # Extract networks
            networks = list(attrs.get("NetworkSettings", {}).get("Networks", {}).keys())
            
            # Extract depends_on from labels (if using our custom labels)
            depends_on = labels.get("backup.depends_on", "").split(",")
            depends_on = [d.strip() for d in depends_on if d.strip()]
            
            result.append(ContainerInfo(
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
            ))
        
        return result
    
    async def list_volumes(self) -> List[VolumeInfo]:
        """List all volumes with usage information."""
        loop = asyncio.get_event_loop()
        volumes = await loop.run_in_executor(
            None,
            lambda: self.client.volumes.list()
        )
        
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
            result.append(VolumeInfo(
                name=volume.name,
                driver=attrs.get("Driver", "local"),
                mountpoint=attrs.get("Mountpoint", ""),
                labels=attrs.get("Labels", {}) or {},
                created_at=attrs.get("CreatedAt", ""),
                used_by=volume_usage.get(volume.name, []),
            ))
        
        return result
    
    async def get_stacks(self) -> List[StackInfo]:
        """Get Docker Compose stacks."""
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
            
            result.append(StackInfo(
                name=stack_name,
                containers=stack_containers,
                volumes=list(volumes),
                networks=list(networks),
            ))
        
        return result
    
    async def stop_container(self, container_id_or_name: str, timeout: int = 30) -> bool:
        """Stop a container safely."""
        try:
            loop = asyncio.get_event_loop()
            container = await loop.run_in_executor(
                None,
                lambda: self.client.containers.get(container_id_or_name)
            )
            await loop.run_in_executor(
                None,
                lambda: container.stop(timeout=timeout)
            )
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
                None,
                lambda: self.client.containers.get(container_id_or_name)
            )
            await loop.run_in_executor(
                None,
                container.start
            )
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
                None,
                lambda: self.client.containers.get(container_id_or_name)
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
                None,
                lambda: self.client.volumes.get(volume_name)
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
    
    def close(self):
        """Close Docker client."""
        if self._client:
            self._client.close()
            self._client = None


# Global Docker client instance
docker_client = DockerClientWrapper()
