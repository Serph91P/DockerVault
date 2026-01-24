<!-- Based on: https://github.com/github/awesome-copilot/blob/main/instructions/containerization-docker-best-practices.instructions.md -->
---
applyTo: "**/Dockerfile,**/docker-compose*.yml,**/docker-compose*.yaml"
description: "Docker containerization best practices for DockerVault"
---

# Docker Development Guidelines

## Multi-Stage Builds

- Use multi-stage builds to separate build and runtime dependencies
- Name build stages descriptively (AS build, AS production)
- Copy only necessary artifacts between stages
- Use different base images for build and runtime when appropriate

## Base Image Selection

- Use official, minimal base images (alpine variants when possible)
- Use specific version tags, avoid 'latest' in production
- Prefer language-specific official images (python:3.11-slim, node:18-alpine)
- Regularly update base images for security patches

## Layer Optimization

- Order instructions from least to most frequently changing
- Combine RUN commands to minimize layers
- Clean up package caches in the same RUN command
- Use .dockerignore to exclude unnecessary files from build context

## Security Best Practices

- Run containers as non-root user
- Create dedicated users for applications
- Use minimal base images to reduce attack surface
- Scan images for vulnerabilities regularly
- Never include secrets or credentials in image layers

## Configuration Management

- Use environment variables for runtime configuration
- Provide sensible defaults with ENV instructions
- Validate required environment variables at startup
- Use secrets management for sensitive data

## Health Checks and Monitoring

- Define HEALTHCHECK instructions for application monitoring
- Design health checks that verify actual functionality
- Use appropriate intervals and timeouts
- Implement both liveness and readiness checks

## Volume and Data Management

- Use named volumes for persistent data
- Never store persistent data in container's writable layer
- Implement proper backup strategies for volumes
- Use bind mounts sparingly and only for development

## Network Configuration

- Create custom networks for service isolation
- Use service discovery features of Docker Compose
- Implement proper network segmentation
- Document exposed ports with EXPOSE instruction

## Resource Management

- Set appropriate CPU and memory limits
- Monitor resource usage to tune limits
- Use resource quotas in orchestrated environments
- Implement proper logging strategies

## Development vs Production

- Use separate Docker Compose files for different environments
- Override configurations with environment-specific values
- Implement proper build optimization for production
- Use development-friendly settings for local development

## DockerVault Specific Guidelines

- Backend container should have access to Docker socket for backup operations
- Frontend should be served by nginx in production
- Use proper volume mounts for backup storage locations
- Implement proper cleanup procedures for temporary files
- Handle Docker daemon connection issues gracefully
- Use appropriate resource limits for backup operations