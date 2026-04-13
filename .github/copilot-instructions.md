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

## Pre-Commit Checklist (MANDATORY)

**ALWAYS run these checks before EVERY commit:**

### Backend (Python)
```bash
cd backend
python -m ruff check app/
python -m ruff format --check app/
# If format check fails, run:
python -m ruff format app/
```

### Frontend (TypeScript/React)
```bash
cd frontend
npx tsc --noEmit
npx eslint src/
```

**Do NOT commit without passing all linting checks!**

When implementing new features, consider:
1. Impact on existing backup processes
2. Resource usage and performance implications
3. User experience and workflow efficiency
4. Error scenarios and recovery procedures
5. Documentation and help text requirements

## Commit Message Convention

This project uses **Conventional Commits** for automatic versioning and changelog generation.
Every commit message MUST follow this format:

```
<type>(<scope>): <description>

[optional body]

[optional footer(s)]
```

### Types

| Type | Purpose | Version Impact |
|------|---------|----------------|
| `feat` | New feature or capability | **Minor** bump (0.X.0) |
| `fix` | Bug fix | **Patch** bump (0.0.X) |
| `docs` | Documentation only | Patch bump |
| `style` | Formatting, whitespace, no code change | Patch bump |
| `refactor` | Code restructuring, no behavior change | Patch bump |
| `perf` | Performance improvement | Patch bump |
| `test` | Adding or updating tests | Patch bump |
| `chore` | Build, tooling, dependencies | Patch bump |
| `ci` | CI/CD pipeline changes | Patch bump |

### Breaking Changes → Major Bump

A breaking change triggers a **Major** version bump (X.0.0). Mark it with either:

- An `!` after the type/scope: `feat!: remove legacy API`
- A `BREAKING CHANGE:` footer in the commit body

### Scopes

- `api` – Backend API routes/endpoints
- `ui` – Frontend components/pages
- `auth` – Authentication/security
- `db` – Database/models/migrations
- `docker` – Docker/deployment
- `backup` – Backup engine/operations
- `storage` – Remote storage (S3, FTP, WebDAV)
- `scheduler` – APScheduler/task scheduling
- `ws` – WebSocket connections

### Rules

- Type and description are **required**
- Scope is optional but encouraged
- Description must be lowercase, imperative mood ("add" not "added" or "adds")
- No period at the end of the description
- Body and footer are optional
- Use `!` or `BREAKING CHANGE:` only for genuinely incompatible changes

### Examples

```
feat(backup): add incremental backup support
fix(ui): correct progress bar not updating
refactor(storage): simplify S3 upload handler
docs: update README with Docker Compose examples
feat!: redesign backup scheduling API
chore(deps): update FastAPI to 0.115
ci: add ARM64 Docker build
perf(db): add index on backup.created_at
```

## Language

- Commit messages in **English**
- Code comments in **English**