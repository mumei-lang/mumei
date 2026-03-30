// E2E test for Verified Configuration Pattern
// Run with: mumei verify tests/test_verified_config.mm

type Port = i64 where v >= 1 && v <= 65535;
type Timeout = i64 where v >= 100 && v <= 30000;
type MaxRetries = i64 where v >= 1 && v <= 10;

atom validate_server_config(port: Port, timeout: Timeout, retries: MaxRetries)
    requires: port >= 1 && port <= 65535 && timeout >= 100 && retries >= 1;
    ensures: result == 1;
    body: 1;

atom validate_port_range(port: Port, min_port: i64, max_port: i64)
    requires: min_port >= 1 && max_port <= 65535 && min_port <= max_port && port >= min_port && port <= max_port;
    ensures: result == port;
    body: port;

// Test: standard HTTP port validation
atom test_http_port(port: Port)
    requires: port >= 80 && port <= 443;
    ensures: result == port;
    body: port;

// Test: timeout within safe bounds
atom test_safe_timeout(timeout: Timeout)
    requires: timeout >= 1000 && timeout <= 5000;
    ensures: result == timeout;
    body: timeout;
