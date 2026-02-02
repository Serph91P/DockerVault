---
applyTo: "**/*"
description: "Code review standards and GitHub review guidelines for DockerVault"
---

# Code Review Guidelines

## General Code Review Principles

- Focus on code correctness, readability, and maintainability
- Provide constructive feedback with specific suggestions
- Consider the bigger picture: architecture, design patterns, and consistency
- Review for security vulnerabilities and performance implications
- Check that tests are adequate and meaningful
- Ensure documentation is updated when necessary

## What to Look For

### Code Quality

- Adherence to project coding standards and style guides
- Proper error handling and edge case coverage
- Clear and descriptive variable and function names
- Appropriate code organization and separation of concerns
- Elimination of code duplication and dead code
- Proper use of design patterns and architectural principles

### Functionality

- Code does what it's supposed to do
- Business logic is correct and complete
- Edge cases and error scenarios are handled
- Input validation is comprehensive
- Output formats match specifications
- Integration points work correctly

### Security

- Input sanitization and validation
- Proper authentication and authorization
- No hardcoded secrets or sensitive data
- Secure handling of file operations and paths
- Proper use of environment variables
- Adherence to security best practices

### Performance

- Efficient algorithms and data structures
- Appropriate use of caching and optimization
- Proper resource management and cleanup
- Async patterns used correctly
- No obvious performance bottlenecks
- Reasonable memory usage patterns

### Testing

- Adequate test coverage for new functionality
- Tests are meaningful and test the right things
- Test data is appropriate and realistic
- Mocking is used appropriately
- Tests are maintainable and not brittle
- Edge cases and error scenarios are tested

## Language-Specific Review Points

### Python Backend Reviews

- Type hints are used consistently
- Async/await patterns are used properly
- Database operations use proper transaction handling
- API endpoints follow REST conventions
- Error responses are consistent and informative
- Docker SDK usage is efficient and safe

### React Frontend Reviews

- Components follow React best practices
- State management is appropriate (local vs global)
- TypeScript types are comprehensive and accurate
- Accessibility considerations are addressed
- Performance optimizations are appropriate
- Error boundaries handle failures gracefully

### Docker Reviews

- Dockerfile follows multi-stage build practices
- Security best practices are implemented
- Image size is optimized
- Resource limits are appropriate
- Health checks are implemented
- Environment configuration is externalized

## Review Process

### Before Submitting a PR

- Self-review your code thoroughly
- Ensure all tests pass and coverage is adequate
- Update documentation as needed
- Write a clear PR description with context
- Link to relevant issues or requirements
- Consider the impact on existing functionality

### Conducting a Review

- Understand the context and requirements
- Review the code changes line by line
- Test the changes locally when appropriate
- Provide specific, actionable feedback
- Suggest alternatives when requesting changes
- Acknowledge good practices and improvements

### Review Comments

- Use clear, respectful language
- Explain the reasoning behind suggestions
- Provide code examples when helpful
- Use appropriate prefixes (nit, optional, blocking)
- Focus on the code, not the person
- Ask questions to understand intent when unclear

### GitHub Review Features

- Use GitHub's suggestion feature for small changes
- Mark conversations as resolved after addressing
- Use appropriate review status (Comment, Approve, Request Changes)
- Add reviewers with relevant expertise
- Use draft PRs for work-in-progress reviews
- Link PRs to issues and project boards

## DockerVault-Specific Review Areas

### Backup Operations

- Proper error handling for backup failures
- Progress tracking and user feedback
- Resource cleanup after operations
- Validation of backup integrity
- Proper handling of large files and volumes
- Scheduling and automation logic

### Docker Integration

- Safe Docker socket access
- Proper container lifecycle management
- Volume mounting and permissions
- Network configuration and security
- Resource limits and monitoring
- Error handling for Docker daemon issues

### User Interface

- Consistent user experience across features
- Proper loading states and error messages
- Accessibility and responsive design
- Real-time updates and WebSocket handling
- Form validation and user feedback
- Navigation and routing logic

### Configuration Management

- Environment variable usage
- Configuration validation
- Default values and documentation
- Security of sensitive configuration
- Deployment and environment differences
- Migration and upgrade procedures

## Common Issues to Watch For

- Hardcoded configuration values
- Missing error handling or overly broad exception catching
- Security vulnerabilities (path traversal, injection, etc.)
- Performance issues (N+1 queries, inefficient algorithms)
- Inconsistent code style or naming conventions
- Missing or inadequate tests
- Outdated or missing documentation
- Breaking changes without proper versioning
- Resource leaks or improper cleanup
- Race conditions in concurrent code

## Review Checklist

- [ ] Code follows project style and conventions
- [ ] Functionality is correct and complete
- [ ] Error handling is comprehensive
- [ ] Security best practices are followed
- [ ] Performance considerations are addressed
- [ ] Tests are adequate and meaningful
- [ ] Documentation is updated as needed
- [ ] No breaking changes without proper communication
- [ ] Configuration is externalized appropriately
- [ ] Resource usage is reasonable and monitored