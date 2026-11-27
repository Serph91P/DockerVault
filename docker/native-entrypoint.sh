#!/bin/sh
set -eu

if [ -S /var/run/docker.sock ]; then
    socket_gid=$(stat -c '%g' /var/run/docker.sock)
    if [ "$socket_gid" != "$(id -g dockervault)" ]; then
        if ! getent group "$socket_gid" >/dev/null; then
            groupadd -g "$socket_gid" "dockersock-$socket_gid"
        fi
        socket_group=$(getent group "$socket_gid" | cut -d: -f1)
        usermod -aG "$socket_group" dockervault
    fi
fi

install -d -o dockervault -g dockervault -m 0755 /app/data /backups
install -d -o dockervault -g dockervault -m 0700 /app/data/ssh_keys
if [ ! -e /app/data/ssh_known_hosts ]; then
    install -o dockervault -g dockervault -m 0600 /dev/null /app/data/ssh_known_hosts
fi
install -d /run/nginx /run/dockervault
/usr/local/libexec/dockervault-nginx-realip >/etc/nginx/conf.d/realip.conf
cp -R /etc/dockervault/services /run/dockervault/
exec s6-svscan /run/dockervault/services
