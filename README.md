# Docker Backup Manager

Ein modernes, containerisiertes Backup-System für Docker Volumes und Host-Pfade mit Web-Interface.

## 🚀 Features

- **Docker Integration**: Automatische Erkennung von Containern, Volumes und Compose-Stacks
- **Flexible Backup-Ziele**: Container, Volumes, Host-Pfade oder ganze Stacks
- **Abhängigkeitsverwaltung**: Beachtet `depends_on` Beziehungen beim Stoppen/Starten
- **Zeitplanung**: Cron-basierte automatische Backups mit Schätzung der Backup-Dauer
- **GFS Retention**: Grandfather-Father-Son Aufbewahrungsstrategie (täglich/wöchentlich/monatlich/jährlich)
- **Remote Storage**: Off-Site Backups via SSH, S3, WebDAV, FTP oder Rclone
- **Komodo Integration**: Optionale Anbindung an Komodo für Container-Orchestrierung
- **Echtzeit-Updates**: WebSocket-basierte Live-Updates im Frontend
- **Sicherheit**: Docker Socket wird read-only gemountet

## 📋 Voraussetzungen

- Docker 20.10+
- Docker Compose 2.0+
- Linux Host (für Docker Socket)

## 🛠️ Installation

### 1. Repository klonen oder Dateien herunterladen

```bash
git clone <repository-url>
cd docker-volume-backup
```

### 2. Umgebungsvariablen konfigurieren

```bash
cp .env.example .env
```

Wichtige Einstellungen in `.env`:

```env
# Docker Group ID ermitteln
DOCKER_GID=$(getent group docker | cut -d: -f3)

# Backup-Speicherort
BACKUP_PATH=/path/to/backups

# Web-Interface Port
PORT=8080
```

### 3. Starten

```bash
docker compose up -d
```

Das Web-Interface ist dann unter `http://localhost:8080` erreichbar.

## 🖥️ Verwendung

### Dashboard

Zeigt eine Übersicht über:
- Aktive Container und Volumes
- Letzte Backups mit Status
- Nächste geplante Backups
- Statistiken

### Container

- Liste aller Docker Container
- Status (running/stopped)
- Zugehörige Volumes
- Compose Stack Informationen
- Ein-Klick Backup-Target Erstellung

### Volumes

- Liste aller Docker Volumes
- Verwendende Container
- Mountpoints
- Ein-Klick Backup-Target Erstellung

### Stacks

- Docker Compose Stacks
- Enthaltene Container und Volumes
- Netzwerk-Informationen
- Kompletter Stack als Backup-Target

### Backup Targets

Konfigurierte Backup-Ziele mit:
- Target-Typ (Container/Volume/Pfad/Stack)
- Zeitplan (Cron-Expression)
- Abhängigkeiten
- Pre/Post Backup Commands
- Container Stop/Start Option
- Komprimierung

### Backups

- Liste aller Backups
- Status und Fortschritt
- Dateigröße und Dauer
- Wiederherstellen
- Löschen

### Zeitpläne

- Übersicht geplanter Backups
- Cron-Expression Editor
- Nächste/Letzte Ausführung
- Manuelles Auslösen

### Retention Policies

- GFS Strategie Konfiguration
- Täglich/Wöchentlich/Monatlich/Jährlich
- Automatisches Aufräumen
- Verwaiste Dateien löschen

### Remote Storage

Off-Site Backup-Synchronisation zu externen Speicherorten:

| Typ | Beschreibung | Verwendung |
|-----|--------------|------------|
| **Local/NFS** | Lokales Verzeichnis oder NFS-Mount | `/mnt/nas/backups` |
| **SSH/SFTP** | SSH-Server mit rsync | `user@server:/backups` |
| **S3** | AWS S3, MinIO, Backblaze B2 | `s3://bucket/path` |
| **WebDAV** | Nextcloud, ownCloud | `https://cloud.example.com/dav/` |
| **FTP/FTPS** | FTP-Server | `ftp://server/path` |
| **Rclone** | 40+ Provider (GDrive, Dropbox, OneDrive, ...) | `remote:path` |

**Features:**
- Automatische Synchronisation nach Backup
- Pro Backup-Target konfigurierbar
- Mehrere Remote-Ziele gleichzeitig
- Verbindungstest im UI
- Verschlüsselte Passwort-Speicherung

## ⚙️ Konfiguration

### Cron Expressions

Format: `Minute Stunde Tag Monat Wochentag`

Beispiele:
- `0 2 * * *` - Täglich um 02:00
- `0 3 * * 0` - Sonntags um 03:00
- `0 */6 * * *` - Alle 6 Stunden
- `30 1 1 * *` - Am 1. jeden Monats um 01:30

### Retention Policy (GFS)

Die Grandfather-Father-Son Strategie behält:
- **Täglich**: Die letzten N täglichen Backups
- **Wöchentlich**: Ein Backup pro Woche für N Wochen
- **Monatlich**: Ein Backup pro Monat für N Monate
- **Jährlich**: Ein Backup pro Jahr für N Jahre

Beispiel-Konfiguration:
```
keep_daily: 7      # Letzte 7 Tage
keep_weekly: 4     # Letzte 4 Wochen
keep_monthly: 6    # Letzte 6 Monate
keep_yearly: 2     # Letzte 2 Jahre
max_age_days: 365  # Maximal 1 Jahr alt
```

### Komodo Integration

Für die Integration mit Komodo:

```env
KOMODO_ENABLED=true
KOMODO_API_URL=http://komodo:8080
KOMODO_API_KEY=your-api-key
```

Features:
- Backup-Benachrichtigungen
- Container Start/Stop über Komodo
- Status-Synchronisation

## 🔒 Sicherheit

### Docker Socket

Der Docker Socket wird **read-only** gemountet:
```yaml
volumes:
  - /var/run/docker.sock:/var/run/docker.sock:ro
```

### Container-Berechtigungen

Der Container läuft nicht als root, benötigt aber Zugriff auf die Docker-Gruppe:
```yaml
group_add:
  - ${DOCKER_GID:-999}
```

### Volume-Zugriff

Docker Volumes werden ebenfalls read-only gemountet:
```yaml
volumes:
  - /var/lib/docker/volumes:/var/lib/docker/volumes:ro
```

## 📁 Projektstruktur

```
docker-volume-backup/
├── backend/
│   ├── app/
│   │   ├── api/              # REST API Endpoints
│   │   ├── main.py           # FastAPI Anwendung
│   │   ├── config.py         # Konfiguration
│   │   ├── database.py       # SQLAlchemy Modelle
│   │   ├── docker_client.py  # Docker SDK Wrapper
│   │   ├── backup_engine.py  # Backup-Logik
│   │   ├── retention.py      # Retention Manager
│   │   ├── scheduler.py      # APScheduler
│   │   ├── komodo.py         # Komodo Client
│   │   └── websocket.py      # WebSocket Handler
│   ├── Dockerfile
│   └── requirements.txt
├── frontend/
│   ├── src/
│   │   ├── api/              # API Client
│   │   ├── components/       # React Komponenten
│   │   ├── pages/            # Seiten
│   │   └── store/            # Zustand (WebSocket)
│   ├── Dockerfile
│   └── package.json
├── docker-compose.yml
├── .env.example
└── README.md
```

## 🛠️ Entwicklung

### Backend lokal starten

```bash
cd backend
python -m venv venv
source venv/bin/activate
pip install -r requirements.txt
uvicorn app.main:app --reload
```

### Frontend lokal starten

```bash
cd frontend
npm install
npm run dev
```

## 📝 API Dokumentation

Nach dem Start ist die API-Dokumentation verfügbar unter:
- Swagger UI: `http://localhost:8000/docs`
- ReDoc: `http://localhost:8000/redoc`

## 🤝 Lizenz

MIT License

## 🙏 Acknowledgments

- [FastAPI](https://fastapi.tiangolo.com/)
- [React](https://react.dev/)
- [Docker SDK for Python](https://docker-py.readthedocs.io/)
- [APScheduler](https://apscheduler.readthedocs.io/)
- [TailwindCSS](https://tailwindcss.com/)
