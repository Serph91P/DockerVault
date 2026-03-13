#!/bin/bash

# Test runner script for DockerVault
# Runs both backend and frontend tests with coverage reporting

set -e  # Exit on any error

echo "🧪 Running DockerVault Test Suite"
echo "================================="

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Function to print colored output
print_status() {
    echo -e "${GREEN}✓${NC} $1"
}

print_warning() {
    echo -e "${YELLOW}⚠${NC} $1"
}

print_error() {
    echo -e "${RED}✗${NC} $1"
}

# Check if we're in the right directory
if [ ! -f "docker-compose.yml" ]; then
    print_error "Please run this script from the project root directory"
    exit 1
fi

# Parse command line arguments
BACKEND_ONLY=false
FRONTEND_ONLY=false
COVERAGE=true
VERBOSE=false
HELP=false

while [[ $# -gt 0 ]]; do
    case $1 in
        --backend-only)
            BACKEND_ONLY=true
            shift
            ;;
        --frontend-only)
            FRONTEND_ONLY=true
            shift
            ;;
        --no-coverage)
            COVERAGE=false
            shift
            ;;
        --verbose|-v)
            VERBOSE=true
            shift
            ;;
        --help|-h)
            HELP=true
            shift
            ;;
        *)
            print_error "Unknown option $1"
            exit 1
            ;;
    esac
done

if [ "$HELP" = true ]; then
    echo "Usage: $0 [OPTIONS]"
    echo ""
    echo "Options:"
    echo "  --backend-only    Run only backend tests"
    echo "  --frontend-only   Run only frontend tests"
    echo "  --no-coverage     Skip coverage reporting"
    echo "  --verbose, -v     Show verbose output"
    echo "  --help, -h        Show this help message"
    echo ""
    echo "Examples:"
    echo "  $0                    # Run all tests with coverage"
    echo "  $0 --backend-only     # Run only backend tests"
    echo "  $0 --no-coverage -v   # Run all tests without coverage, verbose"
    exit 0
fi

# Initialize test results
BACKEND_PASSED=false
FRONTEND_PASSED=false

# Backend Tests
if [ "$FRONTEND_ONLY" = false ]; then
    echo ""
    echo "🐍 Running Backend Tests (Python)"
    echo "--------------------------------"
    
    cd backend
    
    # Check if virtual environment exists
    if [ ! -d "venv" ]; then
        print_warning "Virtual environment not found, creating one..."
        python -m venv venv
        source venv/bin/activate
        pip install -r requirements.txt
        pip install -r requirements-dev.txt
    else
        source venv/bin/activate
    fi
    
    # Install test dependencies if not already installed
    if ! pip list | grep -q pytest; then
        print_status "Installing test dependencies..."
        pip install -r requirements-dev.txt
    fi
    
    # Run backend tests
    print_status "Running pytest..."
    
    if [ "$COVERAGE" = true ]; then
        if [ "$VERBOSE" = true ]; then
            pytest -v --cov=app --cov-report=term-missing --cov-report=html:htmlcov --cov-fail-under=80
        else
            pytest --cov=app --cov-report=term-missing --cov-report=html:htmlcov --cov-fail-under=80 --tb=short
        fi
    else
        if [ "$VERBOSE" = true ]; then
            pytest -v
        else
            pytest --tb=short
        fi
    fi
    
    if [ $? -eq 0 ]; then
        BACKEND_PASSED=true
        print_status "Backend tests passed!"
        if [ "$COVERAGE" = true ]; then
            print_status "Coverage report generated in backend/htmlcov/"
        fi
    else
        print_error "Backend tests failed!"
    fi
    
    cd ..
fi

# Frontend Tests
if [ "$BACKEND_ONLY" = false ]; then
    echo ""
    echo "⚛️  Running Frontend Tests (React/TypeScript)"
    echo "--------------------------------------------"
    
    cd frontend
    
    # Check if node_modules exists
    if [ ! -d "node_modules" ]; then
        print_warning "Dependencies not installed, running npm install..."
        npm install
    fi
    
    # Run frontend tests
    print_status "Running vitest..."
    
    if [ "$COVERAGE" = true ]; then
        if [ "$VERBOSE" = true ]; then
            npm run test:coverage -- --run --reporter=verbose
        else
            npm run test:coverage -- --run
        fi
    else
        if [ "$VERBOSE" = true ]; then
            npm test -- --run --reporter=verbose
        else
            npm test -- --run
        fi
    fi
    
    if [ $? -eq 0 ]; then
        FRONTEND_PASSED=true
        print_status "Frontend tests passed!"
        if [ "$COVERAGE" = true ]; then
            print_status "Coverage report generated in frontend/coverage/"
        fi
    else
        print_error "Frontend tests failed!"
    fi
    
    cd ..
fi

# Test Summary
echo ""
echo "📊 Test Summary"
echo "==============="

if [ "$FRONTEND_ONLY" = false ]; then
    if [ "$BACKEND_PASSED" = true ]; then
        print_status "Backend: PASSED"
    else
        print_error "Backend: FAILED"
    fi
fi

if [ "$BACKEND_ONLY" = false ]; then
    if [ "$FRONTEND_PASSED" = true ]; then
        print_status "Frontend: PASSED"
    else
        print_error "Frontend: FAILED"
    fi
fi

# Overall result
if ([ "$FRONTEND_ONLY" = false ] && [ "$BACKEND_PASSED" = false ]) || 
   ([ "$BACKEND_ONLY" = false ] && [ "$FRONTEND_PASSED" = false ]); then
    echo ""
    print_error "Some tests failed. Please check the output above."
    exit 1
else
    echo ""
    print_status "All tests passed! 🎉"
    
    if [ "$COVERAGE" = true ]; then
        echo ""
        echo "📈 Coverage Reports:"
        [ "$FRONTEND_ONLY" = false ] && echo "  Backend:  backend/htmlcov/index.html"
        [ "$BACKEND_ONLY" = false ] && echo "  Frontend: frontend/coverage/index.html"
    fi
    
    exit 0
fi
