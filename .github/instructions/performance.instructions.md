---
applyTo: "**/*.py,**/*.ts,**/*.tsx,**/docker-compose*.yml"
description: "Performance optimization guidelines for DockerVault"
---

# Performance Guidelines

## General Performance Principles

- Optimize for the most common use cases
- Measure before optimizing - use profiling tools
- Consider memory usage alongside CPU performance
- Implement proper caching strategies
- Use async patterns for I/O-bound operations
- Monitor performance metrics in production

## Backend Performance (Python/FastAPI)

- Use async/await for all I/O operations
- Implement connection pooling for databases and external services
- Use streaming for large file operations
- Cache frequently accessed data with appropriate TTL
- Use background tasks for long-running operations
- Implement proper pagination for large data sets

### Database Performance

- Use database indexes on frequently queried columns
- Implement proper query optimization
- Use connection pooling and connection reuse
- Batch database operations where possible
- Monitor query execution times and optimize slow queries
- Use database-specific optimizations for SQLite

### File Operations

- Use async file I/O for better concurrency
- Implement streaming for large files to manage memory usage
- Use compression to reduce storage and transfer overhead
- Implement proper progress tracking for long operations
- Use parallel processing for independent file operations
- Optimize temporary file usage and cleanup

### Docker Operations

- Use Docker SDK efficiently with proper connection management
- Implement batching for multiple container operations
- Monitor Docker daemon resource usage
- Use appropriate timeouts for Docker operations
- Implement proper cleanup to prevent resource leaks
- Cache Docker image information where appropriate

## Frontend Performance (React/TypeScript)

- Use React.memo for expensive components
- Implement code splitting with React.lazy
- Optimize bundle size with tree shaking
- Use proper dependency arrays in hooks
- Implement virtual scrolling for large lists
- Optimize re-renders with useMemo and useCallback

### State Management Performance

- Use React Query for efficient server state caching
- Implement proper cache invalidation strategies
- Use optimistic updates for better perceived performance
- Minimize unnecessary state updates and re-renders
- Use Zustand selectors to prevent unnecessary subscriptions

### Network Performance

- Implement request deduplication
- Use proper HTTP caching headers
- Implement retry logic with exponential backoff
- Batch API requests where possible
- Use WebSocket efficiently for real-time updates
- Implement offline support and sync strategies

### UI Performance

- Implement lazy loading for images and components
- Use proper loading states and skeleton screens
- Optimize CSS and avoid layout thrashing
 - Implement proper error boundaries to prevent cascading failures
- Use debouncing for user input handling

## Docker Performance

- Use multi-stage builds to reduce image size
- Optimize layer caching for faster builds
- Use appropriate resource limits (CPU, memory)
- Implement proper health checks with reasonable intervals
- Use volumes for persistent data to avoid copy overhead
- Optimize Docker networking for service communication

## System Performance

- Monitor system resources (CPU, memory, disk, network)
- Implement proper logging that doesn't impact performance
- Use appropriate log levels and rotation policies
- Monitor backup operation performance and resource usage
- Implement alerting for performance degradation
- Use appropriate scheduling for background operations

## Backup Performance

- Implement incremental backup strategies
- Use compression to reduce storage and network overhead
- Implement parallel processing for independent backup tasks
- Monitor backup duration and optimize bottlenecks
- Use appropriate chunk sizes for large file operations
- Implement resume capability for interrupted operations

## Remote Storage Performance

- Use connection pooling for remote storage operations
- Implement proper retry mechanisms with backoff
- Use multipart uploads for large files
- Implement progress tracking and cancellation
- Optimize network usage with compression and deduplication
- Cache remote storage metadata appropriately

## Monitoring and Profiling

- Use application performance monitoring (APM) tools
- Implement custom metrics for business-critical operations
- Monitor resource usage patterns over time
- Use profiling tools to identify performance bottlenecks
- Implement performance regression testing
- Set up alerting for performance threshold violations

## Scalability Considerations

- Design for horizontal scaling where possible
- Use stateless application design patterns
- Implement proper load balancing strategies
- Use appropriate queueing for background tasks
- Plan for data growth and archival strategies
- Consider distributed caching solutions for scale

## Development Performance

- Use development tools that support hot reloading
- Optimize build times with proper caching
- Use appropriate test strategies to minimize test execution time
- Implement parallel test execution where possible
- Use efficient development environment setup
- Monitor and optimize CI/CD pipeline performance