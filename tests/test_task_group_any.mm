trusted atom main()
requires: true;
ensures: true;
body: {
    task_group:any {
        task { 7 };
        task { recv(0) };
        task { recv(1) }
    }
};
