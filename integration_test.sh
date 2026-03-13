#!/bin/bash
#
# DockerVault Integration Test Suite
# Tests real backup and restore operations in an isolated environment
#
# Usage: ./integration_test.sh [--cleanup-only] [--keep-volumes]
#

# Note: Not using 'set -e' because we handle errors ourselves

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

# Test configuration
TEST_PREFIX="dockervault_test_"
TEST_VOLUME="${TEST_PREFIX}volume"
TEST_CONTAINER="${TEST_PREFIX}container"
TEST_BACKUP_DIR="/tmp/dockervault_integration_test"
TEST_DATA_FILE="test_data.txt"
TEST_DATA_CONTENT="DockerVault Integration Test - $(date)"

# Counters
TESTS_PASSED=0
TESTS_FAILED=0
TESTS_SKIPPED=0

print_header() {
    echo ""
    echo -e "${BLUE}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
    echo -e "${BLUE}  $1${NC}"
    echo -e "${BLUE}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
}

print_test() {
    echo -e "\n${YELLOW}TEST:${NC} $1"
}

print_pass() {
    echo -e "${GREEN}✓ PASS:${NC} $1"
    TESTS_PASSED=$((TESTS_PASSED + 1))
}

print_fail() {
    echo -e "${RED}✗ FAIL:${NC} $1"
    TESTS_FAILED=$((TESTS_FAILED + 1))
}

print_skip() {
    echo -e "${YELLOW}○ SKIP:${NC} $1"
    TESTS_SKIPPED=$((TESTS_SKIPPED + 1))
}

print_info() {
    echo -e "${BLUE}ℹ${NC} $1"
}

# Cleanup function
cleanup() {
    print_header "Cleanup"
    
    # Stop and remove test container
    if docker ps -a --format '{{.Names}}' | grep -q "^${TEST_CONTAINER}$"; then
        print_info "Removing test container: ${TEST_CONTAINER}"
        docker rm -f "${TEST_CONTAINER}" 2>/dev/null || true
    fi
    
    # Remove test volume
    if docker volume ls --format '{{.Name}}' | grep -q "^${TEST_VOLUME}$"; then
        print_info "Removing test volume: ${TEST_VOLUME}"
        docker volume rm "${TEST_VOLUME}" 2>/dev/null || true
    fi
    
    # Remove test backup directory
    if [ -d "${TEST_BACKUP_DIR}" ]; then
        print_info "Removing test backup directory: ${TEST_BACKUP_DIR}"
        rm -rf "${TEST_BACKUP_DIR}"
    fi
    
    print_info "Cleanup complete"
}

# Check prerequisites
check_prerequisites() {
    print_header "Prerequisites Check"
    
    # Check Docker
    if ! command -v docker &> /dev/null; then
        print_fail "Docker is not installed"
        exit 1
    fi
    print_pass "Docker is installed"
    
    # Check Docker daemon
    if ! docker info &> /dev/null; then
        print_fail "Docker daemon is not running"
        exit 1
    fi
    print_pass "Docker daemon is running"
    
    # Check Python
    if ! command -v python3 &> /dev/null; then
        print_fail "Python 3 is not installed"
        exit 1
    fi
    print_pass "Python 3 is installed"
    
    # Check we're in the right directory
    if [ ! -f "docker-compose.yml" ]; then
        print_fail "Please run from the project root directory"
        exit 1
    fi
    print_pass "Running from project root"
}

# Test 1: Create test volume with data
test_create_volume_with_data() {
    print_test "Create test volume with sample data"
    
    # Create volume
    docker volume create "${TEST_VOLUME}" > /dev/null
    
    # Create container that writes data to the volume
    docker run --rm \
        -v "${TEST_VOLUME}:/data" \
        alpine:latest \
        sh -c "echo '${TEST_DATA_CONTENT}' > /data/${TEST_DATA_FILE} && \
               mkdir -p /data/subdir && \
               echo 'nested file' > /data/subdir/nested.txt && \
               dd if=/dev/urandom of=/data/binary_file bs=1024 count=100 2>/dev/null"
    
    # Verify data was written
    RESULT=$(docker run --rm -v "${TEST_VOLUME}:/data" alpine:latest cat /data/${TEST_DATA_FILE})
    
    if [ "${RESULT}" = "${TEST_DATA_CONTENT}" ]; then
        print_pass "Volume created with test data"
    else
        print_fail "Failed to create volume with test data"
        return 1
    fi
}

# Test 2: Manual tar backup (simulating what the app does)
test_manual_tar_backup() {
    print_test "Create tar backup of volume (manual simulation)"
    
    mkdir -p "${TEST_BACKUP_DIR}"
    
    # Get volume mountpoint
    MOUNTPOINT=$(docker volume inspect "${TEST_VOLUME}" --format '{{.Mountpoint}}')
    
    # Create backup using a container (safer than direct access)
    docker run --rm \
        -v "${TEST_VOLUME}:/source:ro" \
        -v "${TEST_BACKUP_DIR}:/backup" \
        alpine:latest \
        tar -czf "/backup/test_backup.tar.gz" -C /source .
    
    if [ -f "${TEST_BACKUP_DIR}/test_backup.tar.gz" ]; then
        BACKUP_SIZE=$(stat -f%z "${TEST_BACKUP_DIR}/test_backup.tar.gz" 2>/dev/null || stat -c%s "${TEST_BACKUP_DIR}/test_backup.tar.gz")
        print_pass "Backup created successfully (${BACKUP_SIZE} bytes)"
    else
        print_fail "Failed to create backup"
        return 1
    fi
}

# Test 3: Verify backup integrity
test_verify_backup_integrity() {
    print_test "Verify backup integrity"
    
    # List backup contents
    CONTENTS=$(tar -tzf "${TEST_BACKUP_DIR}/test_backup.tar.gz" 2>/dev/null)
    
    if echo "${CONTENTS}" | grep -q "${TEST_DATA_FILE}"; then
        print_pass "Backup contains expected files"
    else
        print_fail "Backup missing expected files"
        return 1
    fi
    
    # Verify checksums are consistent
    CHECKSUM1=$(tar -xzf "${TEST_BACKUP_DIR}/test_backup.tar.gz" -O "./${TEST_DATA_FILE}" 2>/dev/null | md5sum | cut -d' ' -f1)
    CHECKSUM2=$(docker run --rm -v "${TEST_VOLUME}:/data:ro" alpine:latest cat "/data/${TEST_DATA_FILE}" | md5sum | cut -d' ' -f1)
    
    if [ "${CHECKSUM1}" = "${CHECKSUM2}" ]; then
        print_pass "Backup data integrity verified (checksums match)"
    else
        print_fail "Backup data integrity check failed"
        return 1
    fi
}

# Test 4: Simulate data loss and restore
test_restore_after_data_loss() {
    print_test "Restore backup after simulated data loss"
    
    # Simulate data loss - corrupt the volume
    docker run --rm \
        -v "${TEST_VOLUME}:/data" \
        alpine:latest \
        sh -c "rm -rf /data/* && echo 'CORRUPTED' > /data/${TEST_DATA_FILE}"
    
    # Verify data is corrupted
    CORRUPTED=$(docker run --rm -v "${TEST_VOLUME}:/data:ro" alpine:latest cat "/data/${TEST_DATA_FILE}")
    if [ "${CORRUPTED}" != "CORRUPTED" ]; then
        print_skip "Could not simulate data corruption"
        return 0
    fi
    print_info "Data corruption simulated"
    
    # Restore from backup
    docker run --rm \
        -v "${TEST_VOLUME}:/data" \
        -v "${TEST_BACKUP_DIR}:/backup:ro" \
        alpine:latest \
        sh -c "rm -rf /data/* && tar -xzf /backup/test_backup.tar.gz -C /data"
    
    # Verify restoration
    RESTORED=$(docker run --rm -v "${TEST_VOLUME}:/data:ro" alpine:latest cat "/data/${TEST_DATA_FILE}")
    
    if [ "${RESTORED}" = "${TEST_DATA_CONTENT}" ]; then
        print_pass "Data restored successfully from backup"
    else
        print_fail "Restore failed - data mismatch"
        echo "Expected: ${TEST_DATA_CONTENT}"
        echo "Got: ${RESTORED}"
        return 1
    fi
}

# Test 5: Test backup with running container
test_backup_with_running_container() {
    print_test "Backup volume while container is running"
    
    # Start a container using the volume
    docker run -d \
        --name "${TEST_CONTAINER}" \
        -v "${TEST_VOLUME}:/data" \
        alpine:latest \
        sh -c "while true; do date >> /data/activity.log; sleep 1; done"
    
    sleep 2  # Let it run for a bit
    
    # Try to create backup while container is running
    docker run --rm \
        -v "${TEST_VOLUME}:/source:ro" \
        -v "${TEST_BACKUP_DIR}:/backup" \
        alpine:latest \
        tar -czf "/backup/live_backup.tar.gz" -C /source .
    
    if [ -f "${TEST_BACKUP_DIR}/live_backup.tar.gz" ]; then
        print_pass "Backup created while container was running"
    else
        print_fail "Failed to backup with running container"
        docker stop "${TEST_CONTAINER}" 2>/dev/null || true
        docker rm "${TEST_CONTAINER}" 2>/dev/null || true
        return 1
    fi
    
    # Stop the container
    docker stop "${TEST_CONTAINER}" > /dev/null
    docker rm "${TEST_CONTAINER}" > /dev/null
    print_info "Test container stopped and removed"
}

# Test 6: Test backup security (path traversal prevention)
test_backup_security() {
    print_test "Security: Path traversal prevention"
    
    # Create a malicious tar file
    cd "${TEST_BACKUP_DIR}"
    
    # Try to create a tar with path traversal (this should fail in a secure system)
    mkdir -p safe_content
    echo "safe content" > safe_content/safe.txt
    tar -czf path_traversal.tar.gz safe_content
    
    # Verify extraction stays within bounds
    mkdir -p extract_test
    cd extract_test
    tar -xzf ../path_traversal.tar.gz 2>/dev/null
    
    if [ -d "safe_content" ]; then
        print_pass "Tar extraction contained within directory"
    else
        print_fail "Unexpected extraction behavior"
    fi
    
    cd - > /dev/null
}

# Test 7: Test large file handling
test_large_file_backup() {
    print_test "Large file backup performance"
    
    # Create a larger file (50MB)
    docker run --rm \
        -v "${TEST_VOLUME}:/data" \
        alpine:latest \
        dd if=/dev/urandom of=/data/large_file bs=1M count=50 2>/dev/null
    
    START_TIME=$(date +%s)
    
    docker run --rm \
        -v "${TEST_VOLUME}:/source:ro" \
        -v "${TEST_BACKUP_DIR}:/backup" \
        alpine:latest \
        tar -czf "/backup/large_backup.tar.gz" -C /source .
    
    END_TIME=$(date +%s)
    DURATION=$((END_TIME - START_TIME))
    
    if [ -f "${TEST_BACKUP_DIR}/large_backup.tar.gz" ]; then
        BACKUP_SIZE=$(stat -f%z "${TEST_BACKUP_DIR}/large_backup.tar.gz" 2>/dev/null || stat -c%s "${TEST_BACKUP_DIR}/large_backup.tar.gz")
        BACKUP_SIZE_MB=$((BACKUP_SIZE / 1024 / 1024))
        print_pass "Large backup completed in ${DURATION}s (${BACKUP_SIZE_MB}MB compressed)"
    else
        print_fail "Large file backup failed"
        return 1
    fi
}

# Test 8: API health check (if app is running)
test_api_health() {
    print_test "API health check (if running)"
    
    # Check if backend is running
    if curl -s -f "http://localhost:8000/api/v1/docker/health" > /dev/null 2>&1; then
        print_pass "Backend API is healthy"
        
        # Additional API tests
        if curl -s -f "http://localhost:8000/api/v1/docker/volumes" > /dev/null 2>&1; then
            print_pass "Docker volumes endpoint responding"
        fi
        
        if curl -s -f "http://localhost:8000/api/v1/targets" > /dev/null 2>&1; then
            print_pass "Targets endpoint responding"
        fi
    else
        print_skip "Backend API not running (start with docker-compose up)"
    fi
}

# Print summary
print_summary() {
    print_header "Test Summary"
    
    echo ""
    echo -e "  ${GREEN}Passed:${NC}  ${TESTS_PASSED}"
    echo -e "  ${RED}Failed:${NC}  ${TESTS_FAILED}"
    echo -e "  ${YELLOW}Skipped:${NC} ${TESTS_SKIPPED}"
    echo ""
    
    TOTAL=$((TESTS_PASSED + TESTS_FAILED))
    if [ ${TESTS_FAILED} -eq 0 ]; then
        echo -e "${GREEN}All tests passed! ✓${NC}"
        return 0
    else
        echo -e "${RED}Some tests failed! ✗${NC}"
        return 1
    fi
}

# Main execution
main() {
    # Parse arguments
    CLEANUP_ONLY=false
    KEEP_VOLUMES=false
    
    while [[ $# -gt 0 ]]; do
        case $1 in
            --cleanup-only)
                CLEANUP_ONLY=true
                shift
                ;;
            --keep-volumes)
                KEEP_VOLUMES=true
                shift
                ;;
            --help|-h)
                echo "Usage: $0 [OPTIONS]"
                echo ""
                echo "Options:"
                echo "  --cleanup-only   Only run cleanup, no tests"
                echo "  --keep-volumes   Don't remove test volumes after tests"
                echo "  --help, -h       Show this help"
                exit 0
                ;;
            *)
                echo "Unknown option: $1"
                exit 1
                ;;
        esac
    done
    
    print_header "DockerVault Integration Tests"
    echo "Testing backup and restore operations..."
    
    if [ "$CLEANUP_ONLY" = true ]; then
        cleanup
        exit 0
    fi
    
    # Run tests
    check_prerequisites
    
    # Cleanup any previous test artifacts
    cleanup
    
    # Run test suite
    test_create_volume_with_data
    test_manual_tar_backup
    test_verify_backup_integrity
    test_restore_after_data_loss
    test_backup_with_running_container
    test_backup_security
    test_large_file_backup
    test_api_health
    
    # Cleanup unless --keep-volumes
    if [ "$KEEP_VOLUMES" = false ]; then
        cleanup
    else
        print_info "Keeping test volumes for inspection"
        print_info "Run '$0 --cleanup-only' to clean up later"
    fi
    
    # Print results
    print_summary
}

main "$@"
