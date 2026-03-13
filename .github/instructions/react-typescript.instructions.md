<!-- Based on: https://github.com/github/awesome-copilot/blob/main/instructions/reactjs.instructions.md -->
---
applyTo: "**/*.tsx,**/*.ts,**/*.jsx,**/*.js,frontend/**/*.css"
description: "React TypeScript development standards for DockerVault frontend"
---

# React TypeScript Development Guidelines

## Project Context

- React 19+ with TypeScript for type safety
- Vite for fast development and optimized builds
- TailwindCSS for utility-first styling
- React Query (TanStack Query) for server state management
- Zustand for client state management

## Component Architecture

- Use functional components with hooks as the primary pattern
- Implement component composition over inheritance
- Organize components by feature or domain for scalability
- Separate presentational and container components clearly
- Use custom hooks for reusable stateful logic
- Keep components small and focused on a single concern

## TypeScript Integration

- Use TypeScript interfaces for props, state, and API responses
- Define proper types for event handlers and refs
- Use strict mode in tsconfig.json for maximum type safety
- Leverage React's built-in types (React.FC, React.ComponentProps)
- Create union types for component variants and states
- Use generic types for reusable components

## State Management

- Use React Query for server state and caching
- Use Zustand for global client state (UI state, user preferences)
- Use useState for local component state
- Implement useReducer for complex local state logic
- Use useContext sparingly for theme/auth context

## Styling with TailwindCSS

- Use Tailwind utility classes for consistent styling
- Create reusable component classes for common patterns
- Implement responsive design with mobile-first approach
- Use Tailwind's color system and spacing scale consistently
- Combine with clsx/tailwind-merge for conditional classes

## Performance Optimization

- Use React.memo for component memoization when appropriate
- Implement code splitting with React.lazy and Suspense
- Use useMemo and useCallback judiciously to prevent unnecessary re-renders
- Optimize bundle size with tree shaking and dynamic imports
- Implement virtual scrolling for large data lists

## Data Fetching with React Query

- Use React Query for all server state management
- Implement proper loading, error, and success states
- Use optimistic updates for better user experience
- Implement proper caching strategies with stale-while-revalidate
- Handle offline scenarios and network errors gracefully

## Error Handling

- Implement Error Boundaries for component-level error handling
- Use proper error states in data fetching
- Provide meaningful error messages to users
- Log errors appropriately for debugging
- Handle async errors in effects and event handlers

## Form Handling

- Use controlled components for form inputs
- Implement proper form validation with TypeScript types
- Handle form submission and error states appropriately
- Use React Hook Form for complex forms
- Implement accessibility features for forms (labels, ARIA attributes)

## Testing Guidelines

- Write component tests using React Testing Library
- Test component behavior, not implementation details
- Mock external dependencies and API calls appropriately
- Test accessibility features and keyboard navigation
- Use MSW (Mock Service Worker) for API mocking

## Real-time Features

- Use WebSocket connections for backup progress updates
- Implement proper connection management and reconnection
- Handle WebSocket errors and connection states
- Update UI reactively based on real-time data
- Provide visual indicators for connection status

## Security Considerations

- Sanitize user inputs to prevent XSS attacks
- Validate and escape data before rendering
- Use HTTPS for all API communications
- Implement proper authentication state management
- Avoid storing sensitive data in localStorage

## Accessibility

- Use semantic HTML elements appropriately
- Implement proper ARIA attributes and roles
- Ensure keyboard navigation works for all interactive elements
- Provide alt text for images and descriptive text for icons
- Implement proper color contrast ratios
- Test with screen readers

## Code Organization

- Organize components by feature rather than type
- Use index files for clean imports
- Separate API calls into service files
- Keep constants and types in separate files
- Use absolute imports for cleaner import paths