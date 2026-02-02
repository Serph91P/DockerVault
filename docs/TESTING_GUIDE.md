# DockerVault Testing Guide

This guide covers all testing approaches for DockerVault to ensure backups work correctly and nothing breaks.

## Quick Start Testing

### 1. Run Integration Tests (Recommended First)
```bash
# From project root - tests real Docker backup/restore operations
./integration_test.sh
```

This will:
- Create a test Docker volume with sample data
- Perform backup operations
- Verify backup integrity
- Test restore after simulated data loss
- Test backup with running containers
- Check security (path traversal prevention)
- Test large file handling

### 2. Run Unit Tests
```bash
# Backend Python tests
cd backend
source .venv/bin/activate  # or your venv path
pytest -v

# Frontend tests (if needed)
cd frontend
npm test
```

---

## Comprehensive Testing Checklist

### Level 1: Smoke Tests (5 minutes)
Quick verification that the app starts and basic features work.

- [ ] App starts without errors: `docker-compose up -d`
- [ ] Frontend loads: http://localhost:8080
- [ ] Backend health check: `curl http://localhost:8000/api/v1/docker/health`
- [ ] Docker volumes are listed in the UI
- [ ] Docker containers are listed in the UI

### Level 2: Functional Tests (15 minutes)
Test core backup functionality.

#### Backup Target Creation
- [ ] Create a new volume backup target
- [ ] Create a new path backup target  
- [ ] Enable/disable targets
- [ ] Delete a target

#### Manual Backup
- [ ] Trigger a manual backup
- [ ] Monitor backup progress in real-time
- [ ] Verify backup file is created
- [ ] Check backup appears in history

#### Backup Verification
- [ ] Download a backup file
- [ ] Verify tar.gz can be extracted
- [ ] Compare extracted content with original

#### Restore Operation
- [ ] Restore a backup (to original or new location)
- [ ] Verify restored data matches original

### Level 3: Stress Tests (30 minutes)
Test edge cases and limits.

#### Large Data
- [ ] Backup a volume with 1GB+ of data
- [ ] Backup many small files (10,000+ files)
- [ ] Monitor memory usage during backup

#### Concurrent Operations
- [ ] Run 3+ backups simultaneously
- [ ] Verify semaphore limits concurrent backups
- [ ] Check no data corruption

#### Long-Running Containers
- [ ] Backup volume attached to actively-writing container
- [ ] Test stop-before-backup option
- [ ] Verify container restart after backup

### Level 4: Failure/Recovery Tests (20 minutes)
Test error handling and recovery.

- [ ] Cancel a running backup
- [ ] Disk full scenario
- [ ] Network interruption during remote storage upload
- [ ] Docker daemon restart during backup
- [ ] Invalid backup file restore attempt
- [ ] Non-existent volume/path handling

---

## Manual Testing Steps

### Test 1: Basic Volume Backup

```bash
# 1. Create a test volume with data
docker volume create test_backup_volume
docker run --rm -v test_backup_volume:/data alpine sh -c "
  echo 'Important data' > /data/important.txt
  mkdir /data/subdir
  echo 'Nested file' > /data/subdir/nested.txt
"

# 2. Start DockerVault
docker-compose up -d

# 3. Open UI at http://localhost:8080
# 4. Go to Targets → Add Target
# 5. Select 'test_backup_volume' and save
# 6. Go to Backups → Trigger backup
# 7. Wait for completion
# 8. Verify backup file exists

# 9. Verify backup contents
ls -la ./backups/
tar -tzf ./backups/test_backup_volume_*.tar.gz
```

### Test 2: Data Integrity Verification

```bash
# 1. Get checksum of original data
docker run --rm -v test_backup_volume:/data alpine md5sum /data/important.txt

# 2. Extract backup and compare
mkdir /tmp/verify_backup
tar -xzf ./backups/test_backup_volume_*.tar.gz -C /tmp/verify_backup
md5sum /tmp/verify_backup/important.txt

# Checksums should match!
```

### Test 3: Restore Test

```bash
# 1. Corrupt/delete original data
docker run --rm -v test_backup_volume:/data alpine rm -rf /data/*

# 2. Verify data is gone
docker run --rm -v test_backup_volume:/data alpine ls -la /data/

# 3. Use DockerVault UI to restore backup

# 4. Verify data is restored
docker run --rm -v test_backup_volume:/data alpine cat /data/important.txt
```

### Test 4: Scheduled Backup

```bash
# 1. Create a schedule (every 5 minutes for testing)
# Via UI: Schedules → Add Schedule → Cron: */5 * * * *

# 2. Wait 5+ minutes

# 3. Verify automatic backup was created
ls -la ./backups/
```

---

## API Testing with curl

```bash
# Health check
curl http://localhost:8000/api/v1/docker/health

# List Docker volumes
curl http://localhost:8000/api/v1/docker/volumes

# List backup targets
curl http://localhost:8000/api/v1/targets

# List backups
curl http://localhost:8000/api/v1/backups

# Get backup metrics
curl http://localhost:8000/api/v1/backups/metrics/summary

# Create a backup (replace TARGET_ID with actual ID)
curl -X POST http://localhost:8000/api/v1/backups \
  -H "Content-Type: application/json" \
  -d '{"target_id": 1, "backup_type": "full"}'

# Validate backup prerequisites
curl -X POST http://localhost:8000/api/v1/backups/1/validate
```

---

## Automated Regression Testing

### Before Each Release

1. **Run Unit Tests**
   ```bash
   cd backend && pytest --cov=app --cov-fail-under=80
   ```

2. **Run Integration Tests**
   ```bash
   ./integration_test.sh
   ```

3. **Manual Smoke Test**
   - Start fresh: `docker-compose down -v && docker-compose up -d`
   - Create target, run backup, verify, restore

4. **Check Logs for Errors**
   ```bash
   docker-compose logs backend | grep -i error
   ```

---

## Production Safety Checklist

Before using in production:

- [ ] Test with your actual data volumes (on a clone/copy first!)
- [ ] Verify backup retention policy works
- [ ] Test remote storage upload (S3/FTP/WebDAV)
- [ ] Monitor disk space during scheduled backups
- [ ] Set up alerting for failed backups
- [ ] Document restore procedures
- [ ] Test restore on a different machine
- [ ] Verify backups work after DockerVault update

---

## Troubleshooting

### Backup fails with "Volume not found"
- Check volume name is correct
- Verify Docker socket permissions
- Check `docker volume ls` shows the volume

### Backup is 0 bytes or very small
- Volume might be empty
- Check container using volume isn't holding locks
- Try stopping containers before backup

### Restore doesn't work
- Verify backup file isn't corrupted: `tar -tzf backup.tar.gz`
- Check target path exists and is writable
- Ensure no containers are using the volume

### Performance issues
- Reduce concurrent backup limit in settings
- Increase compression level for network storage
- Check available disk space
