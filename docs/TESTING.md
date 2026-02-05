# DockerVault Testing Guide

Comprehensive testing documentation for DockerVault covering all testing approaches, from quick integration tests to in-depth unit testing and manual verification.

## Quick Start

### Run All Tests

```bash
# Backend unit tests with coverage
cd backend
source .venv/bin/activate  # or: python -m venv .venv && source .venv/bin/activate
pip install -r requirements-dev.txt
pytest -v --cov=app --cov-fail-under=80

# Frontend tests
cd frontend
npm install
npm test -- --run
```

### Run Integration Tests (Recommended First)

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

---

## Test Structure

```
backend/
├── tests/
│   ├── conftest.py          # Shared fixtures (database, Docker mocks)
│   ├── test_backup_engine.py # Backup logic tests
│   ├── test_database.py      # Database operations tests
│   ├── test_docker_client.py # Docker SDK wrapper tests
│   ├── test_scheduler.py     # APScheduler tests
│   └── test_api_backups.py   # API endpoint tests

frontend/
└── src/
    ├── api/__tests__/
    │   └── index.test.ts       # API client tests
    ├── pages/__tests__/
    │   ├── Backups.test.tsx    # Backups page tests
    │   └── Dashboard.test.tsx  # Dashboard tests
    ├── store/__tests__/
    │   └── websocket.test.ts   # WebSocket store tests
    └── test/
        ├── setup.ts            # Test configuration
        └── mocks/
            ├── handlers.ts     # MSW request handlers
            └── server.ts       # MSW server setup
```

---

## Coverage Requirements

| Category | Minimum | Target |
|----------|---------|--------|
| Overall | 80% | 90% |
| Critical Paths | 100% | 100% |
| API Endpoints | 90% | 100% |
| Security Code | 100% | 100% |

### Critical Paths (100% Coverage Required)
- `backup_engine.py` - Backup execution logic
- `retention.py` - Backup retention calculations
- `encryption.py` - Backup encryption/decryption
- `remote_storage.py` - Remote storage operations
- All authentication/authorization code

---

## Backend Testing (pytest)

### Running Backend Tests

```bash
cd backend
source .venv/bin/activate

# Run all tests
pytest

# Run with verbose output
pytest -v

# Run specific test file
pytest tests/test_backup_engine.py

# Run specific test
pytest tests/test_backup_engine.py::test_run_backup_volume

# Run with coverage report
pytest --cov=app --cov-report=html

# Generate JUnit XML for CI
pytest --junitxml=results.xml
```

### Test Categories

Tests are categorized using pytest markers:

```python
@pytest.mark.unit          # Fast, isolated tests
@pytest.mark.integration   # Tests requiring external dependencies
@pytest.mark.slow          # Long-running tests
@pytest.mark.security      # Security-focused tests
```

Run by category:
```bash
pytest -m unit           # Only unit tests
pytest -m "not slow"     # Skip slow tests
pytest -m security       # Only security tests
```

### Key Fixtures

```python
# conftest.py - Main test fixtures

@pytest.fixture
async def db_session():
    """Provides a clean database session for each test."""
    
@pytest.fixture
def mock_docker_client():
    """Mocked Docker client for isolated testing."""
    
@pytest.fixture
def sample_backup_target():
    """Pre-configured backup target for testing."""
```

### Example Test

```python
@pytest.mark.asyncio
async def test_backup_fails_when_docker_unavailable(
    db_session: AsyncSession,
    mock_docker_client: Mock
):
    # Arrange
    mock_docker_client.side_effect = DockerException("Docker not available")
    
    # Act
    result = await backup_engine.run_backup(backup_id=1)
    
    # Assert
    assert result is False
    backup = await db_session.get(Backup, 1)
    assert backup.status == BackupStatus.FAILED
```

---

## Frontend Testing (Vitest)

### Running Frontend Tests

```bash
cd frontend

# Run all tests
npm test

# Run once (no watch mode)
npm test -- --run

# Run specific test file
npx vitest src/pages/__tests__/Backups.test.tsx

# Run tests matching pattern
npx vitest --run -t "should display error"

# Run with coverage
npm run test:coverage

# Debug mode
npx vitest --inspect-brk
```

### MSW Mock Handlers

API requests are mocked using MSW (Mock Service Worker):

```typescript
// test/mocks/handlers.ts
import { http, HttpResponse } from 'msw'

export const handlers = [
  http.get('/api/v1/docker/volumes', () => {
    return HttpResponse.json([
      { name: 'test-volume', driver: 'local' }
    ])
  }),
  
  http.post('/api/v1/backups', async ({ request }) => {
    const body = await request.json()
    return HttpResponse.json({ id: 1, ...body })
  }),
]
```

### Example Test

```typescript
import { render, screen, waitFor } from '@testing-library/react'
import { describe, it, expect } from 'vitest'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import Backups from '../Backups'

describe('Backups Page', () => {
  it('should display containers in first tab', async () => {
    // Arrange
    const queryClient = new QueryClient({
      defaultOptions: { queries: { retry: false } }
    })

    // Act
    render(
      <QueryClientProvider client={queryClient}>
        <Backups />
      </QueryClientProvider>
    )

    // Assert
    await waitFor(() => {
      expect(screen.getByText('nginx')).toBeInTheDocument()
    })
  })
})
```

---

## Manual Testing Checklists

### Level 1: Smoke Tests (5 minutes)

Quick verification that the app starts and basic features work.

- [ ] App starts without errors: `docker-compose up -d`
- [ ] Frontend loads: http://localhost:8080
- [ ] Backend health check: `curl http://localhost:8000/api/v1/docker/health`
- [ ] Docker volumes are listed in the UI
- [ ] Docker containers are listed in the UI

### Level 2: Functional Tests (15 minutes)

Test core backup functionality.

**Backup Target Creation:**
- [ ] Create a new volume backup target
- [ ] Create a new path backup target  
- [ ] Create a new stack backup target
- [ ] Enable/disable targets
- [ ] Delete a target

**Manual Backup:**
- [ ] Trigger a manual backup
- [ ] Monitor backup progress in real-time
- [ ] Verify backup file is created
- [ ] Check backup appears in history

**Backup Verification:**
- [ ] Download a backup file
- [ ] Verify tar.gz can be extracted
- [ ] Compare extracted content with original

**Restore Operation:**
- [ ] Restore a backup (to original or new location)
- [ ] Verify restored data matches original

### Level 3: Stress Tests (30 minutes)

**Large Data:**
- [ ] Backup a volume with 1GB+ of data
- [ ] Backup many small files (10,000+ files)
- [ ] Monitor memory usage during backup

**Concurrent Operations:**
- [ ] Run 3+ backups simultaneously
- [ ] Verify semaphore limits concurrent backups
- [ ] Check no data corruption

**Long-Running Containers:**
- [ ] Backup volume attached to actively-writing container
- [ ] Test stop-before-backup option
- [ ] Verify container restart after backup

### Level 4: Failure/Recovery Tests (20 minutes)

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

## Security Testing

Special attention to:

- **Path Traversal**: Test `../../../etc/passwd` scenarios
- **SQL Injection**: Test malicious input in database queries
- **Command Injection**: Test shell command sanitization
- **Input Validation**: Test boundary conditions and invalid input
- **Authentication**: Test unauthorized access attempts
- **File Operations**: Test secure file handling

### Example Security Test

```python
@pytest.mark.security
async def test_path_traversal_prevented():
    """Ensure backup paths cannot escape allowed directories."""
    response = await client.post("/api/v1/targets", json={
        "name": "malicious",
        "target_type": "path",
        "path": "../../../etc/passwd"
    })
    assert response.status_code == 400
```

---

## Debugging Tests

### Backend Debugging

```bash
# Run with print statements visible
pytest -v -s test_backup_engine.py::test_run_backup_volume

# Use pytest-xdist for parallel execution
pytest -n auto

# Stop at first failure
pytest -x

# Show local variables on failure
pytest -l
```

### Frontend Debugging

```bash
# Run specific test file
npx vitest src/pages/__tests__/Backups.test.tsx

# Run tests matching pattern
npx vitest --run -t "should handle errors"

# Debug mode with inspect
npx vitest --inspect-brk
```

### Common Issues

1. **Async/Await Issues**: Ensure all async operations are properly awaited
2. **Mock Cleanup**: Reset mocks between tests using `beforeEach`
3. **Database State**: Use transactions that can be rolled back
4. **WebSocket Mocks**: Properly mock WebSocket connections
5. **File System**: Use temporary directories for file operations

---

## Test Best Practices

### General Guidelines

- **Test Behavior, Not Implementation**: Focus on what the code does, not how
- **Use Descriptive Names**: Test names should explain what is being tested
- **Follow AAA Pattern**: Arrange, Act, Assert
- **Keep Tests Independent**: Each test should be able to run in isolation
- **Mock External Dependencies**: Don't rely on external services

### Naming Conventions

```python
# Backend: test_<action>_<expected_result>
def test_backup_fails_when_volume_not_found():
    ...

def test_retention_keeps_correct_number_of_backups():
    ...
```

```typescript
// Frontend: should <expected behavior>
it('should display error message when backup fails', ...)
it('should show loading spinner while fetching data', ...)
```

---

## Continuous Integration

Tests run automatically on:

- **Push to main/develop branches**
- **Pull requests**
- **Scheduled runs** (nightly)

### GitHub Actions Workflow

```yaml
# .github/workflows/test.yml
jobs:
  backend-tests:
    runs-on: ubuntu-latest
    steps:
      - name: Run tests with coverage
        run: pytest --cov=app --cov-fail-under=80
        
  frontend-tests:
    runs-on: ubuntu-latest
    steps:
      - name: Run tests with coverage
        run: npm run test:coverage -- --run
```

---

## Pre-Release Regression Testing

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

---

## Additional Resources

- [pytest Documentation](https://docs.pytest.org/)
- [Vitest Documentation](https://vitest.dev/)
- [React Testing Library](https://testing-library.com/docs/react-testing-library/intro/)
- [MSW Documentation](https://mswjs.io/docs/)
- [FastAPI Testing](https://fastapi.tiangolo.com/tutorial/testing/)
