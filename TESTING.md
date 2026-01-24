# Testing Guide for DockerVault

This document provides comprehensive information about testing DockerVault, including setup, running tests, coverage requirements, and best practices.

## 🧪 Test Structure

### Backend Tests (Python)

```
backend/
├── tests/
│   ├── conftest.py              # Test configuration and fixtures
│   ├── test_backup_engine.py    # Backup engine functionality
│   ├── test_api_backups.py      # Backup API endpoints
│   ├── test_docker_client.py    # Docker integration
│   ├── test_database.py         # Database models and operations
│   └── test_scheduler.py        # Backup scheduling
├── pytest.ini                   # Pytest configuration
└── requirements-dev.txt         # Test dependencies
```

### Frontend Tests (TypeScript/React)

```
frontend/
├── src/
│   ├── pages/__tests__/         # Page component tests
│   ├── store/__tests__/         # State management tests
│   ├── api/__tests__/           # API layer tests
│   └── test/                    # Test utilities
│       ├── setup.ts             # Test setup configuration
│       └── mocks/               # Mock Service Worker handlers
├── vitest.config.ts             # Vitest configuration
└── package.json                 # Test scripts and dependencies
```

## 🚀 Quick Start

### Run All Tests

```bash
# Make test script executable (first time only)
chmod +x test.sh

# Run all tests with coverage
./test.sh

# Run with options
./test.sh --help                 # Show help
./test.sh --backend-only         # Only backend tests
./test.sh --frontend-only        # Only frontend tests
./test.sh --no-coverage          # Skip coverage reports
./test.sh --verbose              # Verbose output
```

### Backend Tests Only

```bash
cd backend

# Create virtual environment (first time)
python -m venv venv
source venv/bin/activate  # On Windows: venv\Scripts\activate

# Install dependencies
pip install -r requirements.txt
pip install -r requirements-dev.txt

# Run tests
pytest                                    # Basic run
pytest -v                                 # Verbose output
pytest --cov=app                          # With coverage
pytest --cov=app --cov-report=html       # HTML coverage report
pytest -k "test_backup"                  # Run specific tests
pytest --tb=short                         # Short traceback format
```

### Frontend Tests Only

```bash
cd frontend

# Install dependencies (first time)
npm install

# Run tests
npm test                          # Interactive mode
npm test -- --run                 # Run once
npm run test:coverage             # With coverage
npm run test:ui                   # UI mode (browser)
vitest --watch                    # Watch mode
```

## 📊 Coverage Requirements

### Minimum Coverage Thresholds

- **Overall**: 80% minimum
- **Critical paths**: 100% (backup/restore operations)
- **Security functions**: 100% (input validation, path sanitization)
- **API endpoints**: All status codes tested
- **Error handling**: All exception paths covered

### Coverage Reports

After running tests with coverage:

- **Backend**: `backend/htmlcov/index.html`
- **Frontend**: `frontend/coverage/index.html`

```bash
# Open coverage reports
open backend/htmlcov/index.html      # macOS
open frontend/coverage/index.html     # macOS

xdg-open backend/htmlcov/index.html   # Linux
xdg-open frontend/coverage/index.html # Linux
```

## 🧩 Test Categories

### Unit Tests

Test individual functions, classes, and components in isolation.

```python
# Backend example
@pytest.mark.asyncio
async def test_backup_creation():
    engine = BackupEngine()
    backup = await engine.create_backup(target, BackupType.FULL)
    assert backup.status == BackupStatus.PENDING
```

```typescript
// Frontend example
it('should render backup status correctly', () => {
  render(<BackupRow backup={mockBackup} />)
  expect(screen.getByText('completed')).toBeInTheDocument()
})
```

### Integration Tests

Test API endpoints and component interactions.

```python
@pytest.mark.asyncio
async def test_create_backup_api(async_client):
    response = await async_client.post("/api/v1/backups", json={
        "target_id": 1,
        "backup_type": "full"
    })
    assert response.status_code == 201
```

### Security Tests

Test input validation, authentication, and security measures.

```python
async def test_path_traversal_prevention(async_client):
    response = await async_client.post("/api/v1/backups", json={
        "volume_name": "../../../etc/passwd"
    })
    assert response.status_code == 400
```

## 🛠️ Test Utilities and Fixtures

### Backend Fixtures

```python
@pytest.fixture
async def db_session():
    """Provide clean database session for each test."""
    async with test_db() as session:
        yield session
        await session.rollback()

@pytest.fixture
def mock_docker_client():
    """Mock Docker client for testing."""
    with patch('app.docker_client.docker_client') as mock:
        mock.volumes.list.return_value = []
        yield mock
```

### Frontend Mocks

```typescript
// MSW handlers for API mocking
export const handlers = [
  http.get('/api/v1/backups', () => {
    return HttpResponse.json(mockBackups)
  }),
  http.post('/api/v1/backups', async ({ request }) => {
    const body = await request.json()
    return HttpResponse.json(newBackup, { status: 201 })
  }),
]
```

## 🐛 Debugging Tests

### Backend Debugging

```bash
# Run specific test with debugging
pytest -v -s test_backup_engine.py::test_run_backup_volume

# Use pytest-xdist for parallel execution
pytest -n auto

# Generate JUnit XML for CI
pytest --junitxml=results.xml
```

### Frontend Debugging

```bash
# Run specific test file
vitest src/pages/__tests__/Backups.test.tsx

# Run tests matching pattern
vitest --run -t "should handle errors"

# Debug mode with inspect
vitest --inspect-brk
```

### Common Issues

1. **Async/Await Issues**: Ensure all async operations are properly awaited
2. **Mock Cleanup**: Reset mocks between tests using `beforeEach`
3. **Database State**: Use transactions that can be rolled back
4. **WebSocket Mocks**: Properly mock WebSocket connections
5. **File System**: Use temporary directories for file operations

## 🔍 Test Best Practices

### General Guidelines

- **Test Behavior, Not Implementation**: Focus on what the code does, not how
- **Use Descriptive Names**: Test names should explain what is being tested
- **Follow AAA Pattern**: Arrange, Act, Assert
- **Keep Tests Independent**: Each test should be able to run in isolation
- **Mock External Dependencies**: Don't rely on external services

### Backend Best Practices

```python
# ✅ Good
@pytest.mark.asyncio
async def test_backup_fails_when_docker_unavailable():
    # Arrange
    mock_docker.side_effect = DockerException("Docker not available")
    
    # Act
    result = await backup_engine.run_backup(backup_id)
    
    # Assert
    assert result is False
    assert backup.status == BackupStatus.FAILED

# ❌ Bad
async def test_backup():
    result = await backup_engine.run_backup(1)
    assert result
```

### Frontend Best Practices

```typescript
// ✅ Good
it('should display error message when backup creation fails', async () => {
  // Arrange
  server.use(
    http.post('/api/v1/backups', () => {
      return new HttpResponse(null, { status: 500 })
    })
  )
  
  // Act
  render(<BackupForm />, { wrapper: createWrapper() })
  await user.click(screen.getByRole('button', { name: /create/i }))
  
  // Assert
  await waitFor(() => {
    expect(screen.getByText(/error creating backup/i)).toBeInTheDocument()
  })
})

// ❌ Bad
it('should work', () => {
  render(<BackupForm />)
  expect(screen.getByText('form')).toBeInTheDocument()
})
```

## 🚀 Continuous Integration

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

### Coverage Upload

Coverage reports are automatically uploaded to Codecov and available as artifacts.

## 📋 Testing Checklist

Before submitting a PR, ensure:

- [ ] All tests pass locally
- [ ] Coverage meets 80% minimum threshold
- [ ] New features have corresponding tests
- [ ] Security-sensitive code has 100% coverage
- [ ] API endpoints test all status codes
- [ ] Error handling is thoroughly tested
- [ ] Mock external dependencies appropriately
- [ ] Tests follow naming conventions
- [ ] Test documentation is updated

## 🛡️ Security Testing

Special attention to:

- **Path Traversal**: Test `../../../etc/passwd` scenarios
- **SQL Injection**: Test malicious input in database queries
- **Command Injection**: Test shell command sanitization
- **Input Validation**: Test boundary conditions and invalid input
- **Authentication**: Test unauthorized access attempts
- **File Operations**: Test secure file handling

## 📚 Additional Resources

- [pytest Documentation](https://docs.pytest.org/)
- [Vitest Documentation](https://vitest.dev/)
- [React Testing Library](https://testing-library.com/docs/react-testing-library/intro/)
- [MSW Documentation](https://mswjs.io/docs/)
- [FastAPI Testing](https://fastapi.tiangolo.com/tutorial/testing/)

## 🤝 Contributing Tests

When adding new tests:

1. **Follow existing patterns** in the codebase
2. **Add tests for new features** before implementing
3. **Update documentation** if test structure changes
4. **Ensure tests are deterministic** and don't rely on timing
5. **Use appropriate test categories** (unit, integration, e2e)

For questions about testing, please refer to the [Contributing Guide](CONTRIBUTING.md) or open an issue.
