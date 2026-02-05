#!/bin/bash
set -e

# Get Docker socket GID if it exists
if [ -S /var/run/docker.sock ]; then
    DOCKER_SOCK_GID=$(stat -c '%g' /var/run/docker.sock)
    echo "Docker socket GID: $DOCKER_SOCK_GID"
    
    # Only do user setup if not running as root
    if [ "$(id -u)" != "0" ]; then
        # Create docker group with the correct GID if it doesn't exist
        if ! getent group docker > /dev/null 2>&1; then
            groupadd -g "$DOCKER_SOCK_GID" docker
            echo "Created docker group with GID $DOCKER_SOCK_GID"
        fi
        
        # Add dockervault user to docker group
        usermod -aG docker dockervault
        echo "Added dockervault user to docker group"
    else
        echo "Running as root - skipping group setup"
    fi
fi

# Ensure directories exist and have proper permissions
mkdir -p /app/data /backups /var/log/supervisor /run/nginx
chmod 755 /app/data /backups /var/log/supervisor /run/nginx 2>/dev/null || true

# Start supervisord
exec /usr/bin/tini -- /usr/bin/supervisord -c /etc/supervisor/supervisord.conf
