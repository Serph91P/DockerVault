1. manche backups gehen nicht und es kommt der fehler:
```
dockervault  | 2026-02-24T15:53:33.996815683Z INFO:     192.168.16.19:0 - "POST /api/v1/backups HTTP/1.1" 200 OK
dockervault  | 2026-02-24T15:53:34.036437738Z INFO:     192.168.16.19:0 - "GET /api/v1/backups?limit=500 HTTP/1.1" 200 OK
dockervault  | 2026-02-24T15:53:34.238549089Z 2026-02-24 16:53:34,238 - app.backup_engine - INFO - Stack backup will stop containers: ['Nextcloud', 'Nextcloud-mariadb', 'Nextcloud-redis']
dockervault  | 2026-02-24T15:53:35.541990140Z 2026-02-24 16:53:35,541 - app.docker_client - INFO - Stopped container: Nextcloud
dockervault  | 2026-02-24T15:53:36.743735898Z 2026-02-24 16:53:36,743 - app.docker_client - INFO - Stopped container: Nextcloud-mariadb
dockervault  | 2026-02-24T15:53:36.998100442Z 2026-02-24 16:53:36,997 - app.docker_client - INFO - Stopped container: Nextcloud-redis
dockervault  | 2026-02-24T15:53:39.112829122Z INFO:     192.168.16.19:0 - "GET /api/v1/backups?limit=500 HTTP/1.1" 200 OK
dockervault  | 2026-02-24T15:53:43.691413065Z INFO:     192.168.16.19:0 - "WebSocket /ws/updates" 403
dockervault  | 2026-02-24T15:53:43.691435110Z INFO:     connection rejected (403 Forbidden)
dockervault  | 2026-02-24T15:53:43.691535123Z INFO:     connection closed
dockervault  | 2026-02-24T15:53:45.325734620Z 2026-02-24 16:53:45,325 - app.backup_engine - ERROR - Backup 69 failed: [Errno 13] Permission denied: '/var/lib/docker/volumes/Nextcloud-mariadb-data/_data/.my-healthcheck.cnf'
dockervault  | 2026-02-24T15:53:45.521234467Z 2026-02-24 16:53:45,521 - app.docker_client - INFO - Started container: Nextcloud-redis
dockervault  | 2026-02-24T15:53:45.671601237Z 2026-02-24 16:53:45,671 - app.docker_client - INFO - Started container: Nextcloud-mariadb
dockervault  | 2026-02-24T15:53:45.894835676Z 2026-02-24 16:53:45,894 - app.docker_client - INFO - Started container: Nextcloud
```
das muss behoben werden.

2. dann das problem mit den exclude paths scheint irgendwie nicht zu klappen ich weiß nicht ganz wie ich das angeben soll. weil bei emby habe ich nur bestimmte ordner inkludiert aber das backup ist nur 112B was nicht sein kann. 

3. Das Edit Fenster von einem Backup sollte in einer übersicht alle sachen anzeigen die in dem Wizzard angegeben werden. Weil aktuell sehe ich zum Beispiel nicht welche Excludes oder Includes ich angegeben habe usw. 

4. es wird immer sofort about 1 hour ago angezeigt auch wenn ich ein backup gerade ausgeführt habe kann man das genauer machen?
