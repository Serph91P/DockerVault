#!/bin/bash
set -e

# Get Docker socket GID if it exists and grant access to the dockervault user.
# The entrypoint runs as root (docker-compose user: root), but the backend
# process runs as 'dockervault' under supervisord, so it needs group access.
if [ -S /var/run/docker.sock ]; then
    DOCKER_SOCK_GID=$(stat -c '%g' /var/run/docker.sock)
    echo "Docker socket GID: $DOCKER_SOCK_GID"

    # Create a group with the socket's GID if one doesn't already exist
    if ! getent group "$DOCKER_SOCK_GID" > /dev/null 2>&1; then
        groupadd -g "$DOCKER_SOCK_GID" dockersock
        echo "Created dockersock group with GID $DOCKER_SOCK_GID"
    fi

    # Add dockervault user to the socket's group
    SOCK_GROUP=$(getent group "$DOCKER_SOCK_GID" | cut -d: -f1)
    usermod -aG "$SOCK_GROUP" dockervault
    echo "Added dockervault user to group $SOCK_GROUP (GID $DOCKER_SOCK_GID)"
fi

# Ensure directories exist and have proper permissions
mkdir -p /app/data /backups /var/log/supervisor /run/nginx
chmod 755 /app/data /backups /var/log/supervisor /run/nginx 2>/dev/null || true

# Start supervisord
exec /usr/bin/tini -- /usr/bin/supervisord -c /etc/supervisor/supervisord.conf
