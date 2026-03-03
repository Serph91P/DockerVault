"""
Tests for docker_client module.
"""

from unittest.mock import MagicMock, patch

from app.docker_client import ContainerInfo, DockerClientWrapper, VolumeInfo


class TestDockerClientWrapper:
    """Test Docker client wrapper functionality."""

    def test_init_creates_wrapper(self):
        """Test Docker client wrapper initialization."""
        wrapper = DockerClientWrapper()
        assert wrapper is not None
        assert wrapper._client is None  # Lazy initialization

    @patch("app.docker_client.docker.DockerClient")
    def test_client_property_creates_client(self, mock_docker_client):
        """Test that client property creates Docker client lazily."""
        mock_client = MagicMock()
        mock_docker_client.return_value = mock_client

        wrapper = DockerClientWrapper()
        # Access client property to trigger creation
        client = wrapper.client

        assert client is mock_client
        mock_docker_client.assert_called_once()

    @patch("app.docker_client.docker.DockerClient")
    async def test_ping_success(self, mock_docker_client):
        """Test successful Docker ping."""
        mock_client = MagicMock()
        mock_client.ping.return_value = True
        mock_docker_client.return_value = mock_client

        wrapper = DockerClientWrapper()
        result = await wrapper.ping()

        assert result is True

    @patch("app.docker_client.docker.DockerClient")
    async def test_ping_failure(self, mock_docker_client):
        """Test Docker ping failure."""
        mock_client = MagicMock()
        mock_client.ping.side_effect = Exception("Connection refused")
        mock_docker_client.return_value = mock_client

        wrapper = DockerClientWrapper()
        result = await wrapper.ping()

        assert result is False

    @patch("app.docker_client.docker.DockerClient")
    async def test_list_containers_success(self, mock_docker_client):
        """Test successful container listing."""
        mock_client = MagicMock()
        mock_docker_client.return_value = mock_client

        # Mock container data
        mock_container = MagicMock()
        mock_container.id = "container123"
        mock_container.name = "test-container"
        mock_container.status = "running"
        mock_container.attrs = {
            "State": {"Status": "running"},
            "Created": "2024-01-01T00:00:00Z",
            "Config": {
                "Image": "nginx:latest",
                "Labels": {"com.docker.compose.project": "myproject"},
            },
            "Mounts": [],
            "NetworkSettings": {"Networks": {"bridge": {}}},
        }
        mock_client.containers.list.return_value = [mock_container]

        wrapper = DockerClientWrapper()
        containers = await wrapper.list_containers()

        assert len(containers) == 1
        container = containers[0]
        assert isinstance(container, ContainerInfo)
        assert container.id == "container123"
        assert container.name == "test-container"
        assert container.status == "running"
        assert container.compose_project == "myproject"

    @patch("app.docker_client.docker.DockerClient")
    async def test_list_containers_empty(self, mock_docker_client):
        """Test container listing when no containers exist."""
        mock_client = MagicMock()
        mock_docker_client.return_value = mock_client
        mock_client.containers.list.return_value = []

        wrapper = DockerClientWrapper()
        containers = await wrapper.list_containers()

        assert containers == []

    @patch("app.docker_client.docker.DockerClient")
    async def test_list_volumes_success(self, mock_docker_client):
        """Test successful volume listing."""
        mock_client = MagicMock()
        mock_docker_client.return_value = mock_client

        # Mock volume data
        mock_volume = MagicMock()
        mock_volume.name = "test-volume"
        mock_volume.attrs = {
            "Driver": "local",
            "Mountpoint": "/var/lib/docker/volumes/test-volume/_data",
            "Labels": {},
            "CreatedAt": "2024-01-01T00:00:00Z",
        }
        mock_client.volumes.list.return_value = [mock_volume]
        mock_client.containers.list.return_value = []

        wrapper = DockerClientWrapper()
        volumes = await wrapper.list_volumes()

        assert len(volumes) == 1
        volume = volumes[0]
        assert isinstance(volume, VolumeInfo)
        assert volume.name == "test-volume"
        assert volume.driver == "local"

    @patch("app.docker_client.docker.DockerClient")
    async def test_list_volumes_with_usage(self, mock_docker_client):
        """Test volume listing with container usage info."""
        mock_client = MagicMock()
        mock_docker_client.return_value = mock_client

        # Mock volume data
        mock_volume = MagicMock()
        mock_volume.name = "test-volume"
        mock_volume.attrs = {
            "Driver": "local",
            "Mountpoint": "/var/lib/docker/volumes/test-volume/_data",
            "Labels": {},
            "CreatedAt": "2024-01-01T00:00:00Z",
        }
        mock_client.volumes.list.return_value = [mock_volume]

        # Mock container using the volume
        mock_container = MagicMock()
        mock_container.id = "container123"
        mock_container.name = "test-container"
        mock_container.status = "running"
        mock_container.attrs = {
            "State": {"Status": "running"},
            "Created": "2024-01-01T00:00:00Z",
            "Config": {"Image": "nginx:latest", "Labels": {}},
            "Mounts": [
                {
                    "Type": "volume",
                    "Name": "test-volume",
                    "Source": "/var/lib/docker/volumes/test-volume/_data",
                    "Destination": "/data",
                }
            ],
            "NetworkSettings": {"Networks": {}},
        }
        mock_client.containers.list.return_value = [mock_container]

        wrapper = DockerClientWrapper()
        volumes = await wrapper.list_volumes()

        assert len(volumes) == 1
        volume = volumes[0]
        assert "test-container" in volume.used_by

    @patch("app.docker_client.docker.DockerClient")
    async def test_stop_container_success(self, mock_docker_client):
        """Test successful container stopping."""
        mock_client = MagicMock()
        mock_docker_client.return_value = mock_client

        mock_container = MagicMock()
        mock_container.status = "running"
        mock_client.containers.get.return_value = mock_container

        wrapper = DockerClientWrapper()
        success = await wrapper.stop_container("container1")

        assert success is True
        mock_client.containers.get.assert_called_with("container1")

    @patch("app.docker_client.docker.DockerClient")
    async def test_stop_container_failure(self, mock_docker_client):
        """Test container stop failure."""
        mock_client = MagicMock()
        mock_docker_client.return_value = mock_client
        mock_client.containers.get.side_effect = Exception("Container not found")

        wrapper = DockerClientWrapper()
        success = await wrapper.stop_container("nonexistent")

        assert success is False

    @patch("app.docker_client.docker.DockerClient")
    async def test_start_container_success(self, mock_docker_client):
        """Test successful container starting."""
        mock_client = MagicMock()
        mock_docker_client.return_value = mock_client

        mock_container = MagicMock()
        mock_container.status = "exited"
        mock_client.containers.get.return_value = mock_container

        wrapper = DockerClientWrapper()
        success = await wrapper.start_container("container1")

        assert success is True

    @patch("app.docker_client.docker.DockerClient")
    async def test_start_container_failure(self, mock_docker_client):
        """Test container start failure."""
        mock_client = MagicMock()
        mock_docker_client.return_value = mock_client
        mock_client.containers.get.side_effect = Exception("Container not found")

        wrapper = DockerClientWrapper()
        success = await wrapper.start_container("nonexistent")

        assert success is False

    @patch("app.docker_client.docker.DockerClient")
    async def test_get_container_state(self, mock_docker_client):
        """Test getting container state."""
        mock_client = MagicMock()
        mock_docker_client.return_value = mock_client

        mock_container = MagicMock()
        mock_container.status = "running"
        mock_client.containers.get.return_value = mock_container

        wrapper = DockerClientWrapper()
        state = await wrapper.get_container_state("container1")

        assert state == "running"

    @patch("app.docker_client.docker.DockerClient")
    async def test_get_container_state_not_found(self, mock_docker_client):
        """Test getting state of non-existent container."""
        mock_client = MagicMock()
        mock_docker_client.return_value = mock_client
        mock_client.containers.get.side_effect = Exception("Container not found")

        wrapper = DockerClientWrapper()
        state = await wrapper.get_container_state("nonexistent")

        assert state is None

    @patch("app.docker_client.docker.DockerClient")
    async def test_get_stacks(self, mock_docker_client):
        """Test getting Docker Compose stacks."""
        mock_client = MagicMock()
        mock_docker_client.return_value = mock_client

        # Mock container in a compose project
        mock_container = MagicMock()
        mock_container.id = "container123"
        mock_container.name = "myproject-web-1"
        mock_container.status = "running"
        mock_container.attrs = {
            "State": {"Status": "running"},
            "Created": "2024-01-01T00:00:00Z",
            "Config": {
                "Image": "nginx:latest",
                "Labels": {
                    "com.docker.compose.project": "myproject",
                    "com.docker.compose.service": "web",
                },
            },
            "Mounts": [],
            "NetworkSettings": {"Networks": {"myproject_default": {}}},
        }
        mock_client.containers.list.return_value = [mock_container]

        wrapper = DockerClientWrapper()
        stacks = await wrapper.get_stacks()

        assert len(stacks) == 1
        stack = stacks[0]
        assert stack.name == "myproject"
        assert len(stack.containers) == 1

    @patch("app.docker_client.docker.DockerClient")
    async def test_get_dependency_order(self, mock_docker_client):
        """Test getting container dependency order."""
        mock_client = MagicMock()
        mock_docker_client.return_value = mock_client

        # Mock containers with dependencies
        mock_db = MagicMock()
        mock_db.id = "db123"
        mock_db.name = "db"
        mock_db.status = "running"
        mock_db.attrs = {
            "State": {"Status": "running"},
            "Created": "2024-01-01T00:00:00Z",
            "Config": {"Image": "postgres:14", "Labels": {}},
            "Mounts": [],
            "NetworkSettings": {"Networks": {}},
        }

        mock_app = MagicMock()
        mock_app.id = "app123"
        mock_app.name = "app"
        mock_app.status = "running"
        mock_app.attrs = {
            "State": {"Status": "running"},
            "Created": "2024-01-01T00:00:00Z",
            "Config": {"Image": "myapp:latest", "Labels": {"backup.depends_on": "db"}},
            "Mounts": [],
            "NetworkSettings": {"Networks": {}},
        }

        mock_client.containers.list.return_value = [mock_db, mock_app]

        wrapper = DockerClientWrapper()
        order = await wrapper.get_dependency_order(["app", "db"])

        assert isinstance(order, list)
        # App should come before db for stopping (since app depends on db)
        assert "app" in order
        assert "db" in order

    def test_close(self):
        """Test closing Docker client."""
        wrapper = DockerClientWrapper()
        wrapper._client = MagicMock()

        wrapper.close()

        assert wrapper._client is None


class TestContainerInfo:
    """Test ContainerInfo dataclass."""

    def test_container_info_creation(self):
        """Test creating ContainerInfo."""
        info = ContainerInfo(
            id="test123",
            name="test-container",
            image="nginx:latest",
            status="running",
            state="running",
            created="2024-01-01T00:00:00Z",
            labels={"key": "value"},
            mounts=[],
            networks=["bridge"],
            compose_project="myproject",
            compose_service="web",
        )

        assert info.id == "test123"
        assert info.name == "test-container"
        assert info.compose_project == "myproject"
        assert info.depends_on == []  # Default empty list

    def test_container_info_with_depends_on(self):
        """Test ContainerInfo with dependencies."""
        info = ContainerInfo(
            id="test123",
            name="test-container",
            image="nginx:latest",
            status="running",
            state="running",
            created="2024-01-01T00:00:00Z",
            labels={},
            mounts=[],
            networks=[],
            depends_on=["db", "redis"],
        )

        assert info.depends_on == ["db", "redis"]


class TestVolumeInfo:
    """Test VolumeInfo dataclass."""

    def test_volume_info_creation(self):
        """Test creating VolumeInfo."""
        info = VolumeInfo(
            name="test-volume",
            driver="local",
            mountpoint="/var/lib/docker/volumes/test-volume/_data",
            labels={"backup": "true"},
            created_at="2024-01-01T00:00:00Z",
            used_by=["container1", "container2"],
        )

        assert info.name == "test-volume"
        assert info.driver == "local"
        assert len(info.used_by) == 2
