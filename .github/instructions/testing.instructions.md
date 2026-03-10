---
applyTo: "**/test_*.py,**/tests/**/*.py,**/*.test.ts,**/*.test.tsx,**/*.spec.ts,**/*.spec.tsx"
description: "Testing standards and practices for DockerVault"
---

# Testing Guidelines

## General Testing Principles

- Maintain test coverage above 80% for both frontend and backend
- Write tests that verify behavior, not implementation details
- Use descriptive test names that explain what is being tested
- Follow the AAA pattern: Arrange, Act, Assert
- Keep tests isolated and independent

## Backend Testing (Python)

- Use pytest for all Python testing
- Write unit tests for business logic and utilities
- Create integration tests for API endpoints
- Use pytest fixtures for database and Docker test setup
- Mock external dependencies (Docker daemon, remote storage, file system)
- Test async code properly with pytest-asyncio
- Include performance tests for file operations

### Database Testing

- Use in-memory SQLite for faster test execution
- Create fresh database instances for each test
- Use database transactions that can be rolled back
- Test database migrations and schema changes
- Verify proper error handling for database failures

### API Testing

- Test all HTTP status codes and response formats
- Verify input validation and error messages
- Test authentication and authorization scenarios
- Include tests for edge cases and boundary conditions
- Use TestClient from FastAPI for endpoint testing

### Docker Integration Testing

- Mock Docker SDK calls for unit tests
- Use real Docker daemon for integration tests where necessary
- Test container lifecycle management
- Verify proper cleanup of test containers and volumes
- Test error scenarios like Docker daemon unavailability

## Frontend Testing (React/TypeScript)

- Use React Testing Library for component testing
- Write tests that verify user interactions and behavior
- Mock API calls with MSW (Mock Service Worker)
- Test accessibility features and keyboard navigation
- Use Jest for test running and assertions

### Component Testing

- Test component rendering with different props
- Verify event handling and state changes
- Test conditional rendering and error states
- Include tests for loading and empty states
- Test responsive behavior where applicable

### Integration Testing

- Test complete user workflows
- Verify data flow between components
- Test real-time updates via WebSocket
- Include tests for error boundaries
- Test routing and navigation

### State Management Testing

- Test React Query cache behavior and invalidation
- Verify Zustand store updates and selectors
- Test optimistic updates and error handling
- Include tests for offline scenarios

## End-to-End Testing

- Test critical backup workflows from UI to completion
- Verify file system operations and backup integrity
- Test user authentication and session management
- Include tests for error recovery and retry mechanisms
- Test backup scheduling and automated operations

## Test Data Management

- Use factories or builders for test data creation
- Keep test data minimal and focused
- Use realistic but anonymized test data
- Clean up test files and containers after tests
- Avoid using production data in tests

## Performance Testing

- Include load tests for backup operations
- Test memory usage during large file operations
- Verify proper resource cleanup after operations
- Test concurrent backup scenarios
- Monitor test execution time and optimize slow tests

## Security Testing

- Test input validation and sanitization
- Verify authentication and authorization controls
- Test for common security vulnerabilities
- Include tests for file path traversal prevention
- Test error messages don't leak sensitive information

## Test Environment Setup

- Use Docker containers for consistent test environments
- Include database seeding scripts for integration tests
- Set up CI/CD pipeline to run all tests automatically
- Use environment variables for test configuration
- Implement parallel test execution where possible