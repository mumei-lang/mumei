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
#include <stdatomic.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <stdlib.h>

#define MUMEI_MAX_CHANNELS 256
#define MUMEI_MAX_TASK_GROUPS 1024

typedef struct {
    pthread_mutex_t mu;
    pthread_cond_t cv;
    int64_t value;
    int ready;
    /*
     * `initialized` is read once without the `g_init_mu` lock (the outer
     * check of the double-checked locking pattern in `mumei_chan_get`).
     * Under the C11 memory model that unsynchronized read would be a
     * data race on a plain `int`, so we make the field `_Atomic` and
     * pair acquire-loads with release-stores. On x86-64 this compiles
     * to plain mov instructions; on weaker architectures (ARM, POWER)
     * it inserts the required barriers.
     */
    _Atomic int initialized;
} mumei_chan_t;

static mumei_chan_t g_channels[MUMEI_MAX_CHANNELS];
static pthread_mutex_t g_init_mu = PTHREAD_MUTEX_INITIALIZER;

typedef struct {
    _Atomic int completed;
} mumei_task_group_t;

static mumei_task_group_t g_task_groups[MUMEI_MAX_TASK_GROUPS];

static mumei_task_group_t *mumei_task_group_get(int64_t group_id) {
    if (group_id < 0 || group_id >= MUMEI_MAX_TASK_GROUPS) {
        fprintf(stderr,
                "[mumei runtime] fatal: task_group id %lld out of range [0, %d)\n",
                (long long)group_id, MUMEI_MAX_TASK_GROUPS);
        abort();
    }
    return &g_task_groups[group_id];
}

void __mumei_task_cancel(int64_t task_id) {
    if (task_id == 0) {
        return;
    }
    (void)pthread_cancel((pthread_t)(uintptr_t)task_id);
}

void __mumei_task_group_reset(int64_t group_id) {
    mumei_task_group_t *group = mumei_task_group_get(group_id);
    atomic_store_explicit(&group->completed, 0, memory_order_release);
}

int64_t __mumei_task_group_any_flag(int64_t group_id) {
    mumei_task_group_t *group = mumei_task_group_get(group_id);
    return atomic_load_explicit(&group->completed, memory_order_acquire) == 2 ? 1 : 0;
}

void __mumei_task_group_set_completed(int64_t group_id) {
    mumei_task_group_t *group = mumei_task_group_get(group_id);
    atomic_store_explicit(&group->completed, 2, memory_order_release);
}

int64_t __mumei_task_group_complete(int64_t group_id, int64_t result, int64_t *result_ptr) {
    mumei_task_group_t *group = mumei_task_group_get(group_id);
    int expected = 0;
    if (atomic_compare_exchange_strong_explicit(
            &group->completed, &expected, 1, memory_order_acq_rel, memory_order_acquire)) {
        if (result_ptr != NULL) {
            *result_ptr = result;
        }
        atomic_store_explicit(&group->completed, 2, memory_order_release);
        return 1;
    }
    return 0;
}

static void mumei_unlock_mutex_cleanup(void *arg) {
    pthread_mutex_unlock((pthread_mutex_t *)arg);
}

static mumei_chan_t *mumei_chan_get(int64_t chan_id) {
    /*
     * Fail-fast on out-of-range handles. The previous behaviour silently
     * clamped to channel 0, which would quietly alias logically distinct
     * channels once `chan_new()` returns fresh ids — a very hard-to-debug
     * source of spurious rendezvous. Aborting here surfaces the bug at
     * the point of first misuse.
     */
    if (chan_id < 0 || chan_id >= MUMEI_MAX_CHANNELS) {
        fprintf(stderr,
                "[mumei runtime] fatal: chan_id %lld out of range [0, %d)\n",
                (long long)chan_id, MUMEI_MAX_CHANNELS);
        abort();
    }
    mumei_chan_t *ch = &g_channels[chan_id];
    if (!atomic_load_explicit(&ch->initialized, memory_order_acquire)) {
        pthread_mutex_lock(&g_init_mu);
        if (!atomic_load_explicit(&ch->initialized, memory_order_relaxed)) {
            pthread_mutex_init(&ch->mu, NULL);
            pthread_cond_init(&ch->cv, NULL);
            ch->value = 0;
            ch->ready = 0;
            atomic_store_explicit(&ch->initialized, 1, memory_order_release);
        }
        pthread_mutex_unlock(&g_init_mu);
    }
    return ch;
}

void __mumei_chan_send(int64_t chan_id, int64_t value) {
    mumei_chan_t *ch = mumei_chan_get(chan_id);
    pthread_mutex_lock(&ch->mu);
    pthread_cleanup_push(mumei_unlock_mutex_cleanup, &ch->mu);
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
    pthread_cleanup_pop(1);
}

int64_t __mumei_chan_recv(int64_t chan_id) {
    mumei_chan_t *ch = mumei_chan_get(chan_id);
    int64_t v = 0;
    pthread_mutex_lock(&ch->mu);
    pthread_cleanup_push(mumei_unlock_mutex_cleanup, &ch->mu);
    while (!ch->ready) {
        pthread_cond_wait(&ch->cv, &ch->mu);
    }
    v = ch->value;
    ch->ready = 0;
    pthread_cond_broadcast(&ch->cv);
    pthread_cleanup_pop(1);
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


#define MUMEI_MAX_NAMED_RESOURCES 128
#define MUMEI_MAX_NAMED_RESOURCE_NAME 128

typedef struct {
    pthread_mutex_t mu;
    char name[MUMEI_MAX_NAMED_RESOURCE_NAME];
    _Atomic int initialized;
} mumei_named_resource_t;

static mumei_named_resource_t g_named_resources[MUMEI_MAX_NAMED_RESOURCES];
static pthread_mutex_t g_named_resource_mu = PTHREAD_MUTEX_INITIALIZER;

static pthread_mutex_t *mumei_named_resource_get(const char *name) {
    if (name == NULL || name[0] == '\0') {
        fprintf(stderr, "[mumei runtime] fatal: empty resource name\n");
        abort();
    }

    pthread_mutex_lock(&g_named_resource_mu);
    int free_slot = -1;
    for (int i = 0; i < MUMEI_MAX_NAMED_RESOURCES; i++) {
        if (atomic_load_explicit(&g_named_resources[i].initialized, memory_order_acquire)) {
            if (strncmp(g_named_resources[i].name, name, MUMEI_MAX_NAMED_RESOURCE_NAME) == 0) {
                pthread_mutex_unlock(&g_named_resource_mu);
                return &g_named_resources[i].mu;
            }
        } else if (free_slot < 0) {
            free_slot = i;
        }
    }

    if (free_slot < 0) {
        pthread_mutex_unlock(&g_named_resource_mu);
        fprintf(stderr, "[mumei runtime] fatal: too many named resources\n");
        abort();
    }

    mumei_named_resource_t *slot = &g_named_resources[free_slot];
    pthread_mutex_init(&slot->mu, NULL);
    snprintf(slot->name, MUMEI_MAX_NAMED_RESOURCE_NAME, "%s", name);
    atomic_store_explicit(&slot->initialized, 1, memory_order_release);
    pthread_mutex_unlock(&g_named_resource_mu);
    return &slot->mu;
}

pthread_mutex_t *__mumei_get_resource_mutex(const char *name) {
    return mumei_named_resource_get(name);
}

int64_t __mumei_effect_stub(const char *effect, const char *operation) {
    (void)effect;
    (void)operation;
    return 0;
}
