#!/bin/bash
set -e

# Get Docker socket GID if it exists
if [ -S /var/run/docker.sock ]; then
    DOCKER_SOCK_GID=$(stat -c '%g' /var/run/docker.sock)
    echo "Docker socket GID: $DOCKER_SOCK_GID"
    
    # Create docker group with the correct GID if it doesn't exist
    if ! getent group docker > /dev/null 2>&1; then
        groupadd -g "$DOCKER_SOCK_GID" docker
        echo "Created docker group with GID $DOCKER_SOCK_GID"
    fi
    
    # Add dockervault user to docker group
    usermod -aG docker dockervault
    echo "Added dockervault user to docker group"
fi

# Start supervisord
exec /usr/bin/tini -- /usr/bin/supervisord -c /etc/supervisor/supervisord.conf
