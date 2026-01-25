"""
Tests for docker_client module.
"""

from unittest.mock import MagicMock, patch

import pytest
from docker.errors import APIError, DockerException, NotFound

from app.docker_client import ContainerInfo, DockerClientWrapper


@pytest.mark.asyncio
class TestDockerClientWrapper:
    """Test Docker client wrapper functionality."""

    @patch("app.docker_client.docker.from_env")
    def test_init_success(self, mock_docker):
        """Test successful Docker client initialization."""
        mock_client = MagicMock()
        mock_docker.return_value = mock_client
        mock_client.ping.return_value = True

        wrapper = DockerClientWrapper()
        assert wrapper.client is not None
        assert wrapper.available is True
        mock_client.ping.assert_called_once()

    @patch("app.docker_client.docker.from_env")
    def test_init_failure(self, mock_docker):
        """Test Docker client initialization failure."""
        mock_docker.side_effect = DockerException("Docker not available")

        wrapper = DockerClientWrapper()
        assert wrapper.client is None
        assert wrapper.available is False

    @patch("app.docker_client.docker.from_env")
    def test_get_containers_success(self, mock_docker):
        """Test successful container listing."""
        mock_client = MagicMock()
        mock_docker.return_value = mock_client
        mock_client.ping.return_value = True

        # Mock container data
        mock_container = MagicMock()
        mock_container.id = "container123"
        mock_container.name = "test-container"
        mock_container.image.tags = ["nginx:latest"]
        mock_container.status = "running"
        mock_container.attrs = {
            "State": {"Status": "running"},
            "Created": "2024-01-01T00:00:00Z",
            "Config": {"Labels": {"com.docker.compose.project": "myproject"}},
            "Mounts": [],
            "NetworkSettings": {"Networks": {"bridge": {}}},
        }
        mock_client.containers.list.return_value = [mock_container]

        wrapper = DockerClientWrapper()
        containers = wrapper.get_containers()

        assert len(containers) == 1
        container = containers[0]
        assert isinstance(container, ContainerInfo)
        assert container.id == "container123"
        assert container.name == "test-container"
        assert container.image == "nginx:latest"
        assert container.status == "running"
        assert container.compose_project == "myproject"

    @patch("app.docker_client.docker.from_env")
    def test_get_containers_docker_unavailable(self, mock_docker):
        """Test container listing when Docker is unavailable."""
        mock_docker.side_effect = DockerException("Docker not available")

        wrapper = DockerClientWrapper()
        containers = wrapper.get_containers()

        assert containers == []

    @patch("app.docker_client.docker.from_env")
    def test_get_volumes_success(self, mock_docker):
        """Test successful volume listing."""
        mock_client = MagicMock()
        mock_docker.return_value = mock_client
        mock_client.ping.return_value = True

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

        # Mock containers using volume
        mock_container = MagicMock()
        mock_container.name = "container1"
        mock_container.attrs = {
            "Mounts": [
                {
                    "Type": "volume",
                    "Name": "test-volume",
                    "Source": "/var/lib/docker/volumes/test-volume/_data",
                    "Destination": "/data",
                }
            ]
        }
        mock_client.containers.list.return_value = [mock_container]

        wrapper = DockerClientWrapper()
        volumes = wrapper.get_volumes()

        assert len(volumes) == 1
        volume = volumes[0]
        assert volume["name"] == "test-volume"
        assert volume["driver"] == "local"
        assert "container1" in volume["used_by"]

    @patch("app.docker_client.docker.from_env")
    def test_stop_containers_success(self, mock_docker):
        """Test successful container stopping."""
        mock_client = MagicMock()
        mock_docker.return_value = mock_client
        mock_client.ping.return_value = True

        mock_container = MagicMock()
        mock_container.status = "running"
        mock_client.containers.get.return_value = mock_container

        wrapper = DockerClientWrapper()
        success = wrapper.stop_containers(["container1"])

        assert success is True
        mock_client.containers.get.assert_called_with("container1")
        mock_container.stop.assert_called_once()

    @patch("app.docker_client.docker.from_env")
    def test_stop_containers_not_found(self, mock_docker):
        """Test stopping non-existent container."""
        mock_client = MagicMock()
        mock_docker.return_value = mock_client
        mock_client.ping.return_value = True

        mock_client.containers.get.side_effect = NotFound("Container not found")

        wrapper = DockerClientWrapper()
        success = wrapper.stop_containers(["nonexistent"])

        assert success is False

    @patch("app.docker_client.docker.from_env")
    def test_start_containers_success(self, mock_docker):
        """Test successful container starting."""
        mock_client = MagicMock()
        mock_docker.return_value = mock_client
        mock_client.ping.return_value = True

        mock_container = MagicMock()
        mock_container.status = "exited"
        mock_client.containers.get.return_value = mock_container

        wrapper = DockerClientWrapper()
        success = wrapper.start_containers(["container1"])

        assert success is True
        mock_container.start.assert_called_once()

    @patch("app.docker_client.docker.from_env")
    def test_create_backup_container(self, mock_docker):
        """Test backup container creation."""
        mock_client = MagicMock()
        mock_docker.return_value = mock_client
        mock_client.ping.return_value = True

        mock_container = MagicMock()
        mock_container.id = "backup123"
        mock_client.containers.create.return_value = mock_container

        wrapper = DockerClientWrapper()
        container_id = wrapper.create_backup_container(
            "test-volume", "/backup/test.tar.gz"
        )

        assert container_id == "backup123"
        mock_client.containers.create.assert_called_once()

        # Verify container creation parameters
        call_args = mock_client.containers.create.call_args
        assert "ubuntu:latest" in call_args[1]["image"]
        assert "/data" in str(call_args[1]["volumes"])

    @patch("app.docker_client.docker.from_env")
    def test_security_volume_name_validation(self, mock_docker):
        """Test that malicious volume names are rejected."""
        mock_client = MagicMock()
        mock_docker.return_value = mock_client
        mock_client.ping.return_value = True

        wrapper = DockerClientWrapper()

        # Test path traversal in volume name
        with pytest.raises(ValueError, match="Invalid volume name"):
            wrapper.create_backup_container(
                "../../../etc/passwd", "/backup/test.tar.gz"
            )

        # Test command injection in volume name
        with pytest.raises(ValueError, match="Invalid volume name"):
            wrapper.create_backup_container("volume; rm -rf /", "/backup/test.tar.gz")

    @patch("app.docker_client.docker.from_env")
    def test_wait_for_container_success(self, mock_docker):
        """Test successful container wait."""
        mock_client = MagicMock()
        mock_docker.return_value = mock_client
        mock_client.ping.return_value = True

        mock_container = MagicMock()
        mock_container.wait.return_value = {"StatusCode": 0}
        mock_container.logs.return_value = b"Backup completed"
        mock_client.containers.get.return_value = mock_container

        wrapper = DockerClientWrapper()
        exit_code, logs = wrapper.wait_for_container("container123")

        assert exit_code == 0
        assert logs == "Backup completed"
        mock_container.wait.assert_called_once()
        mock_container.logs.assert_called_once()

    @patch("app.docker_client.docker.from_env")
    def test_cleanup_container(self, mock_docker):
        """Test container cleanup."""
        mock_client = MagicMock()
        mock_docker.return_value = mock_client
        mock_client.ping.return_value = True

        mock_container = MagicMock()
        mock_client.containers.get.return_value = mock_container

        wrapper = DockerClientWrapper()
        wrapper.cleanup_container("container123")

        mock_container.remove.assert_called_once_with(force=True)

    @patch("app.docker_client.docker.from_env")
    def test_cleanup_container_not_found(self, mock_docker):
        """Test cleanup of non-existent container."""
        mock_client = MagicMock()
        mock_docker.return_value = mock_client
        mock_client.ping.return_value = True

        mock_client.containers.get.side_effect = NotFound("Container not found")

        wrapper = DockerClientWrapper()
        # Should not raise exception
        wrapper.cleanup_container("nonexistent")
