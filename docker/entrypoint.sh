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

# /app/data holds the SQLite database, ssh_known_hosts and ssh_keys.
# Backend runs as ``dockervault``, so it must own this directory.
chown -R dockervault:dockervault /app/data 2>/dev/null || true

# Pre-create a persistent known_hosts file with safe perms so SSH
# trust-on-first-use can write to it and OpenSSH stops complaining.
touch /app/data/ssh_known_hosts
chown dockervault:dockervault /app/data/ssh_known_hosts 2>/dev/null || true
chmod 600 /app/data/ssh_known_hosts 2>/dev/null || true

# Persistent directory for SSH keypairs generated via the UI. 0700 +
# dockervault ownership so OpenSSH accepts the private keys.
mkdir -p /app/data/ssh_keys
chown dockervault:dockervault /app/data/ssh_keys 2>/dev/null || true
chmod 700 /app/data/ssh_keys 2>/dev/null || true

# Fix ownership of backup directory so the dockervault user can read,
# browse, and delete all backup files (including ones previously created
# as root or by containers with different UIDs).
chown -R dockervault:dockervault /backups 2>/dev/null || true

# Generate nginx realip config from TRUSTED_PROXIES env var.
# When set, nginx uses the real_ip module to extract the actual client IP
# from X-Forwarded-For headers sent by trusted reverse proxies.
REALIP_CONF="/etc/nginx/conf.d/realip.conf"
if [ -n "${TRUSTED_PROXIES:-}" ]; then
    echo "# Auto-generated from TRUSTED_PROXIES" > "$REALIP_CONF"
    echo "real_ip_header X-Forwarded-For;" >> "$REALIP_CONF"
    echo "real_ip_recursive on;" >> "$REALIP_CONF"
    IFS=',' read -ra PROXIES <<< "$TRUSTED_PROXIES"
    for proxy in "${PROXIES[@]}"; do
        proxy=$(echo "$proxy" | xargs)  # trim whitespace
        if [ -n "$proxy" ]; then
            echo "set_real_ip_from $proxy;" >> "$REALIP_CONF"
        fi
    done
    echo "Configured trusted proxies: ${TRUSTED_PROXIES}"
else
    # Empty file so nginx include doesn't fail
    echo "# No TRUSTED_PROXIES configured" > "$REALIP_CONF"
fi

# Start supervisord
exec /usr/bin/tini -- /usr/bin/supervisord -c /etc/supervisor/supervisord.conf
