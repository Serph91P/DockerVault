#!/bin/sh
set -eu

install -d -o dockervault -g dockervault -m 0755 /app/data /backups
install -d -o dockervault -g dockervault -m 0700 /app/data/ssh_keys
if [ ! -e /app/data/ssh_known_hosts ]; then
    install -o dockervault -g dockervault -m 0600 /dev/null /app/data/ssh_known_hosts
fi
install -d /run/nginx /run/dockervault
printf '# No trusted proxies configured for the local candidate\n' >/etc/nginx/conf.d/realip.conf
cp -R /etc/dockervault/services /run/dockervault/
exec s6-svscan /run/dockervault/services
