enum Chan {
    Tcp(port: i64, keep: i64),
    Udp
}

atom block_root_enum_payload_read(e: Chan) -> i64
requires: true;
ensures: result == 1;
body: {
    let w = 0;
    { w = w + 1; e }.port;
    w
};
