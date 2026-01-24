# DockerVault

A modern, containerized backup system for Docker volumes and host paths with a web interface.

## Features

- **Docker Integration**: Automatic detection of containers, volumes, and Compose stacks
- **Flexible Backup Targets**: Containers, volumes, host paths, or entire stacks
- **Dependency Management**: Respects `depends_on` relationships when stopping/starting containers
- **Scheduling**: Cron-based automatic backups with duration estimation
- **GFS Retention**: Grandfather-Father-Son retention strategy per backup target
- **Remote Storage**: Off-site backups via SSH, S3, WebDAV, FTP, or Rclone
- **Komodo Integration**: Optional integration with Komodo for container orchestration
- **Real-time Updates**: WebSocket-based live updates in the frontend
- **Security**: Docker socket mounted read-only

## Requirements

- Docker 20.10+
- Docker Compose 2.0+
- Linux Host (for Docker socket access)

## Installation

### 1. Clone the repository

```bash
git clone https://github.com/Serph91P/DockerVault.git
cd DockerVault
```

### 2. Configure environment variables

```bash
cp .env.example .env
```

Important settings in `.env`:

```env
# Get Docker group ID
DOCKER_GID=$(getent group docker | cut -d: -f3)

# Backup storage location
BACKUP_PATH=/path/to/backups

# Web interface port
PORT=8080
```

### 3. Start

```bash
docker compose up -d
```

The web interface is available at `http://localhost:8080`.

## Usage

### Dashboard

Overview showing:
- Active containers and volumes
- Recent backups with status
- Upcoming scheduled backups
- Statistics

### Containers

- List of all Docker containers
- Status (running/stopped)
- Associated volumes
- Compose stack information
- One-click backup target creation

### Volumes

- List of all Docker volumes
- Containers using the volume
- Mountpoints
- One-click backup target creation

### Stacks

- Docker Compose stacks
- Included containers and volumes
- Network information
- Complete stack as backup target

### Backup Targets

Configured backup targets with:
- Target type (container/volume/path/stack)
- Schedule (cron expression)
- Dependencies
- Pre/Post backup commands
- Container stop/start option
- Compression
- **Individual retention policy**

### Backups

- List of all backups
- Status and progress
- File size and duration
- Restore
- Delete

### Schedules

- Overview of scheduled backups
- Cron expression editor
- Next/Last execution
- Manual trigger

### Retention Policies

Each backup target can have its own GFS (Grandfather-Father-Son) retention policy:

| Option | Description | Example |
|--------|-------------|---------|
| `keep_last` | Keep the last N backups regardless of age | `3` |
| `keep_daily` | Keep one backup per day for N days | `7` |
| `keep_weekly` | Keep one backup per week for N weeks | `4` |
| `keep_monthly` | Keep one backup per month for N months | `6` |
| `keep_yearly` | Keep one backup per year for N years | `2` |

Example configuration (similar to restic):
```
--keep-last 3 --keep-daily 7 --keep-weekly 4 --keep-monthly 6 --keep-yearly 2
```

This keeps:
- The 3 most recent backups
- 7 daily backups (last week)
- 4 weekly backups (last month)
- 6 monthly backups (last 6 months)
- 2 yearly backups

### Remote Storage

Off-site backup synchronization to external storage:

| Type | Description | Example |
|------|-------------|---------|
| **Local/NFS** | Local directory or NFS mount | `/mnt/nas/backups` |
| **SSH/SFTP** | SSH server with rsync | `user@server:/backups` |
| **S3** | AWS S3, MinIO, Backblaze B2 | `s3://bucket/path` |
| **WebDAV** | Nextcloud, ownCloud | `https://cloud.example.com/dav/` |
| **FTP/FTPS** | FTP server | `ftp://server/path` |
| **Rclone** | 40+ providers (GDrive, Dropbox, OneDrive, ...) | `remote:path` |

**Features:**
- Automatic sync after backup
- Configurable per backup target
- Multiple remote destinations
- Connection test in UI
- Encrypted password storage

## Configuration

### Cron Expressions

Format: `Minute Hour Day Month Weekday`

Examples:
- `0 2 * * *` - Daily at 02:00
- `0 3 * * 0` - Sundays at 03:00
- `0 */6 * * *` - Every 6 hours
- `30 1 1 * *` - 1st of every month at 01:30

### Default Retention Policy

Default GFS retention (can be overridden per target):

```env
DEFAULT_KEEP_LAST=3
DEFAULT_KEEP_DAILY=7
DEFAULT_KEEP_WEEKLY=4
DEFAULT_KEEP_MONTHLY=6
DEFAULT_KEEP_YEARLY=2
```

### Komodo Integration

For integration with Komodo:

```env
KOMODO_ENABLED=true
KOMODO_API_URL=http://komodo:8080
KOMODO_API_KEY=your-api-key
```

Features:
- Backup notifications
- Container start/stop via Komodo
- Status synchronization

## Security

### Docker Socket

The Docker socket is mounted **read-only**:
```yaml
volumes:
  - /var/run/docker.sock:/var/run/docker.sock:ro
```

### Container Permissions

The container does not run as root but requires Docker group access:
```yaml
group_add:
  - ${DOCKER_GID:-999}
```

### Volume Access

Docker volumes are also mounted read-only:
```yaml
volumes:
  - /var/lib/docker/volumes:/var/lib/docker/volumes:ro
```

## Project Structure

```
DockerVault/
├── backend/
│   ├── app/
│   │   ├── api/              # REST API endpoints
│   │   ├── main.py           # FastAPI application
│   │   ├── config.py         # Configuration
│   │   ├── database.py       # SQLAlchemy models
│   │   ├── docker_client.py  # Docker SDK wrapper
│   │   ├── backup_engine.py  # Backup logic
│   │   ├── retention.py      # Retention manager
│   │   ├── scheduler.py      # APScheduler
│   │   ├── komodo.py         # Komodo client
│   │   ├── remote_storage.py # Remote storage backends
│   │   └── websocket.py      # WebSocket handler
│   ├── Dockerfile
│   └── requirements.txt
├── frontend/
│   ├── src/
│   │   ├── api/              # API client
│   │   ├── components/       # React components
│   │   ├── pages/            # Pages
│   │   └── store/            # State (WebSocket)
│   ├── Dockerfile
│   └── package.json
├── docker-compose.yml
├── .env.example
└── README.md
```

## Development

### Start backend locally

```bash
cd backend
python -m venv venv
source venv/bin/activate
pip install -r requirements.txt
uvicorn app.main:app --reload
```

### Start frontend locally

```bash
cd frontend
npm install
npm run dev
```

## API Documentation

After starting, API documentation is available at:
- Swagger UI: `http://localhost:8000/docs`
- ReDoc: `http://localhost:8000/redoc`

## License

MIT License

## Acknowledgments

- [FastAPI](https://fastapi.tiangolo.com/)
- [React](https://react.dev/)
- [Docker SDK for Python](https://docker-py.readthedocs.io/)
- [APScheduler](https://apscheduler.readthedocs.io/)
- [TailwindCSS](https://tailwindcss.com/)
