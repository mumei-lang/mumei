effect Door
    states: [Closed, Open];
    initial: Closed;
    transition open: Closed -> Open;
    transition close: Open -> Closed;

atom open_door(x: i64) -> i64
effects: [Door];
requires: x >= 0;
ensures: result == x;
body: x;
