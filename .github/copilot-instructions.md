---
description: "Main repository instructions for DockerVault - Docker Volume Backup solution"
---

# DockerVault Development Guidelines

DockerVault is an automated Docker backup solution with a modern web interface, built with Python FastAPI backend and React TypeScript frontend.

## Project Architecture

- **Backend**: Python FastAPI with SQLAlchemy, Docker SDK, APScheduler for task scheduling
- **Frontend**: React 19 + TypeScript, Vite build system, TailwindCSS styling
- **Database**: SQLite with async support via aiosqlite
- **Infrastructure**: Docker containerized, nginx for frontend serving
- **Storage**: Local Docker volumes with remote backup support (S3, FTP, WebDAV)

## Development Standards

### Code Quality
- Follow language-specific guidelines in [instructions/](./instructions/) directory
- Maintain consistent naming conventions across frontend and backend
- Write comprehensive tests for both API endpoints and React components
- Use TypeScript strict mode for frontend type safety
- Follow Python PEP 8 standards with type hints

### Architecture Principles
- Keep backend and frontend loosely coupled via REST API
- Use WebSocket for real-time backup progress updates
- Implement proper error handling and user feedback
- Design for scalability with async patterns
- Follow Docker best practices for containerization

### Security
- Never commit sensitive configuration or credentials
- Use environment variables for all configuration
- Implement proper input validation on both frontend and backend
- Follow security best practices for Docker containers
- Use non-root users in containers

### Testing Strategy
- Unit tests for backend services and utilities
- Integration tests for API endpoints
- Component tests for React components
- End-to-end tests for critical backup workflows
- Maintain test coverage above 80%

### Documentation
- Keep README.md updated with setup and usage instructions
- Document API endpoints with OpenAPI/Swagger
- Add inline documentation for complex business logic
- Update deployment guides for any infrastructure changes

## Specific Guidelines

### Backup System
- Implement robust error handling for backup operations
- Provide clear progress indicators and status messages
- Support resumable operations where possible
- Log all backup activities with appropriate detail levels

### Docker Integration
- Use Docker SDK for container and volume management
- Implement proper cleanup for failed operations
- Handle Docker daemon connection errors gracefully
- Support both local and remote Docker endpoints

### File Operations
- Use async file I/O patterns for better performance
- Implement proper stream processing for large files
- Handle disk space issues and quota limits
- Support compression and encryption options

### User Interface
- Provide intuitive backup configuration workflows
- Display real-time status and progress information
- Implement responsive design for various screen sizes
- Use consistent icons and terminology throughout

When implementing new features, consider:
1. Impact on existing backup processes
2. Resource usage and performance implications
3. User experience and workflow efficiency
4. Error scenarios and recovery procedures
5. Documentation and help text requirements