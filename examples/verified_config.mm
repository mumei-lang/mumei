// Verified Configuration Pattern
// Demonstrates using refinement types for configuration validation

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
