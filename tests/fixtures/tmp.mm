effect File
    states: [Closed, Open];
    initial: Closed;
    transition open: Closed -> Open;
    transition write: Open -> Open;
    transition close: Open -> Closed;

atom bad_file_usage(x: i64) -> i64
    effects: [File];
    body: {
        perform File.open(x);
        perform File.close(x);
        perform File.write(x);
        0
    };
