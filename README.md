<!-- prettier-ignore -->
<div align="center">

<img src="frontend/public/backup.svg" alt="DockerVault logo" height="96" />

# DockerVault

**Automated Docker backup solution with a modern web interface**

[![Docker](https://img.shields.io/badge/Docker-20.10+-2496ED?style=flat-square&logo=docker&logoColor=white)](https://www.docker.com/)
[![Python](https://img.shields.io/badge/Python-3.14-3776AB?style=flat-square&logo=python&logoColor=white)](https://www.python.org/)
[![React](https://img.shields.io/badge/React-18-61DAFB?style=flat-square&logo=react&logoColor=black)](https://react.dev/)
[![FastAPI](https://img.shields.io/badge/FastAPI-009688?style=flat-square&logo=fastapi&logoColor=white)](https://fastapi.tiangolo.com/)
[![License](https://img.shields.io/badge/License-MIT-yellow?style=flat-square)](LICENSE)

[Features](#features) • [Getting Started](#getting-started) • [Configuration](#configuration) • [Development](#development)

</div>

DockerVault is a containerized backup system for Docker volumes and host paths. It provides automatic detection of containers, volumes, and Compose stacks, with flexible scheduling, GFS retention policies, and remote storage synchronization.

## Features

- **Docker Integration** — Automatic detection of containers, volumes, and Compose stacks
- **Flexible Targets** — Back up containers, volumes, host paths, or entire stacks
- **Dependency Management** — Respects `depends_on` relationships when stopping/starting containers
- **Cron Scheduling** — Automated backups with duration estimation
- **GFS Retention** — Grandfather-Father-Son retention strategy per backup target
- **Remote Storage** — Sync to SSH, S3, WebDAV, FTP, or 40+ providers via Rclone
- **Real-time UI** — WebSocket-based live updates in the web interface
- **Komodo Integration** — Optional integration with Komodo for container orchestration
- **Security First** — Docker socket and volumes mounted read-only

## Getting Started

### Prerequisites

- Docker 20.10+
- Docker Compose 2.0+
- Linux host (for Docker socket access)

### Quick Start

1. **Clone the repository**

   ```bash
   git clone https://github.com/Serph91P/DockerVault.git
   cd DockerVault
   ```

2. **Configure environment**

   ```bash
   cp .env.example .env
   ```

   Edit `.env` with your settings:

   ```env
   # Docker group ID (find with: getent group docker | cut -d: -f3)
   DOCKER_GID=999

   # Backup storage location
   BACKUP_PATH=/path/to/backups

   # Web interface port
   PORT=8080
   ```

3. **Start DockerVault**

   ```bash
   docker compose up -d
   ```

4. **Access the web interface** at `http://localhost:8080`

> [!TIP]
> Use `docker compose logs -f` to monitor startup and check for any configuration issues.

## Configuration

### Retention Policy

DockerVault uses a GFS (Grandfather-Father-Son) retention strategy. Each backup target can have its own policy:

| Option | Description | Default |
|--------|-------------|---------|
| `keep_last` | Keep the last N backups | `3` |
| `keep_daily` | Keep one backup per day for N days | `7` |
| `keep_weekly` | Keep one backup per week for N weeks | `4` |
| `keep_monthly` | Keep one backup per month for N months | `6` |
| `keep_yearly` | Keep one backup per year for N years | `2` |

Configure defaults via environment variables:

```env
DEFAULT_KEEP_LAST=3
DEFAULT_KEEP_DAILY=7
DEFAULT_KEEP_WEEKLY=4
DEFAULT_KEEP_MONTHLY=6
DEFAULT_KEEP_YEARLY=2
```

### Remote Storage

Sync backups to external storage providers:

| Type | Description | Example |
|------|-------------|---------|
| Local/NFS | Local directory or NFS mount | `/mnt/nas/backups` |
| SSH/SFTP | SSH server with rsync | `user@server:/backups` |
| S3 | AWS S3, MinIO, Backblaze B2 | `s3://bucket/path` |
| WebDAV | Nextcloud, ownCloud | `https://cloud.example.com/dav/` |
| FTP/FTPS | FTP server | `ftp://server/path` |
| Rclone | 40+ providers (GDrive, Dropbox, OneDrive, ...) | `remote:path` |

### Cron Expressions

Schedule format: `Minute Hour Day Month Weekday`

| Expression | Description |
|------------|-------------|
| `0 2 * * *` | Daily at 02:00 |
| `0 3 * * 0` | Sundays at 03:00 |
| `0 */6 * * *` | Every 6 hours |
| `30 1 1 * *` | 1st of every month at 01:30 |

### Komodo Integration

Enable optional integration with [Komodo](https://github.com/mbecker20/komodo):

```env
KOMODO_ENABLED=true
KOMODO_API_URL=http://komodo:8080
KOMODO_API_KEY=your-api-key
```

## Security

DockerVault follows security best practices:

- **Docker socket** — Mounted read-only (`/var/run/docker.sock:ro`)
- **Docker volumes** — Mounted read-only (`/var/lib/docker/volumes:ro`)
- **Non-root user** — Container runs as unprivileged user with Docker group access

### Backup Encryption

DockerVault supports end-to-end encryption for your backups using **AES-256-CBC** with envelope encryption:

- **Per-backup keys** — Each backup gets a unique Data Encryption Key (DEK)
- **Asymmetric wrapping** — DEKs are encrypted with your public key using [age](https://github.com/FiloSottile/age)
- **Disaster recovery** — Backups can be restored without DockerVault using standard command-line tools

#### Setting Up Encryption

1. Navigate to **Settings → Backup Encryption**
2. Click **Set Up Encryption** to generate a new key pair
3. **Download your private key** and store it securely (password manager, encrypted drive)
4. Confirm that you have saved the private key

> [!CAUTION]
> Your private key is only shown **once** during setup. If lost, encrypted backups **cannot be recovered**.

#### Restoring Encrypted Backups

**With DockerVault:**
1. Navigate to **Backups** and select the backup
2. Click **Restore** — DockerVault handles decryption automatically

**Without DockerVault (Disaster Recovery):**

If you lose access to DockerVault, you can still recover encrypted backups using standard command-line tools:

```bash
# Prerequisites: age (https://github.com/FiloSottile/age) and openssl

# 1. Save your private key to a file
cat > private_key.txt << 'EOF'
AGE-SECRET-KEY-1XXXXXX...
EOF
chmod 600 private_key.txt

# 2. Decrypt the DEK (Data Encryption Key)
age -d -i private_key.txt backup.tar.gz.key > dek.txt

# 3. Decrypt the backup
openssl enc -d -aes-256-cbc -pbkdf2 -iter 100000 \
    -in backup.tar.gz.enc \
    -out backup.tar.gz \
    -pass file:dek.txt

# 4. Extract the backup
tar xzf backup.tar.gz

# 5. Clean up (don't leave keys lying around)
rm dek.txt private_key.txt
```

> [!TIP]
> The downloaded private key file includes these recovery instructions.

## Development

### Backend

```bash
cd backend
python -m venv venv
source venv/bin/activate
pip install -r requirements.txt
uvicorn app.main:app --reload
```

### Frontend

```bash
cd frontend
npm install
npm run dev
```

### API Documentation

When running, API documentation is available at:
- **Swagger UI**: `http://localhost:8000/docs`
- **ReDoc**: `http://localhost:8000/redoc`

## Project Structure

```
DockerVault/
├── backend/
│   └── app/
│       ├── api/              # REST API endpoints
│       ├── backup_engine.py  # Backup logic
│       ├── docker_client.py  # Docker SDK wrapper
│       ├── remote_storage.py # Remote storage backends
│       ├── retention.py      # Retention manager
│       ├── scheduler.py      # APScheduler integration
│       └── websocket.py      # Real-time updates
├── frontend/
│   └── src/
│       ├── components/       # React components
│       ├── pages/            # Application pages
│       └── api/              # API client
├── docker-compose.yml
└── Dockerfile
```

## Resources

- [FastAPI](https://fastapi.tiangolo.com/) — Backend framework
- [React](https://react.dev/) — Frontend library
- [Docker SDK for Python](https://docker-py.readthedocs.io/) — Docker integration
- [APScheduler](https://apscheduler.readthedocs.io/) — Job scheduling
- [TailwindCSS](https://tailwindcss.com/) — UI styling
