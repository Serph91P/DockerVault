<!-- Based on: https://github.com/github/awesome-copilot/blob/main/instructions/python.instructions.md -->
---
applyTo: "**/*.py"
description: "Python development standards for DockerVault backend"
---

# Python Development Guidelines

## Code Style and Formatting

- Follow **PEP 8** style guide for Python
- Use 4 spaces for indentation
- Keep lines under 88 characters (Black formatter standard)
- Use type hints for all function parameters and return values
- Write clear and concise docstrings following PEP 257 conventions

## FastAPI Specific Guidelines

- Use Pydantic models for request/response validation
- Implement proper dependency injection patterns
- Use async/await for all I/O operations
- Define clear API endpoint groupings with routers
- Include comprehensive OpenAPI documentation with examples

## Database and SQLAlchemy

- Use async SQLAlchemy patterns with aiosqlite
- Define clear database models with proper relationships
- Implement database migrations for schema changes
- Use connection pooling appropriately
- Handle database errors gracefully with proper rollbacks

## Docker SDK Integration

- Use async patterns with aiodocker or docker-py
- Implement proper connection management and cleanup
- Handle Docker daemon unavailability scenarios
- Use context managers for resource management
- Log Docker operations with appropriate detail levels

## Error Handling and Logging

- Use structured logging with JSON format for production
- Implement custom exception classes for domain-specific errors
- Log all backup operations with trace IDs for debugging
- Handle edge cases like disk space, permissions, and network issues
- Provide meaningful error messages for API responses

## Async Programming

- Use asyncio properly for concurrent operations
- Implement proper cancellation handling for long-running tasks
- Use semaphores and rate limiting for external API calls
- Handle backpressure in streaming operations
- Test async code with pytest-asyncio

## Testing Guidelines

- Write unit tests for all business logic functions
- Use pytest fixtures for database and Docker test setup
- Mock external dependencies (Docker daemon, remote storage)
- Test error scenarios and edge cases
- Include performance tests for file operations

## Security Considerations

- Validate all input data with Pydantic models
- Use environment variables for sensitive configuration
- Implement proper authentication and authorization
- Sanitize file paths to prevent directory traversal
- Use secure defaults for all configuration options

## Performance Optimization

- Use connection pooling for database operations
- Implement streaming for large file operations
- Use background tasks for long-running operations
- Cache frequently accessed data appropriately
- Profile code to identify performance bottlenecks

## Code Organization

- Separate business logic from API endpoints
- Use dependency injection for testability
- Group related functionality in modules
- Keep configuration in separate files
- Follow domain-driven design principles