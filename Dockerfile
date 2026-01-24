# DockerVault - Combined Dockerfile
# Multi-stage build for both backend and frontend

# =============================================================================
# Stage 1: Build Frontend
# =============================================================================
FROM node:24-alpine AS frontend-builder

WORKDIR /app/frontend

# Copy package files
COPY frontend/package.json frontend/package-lock.json* ./

# Install dependencies
RUN npm ci || npm install

# Copy source code
COPY frontend/ .

# Build the application
RUN npm run build

# =============================================================================
# Stage 2: Production Image
# =============================================================================
FROM python:3.14-slim

# Build arguments
ARG VERSION=dev
ARG COMMIT_SHA=unknown
ARG BRANCH=unknown

# Labels
LABEL org.opencontainers.image.title="DockerVault"
LABEL org.opencontainers.image.description="Docker Volume Backup Manager with Web UI"
LABEL org.opencontainers.image.version="${VERSION}"
LABEL org.opencontainers.image.revision="${COMMIT_SHA}"
LABEL org.opencontainers.image.source="https://github.com/Serph91P/DockerVault"

WORKDIR /app

# Install runtime dependencies
RUN apt-get update && apt-get install -y --no-install-recommends \
    nginx \
    supervisor \
    curl \
    rsync \
    openssh-client \
    && rm -rf /var/lib/apt/lists/*

# Copy and install Python requirements
COPY backend/requirements.txt .
RUN pip install --no-cache-dir -r requirements.txt

# Copy backend application
COPY backend/app ./app

# Copy frontend build
COPY --from=frontend-builder /app/frontend/dist /usr/share/nginx/html

# Copy nginx configuration
COPY frontend/nginx.conf /etc/nginx/conf.d/default.conf

# Create directories and user
RUN useradd -m -u 1000 dockervault && \
    mkdir -p /app/data /backups /var/log/supervisor && \
    chown -R dockervault:dockervault /app /backups

# Copy supervisor configuration
COPY docker/supervisord.conf /etc/supervisor/conf.d/dockervault.conf

# Environment variables
ENV DATABASE_URL=sqlite+aiosqlite:///./data/backup.db \
    DOCKER_SOCKET=/var/run/docker.sock \
    BACKUP_BASE_PATH=/backups \
    TZ=UTC \
    VERSION=${VERSION} \
    COMMIT_SHA=${COMMIT_SHA}

# Expose ports
EXPOSE 80

# Health check
HEALTHCHECK --interval=30s --timeout=10s --start-period=10s --retries=3 \
    CMD curl -f http://localhost:8000/api/v1/docker/health || exit 1

# Volumes
VOLUME ["/app/data", "/backups"]

# Start supervisor
CMD ["/usr/bin/supervisord", "-c", "/etc/supervisor/supervisord.conf"]
