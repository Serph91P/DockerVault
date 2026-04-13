# DockerVault - Optimized Multi-Stage Dockerfile
# Following best practices for smaller, more secure container images

# =============================================================================
# Stage 1: Frontend Dependencies
# =============================================================================
FROM node:24-alpine AS frontend-deps

WORKDIR /app

# Copy only package files for dependency caching
COPY frontend/package.json frontend/package-lock.json* ./

# Install dependencies (cached if package files unchanged)
RUN npm ci --prefer-offline --no-audit

# =============================================================================
# Stage 2: Build Frontend
# =============================================================================
FROM frontend-deps AS frontend-builder

# Build argument to control environment
ARG BUILD_ENV=production

WORKDIR /app

# Copy source code
COPY frontend/ .

# Set NODE_ENV based on build argument
ENV NODE_ENV=${BUILD_ENV}
RUN npm run build

# =============================================================================
# Stage 3: Python Dependencies Builder
# =============================================================================
FROM python:3.14-slim AS python-deps

WORKDIR /app

# Install build dependencies for Python packages
RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential \
    libffi-dev \
    && rm -rf /var/lib/apt/lists/*

# Copy requirements first for caching
COPY backend/requirements.txt .

# Install Python dependencies to a virtual environment
RUN python -m venv /opt/venv
ENV PATH="/opt/venv/bin:$PATH"
RUN pip install --no-cache-dir --upgrade pip==25.3 && \
    pip install --no-cache-dir -r requirements.txt

# =============================================================================
# Stage 4: Production Image
# =============================================================================
FROM python:3.14-slim AS production

# Build arguments
ARG VERSION=dev
ARG COMMIT_SHA=unknown
ARG BRANCH=unknown
ARG BUILD_ENV=production

# Labels (OCI standard)
LABEL org.opencontainers.image.title="DockerVault" \
      org.opencontainers.image.description="Docker Volume Backup Manager with Web UI" \
      org.opencontainers.image.version="${VERSION}" \
      org.opencontainers.image.revision="${COMMIT_SHA}" \
      org.opencontainers.image.source="https://github.com/Serph91P/DockerVault" \
      org.opencontainers.image.licenses="MIT"

WORKDIR /app

# Install only runtime dependencies (no build tools)
RUN apt-get update && apt-get install -y --no-install-recommends \
    nginx \
    supervisor \
    curl \
    rsync \
    openssh-client \
    tini \
    openssl \
    sudo \
    && rm -rf /var/lib/apt/lists/* \
    && apt-get clean \
    && rm -f /etc/nginx/sites-enabled/default

# Install age for encryption (not available in standard repos)
SHELL ["/bin/bash", "-o", "pipefail", "-c"]
RUN ARCH=$(dpkg --print-architecture) && \
    if [ "$ARCH" = "amd64" ]; then AGE_ARCH="amd64"; \
    elif [ "$ARCH" = "arm64" ]; then AGE_ARCH="arm64"; \
    else echo "Unsupported architecture: $ARCH" && exit 1; fi && \
    curl -fsSL "https://github.com/FiloSottile/age/releases/download/v1.2.1/age-v1.2.1-linux-${AGE_ARCH}.tar.gz" | \
    tar -xz -C /usr/local/bin --strip-components=1 age/age age/age-keygen && \
    chmod +x /usr/local/bin/age /usr/local/bin/age-keygen

# Copy Python virtual environment from builder
COPY --from=python-deps /opt/venv /opt/venv
ENV PATH="/opt/venv/bin:$PATH" \
    PYTHONDONTWRITEBYTECODE=1 \
    PYTHONUNBUFFERED=1

# Create non-root user before copying files
RUN groupadd --gid 1000 dockervault && \
    useradd --uid 1000 --gid dockervault --shell /bin/bash --create-home dockervault

# Allow dockervault to run ONLY the tar-worker script as root (no password).
# This is required because Docker volume files are owned by container-internal
# UIDs (e.g. mongodb:999) that the dockervault user cannot read.
# The trailing * allows sudo to pass any argument (the JSON config) to the script.
RUN echo 'dockervault ALL=(root) NOPASSWD: /opt/venv/bin/python3 /app/app/tar_worker.py *' \
    > /etc/sudoers.d/dockervault-tar && \
    chmod 0440 /etc/sudoers.d/dockervault-tar && \
    visudo -c

# Create necessary directories with proper ownership
RUN mkdir -p /app/data /backups /var/log/supervisor /run/nginx && \
    chown -R dockervault:dockervault /app /backups /var/log/supervisor

# Copy backend application
COPY --chown=dockervault:dockervault backend/app ./app

# Copy frontend build from builder stage
COPY --from=frontend-builder /app/dist /usr/share/nginx/html

# Copy configuration files
COPY frontend/nginx.conf /etc/nginx/conf.d/default.conf
COPY docker/supervisord.conf /etc/supervisor/conf.d/dockervault.conf

# Create empty realip config (populated by entrypoint from TRUSTED_PROXIES)
RUN echo '# No TRUSTED_PROXIES configured' > /etc/nginx/conf.d/realip.conf

# Fix nginx permissions for non-root operation
RUN chown -R dockervault:dockervault /var/log/nginx /var/lib/nginx /run/nginx && \
    chmod 755 /var/log/nginx /var/lib/nginx /run/nginx

# Copy entrypoint script
COPY docker/entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh

# Environment variables
ENV DATABASE_URL=sqlite+aiosqlite:///./data/backup.db \
    DOCKER_SOCKET=/var/run/docker.sock \
    BACKUP_BASE_PATH=/backups \
    TZ=UTC \
    VERSION=${VERSION} \
    COMMIT_SHA=${COMMIT_SHA} \
    APP_ENV=${BUILD_ENV}

# Expose port
EXPOSE 80

# Health check
HEALTHCHECK --interval=30s --timeout=10s --start-period=15s --retries=3 \
    CMD curl -sf http://localhost:8000/health || exit 1

# Declare volumes
VOLUME ["/app/data", "/backups"]

# Entrypoint handles docker group setup and starts supervisord
ENTRYPOINT ["/entrypoint.sh"]
