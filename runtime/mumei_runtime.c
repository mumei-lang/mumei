/*
 * Mumei concurrency runtime — minimal C runtime backing the LLVM IR
 * emitted by `mumei-emit-llvm` for `task` / `task_group` / `chan`
 * constructs. Plan 21 — concurrency runtime.
 *
 * The compiler emits direct calls to `pthread_create` / `pthread_join`
 * for tasks (see `mumei-emit-llvm/src/codegen.rs::compile_task_spawn`),
 * so this file does NOT need to wrap them. It only provides the
 * channel-side helpers that the front-end's `send` / `recv` lowerings
 * call into:
 *
 *   void __mumei_chan_send(int64_t chan_id, int64_t value);
 *   int64_t __mumei_chan_recv(int64_t chan_id);
 *
 * Channels are identified by a small integer handle (i64). The front
 * end currently lowers `chan<T>` literals to `0` (the default
 * channel) and threads the value through unchanged; once the parser
 * grows a `chan_new()` builtin that returns a fresh id, the runtime
 * will already support up to `MUMEI_MAX_CHANNELS` distinct channels.
 *
 * Build (Linux):
 *   cc -O2 -fPIC -c runtime/mumei_runtime.c -o runtime/mumei_runtime.o
 *   ar rcs runtime/libmumei_runtime.a runtime/mumei_runtime.o
 *
 * The `mumei build` driver links the resulting archive (or .o) against
 * the per-atom `.ll` outputs together with `-lpthread`.
 */

#include <pthread.h>
#include <stdint.h>
#include <stdlib.h>

#define MUMEI_MAX_CHANNELS 256

typedef struct {
    pthread_mutex_t mu;
    pthread_cond_t cv;
    int64_t value;
    int ready;
    int initialized;
} mumei_chan_t;

static mumei_chan_t g_channels[MUMEI_MAX_CHANNELS];
static pthread_mutex_t g_init_mu = PTHREAD_MUTEX_INITIALIZER;

static mumei_chan_t *mumei_chan_get(int64_t chan_id) {
    if (chan_id < 0 || chan_id >= MUMEI_MAX_CHANNELS) {
        chan_id = 0;
    }
    mumei_chan_t *ch = &g_channels[chan_id];
    if (!ch->initialized) {
        pthread_mutex_lock(&g_init_mu);
        if (!ch->initialized) {
            pthread_mutex_init(&ch->mu, NULL);
            pthread_cond_init(&ch->cv, NULL);
            ch->value = 0;
            ch->ready = 0;
            ch->initialized = 1;
        }
        pthread_mutex_unlock(&g_init_mu);
    }
    return ch;
}

void __mumei_chan_send(int64_t chan_id, int64_t value) {
    mumei_chan_t *ch = mumei_chan_get(chan_id);
    pthread_mutex_lock(&ch->mu);
    /*
     * Single-slot rendezvous semantics: if a value is already pending,
     * wait for the receiver to drain it. This matches the behaviour
     * the verifier currently models (one outstanding message per
     * channel) and avoids unbounded buffering inside the runtime.
     */
    while (ch->ready) {
        pthread_cond_wait(&ch->cv, &ch->mu);
    }
    ch->value = value;
    ch->ready = 1;
    pthread_cond_broadcast(&ch->cv);
    pthread_mutex_unlock(&ch->mu);
}

int64_t __mumei_chan_recv(int64_t chan_id) {
    mumei_chan_t *ch = mumei_chan_get(chan_id);
    pthread_mutex_lock(&ch->mu);
    while (!ch->ready) {
        pthread_cond_wait(&ch->cv, &ch->mu);
    }
    int64_t v = ch->value;
    ch->ready = 0;
    pthread_cond_broadcast(&ch->cv);
    pthread_mutex_unlock(&ch->mu);
    return v;
}

/*
 * Convenience init hook the front-end may call once at program start
 * to materialize channel 0 eagerly. Today this is a no-op because
 * `mumei_chan_get` lazily initializes; kept here for future
 * pre-warming and so the symbol is reachable from `__mumei_chan_init`
 * call sites the codegen may grow later.
 */
void __mumei_chan_init(void) {
    (void)mumei_chan_get(0);
}
