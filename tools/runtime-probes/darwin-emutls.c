/* Experimental compiler TLS ABI for the pinned Darwin ARM64EC frontends.
 * These frontends have a few private, trivial TLS values. Preserve per-thread
 * initialization/alignment and reclaim allocations at PE thread detach.
 * A separate copy lives in each DLL; exporting TLS data across DLLs is unsupported.
 */
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

struct control {
    uintptr_t size, alignment;
    void *object;
    void *initial;
};
struct entry {
    struct control *control;
    void *value;
    struct entry *next;
};
static INIT_ONCE once = INIT_ONCE_STATIC_INIT;
static DWORD slot = TLS_OUT_OF_INDEXES;
#ifdef NUCLEON_TLS_TEST
static volatile LONG live;
__declspec(dllexport) LONG tls_live(void) { return InterlockedCompareExchange(&live, 0, 0); }
#endif

static void CALLBACK destroy(void *value)
{
    struct entry *entry = value;
    while (entry) {
        struct entry *next = entry->next;
        _aligned_free(entry->value);
        free(entry);
#ifdef NUCLEON_TLS_TEST
        InterlockedDecrement(&live);
#endif
        entry = next;
    }
}
static void shutdown_tls(void)
{
    if (slot != TLS_OUT_OF_INDEXES) {
        destroy(TlsGetValue(slot));
        TlsFree(slot);
    }
    slot = TLS_OUT_OF_INDEXES;
}
static BOOL CALLBACK initialize(PINIT_ONCE ignored, void *parameter, void **context)
{
    (void)ignored; (void)parameter; (void)context;
    slot = TlsAlloc();
    if (slot == TLS_OUT_OF_INDEXES) return FALSE;
    if (atexit(shutdown_tls)) { TlsFree(slot); slot = TLS_OUT_OF_INDEXES; return FALSE; }
    return TRUE;
}
void *__emutls_get_address(struct control *control)
{
    if (!InitOnceExecuteOnce(&once, initialize, NULL, NULL)) abort();
    struct entry *head = TlsGetValue(slot);
    for (struct entry *entry = head; entry; entry = entry->next)
        if (entry->control == control) return entry->value;
    size_t alignment = control->alignment < sizeof(void *) ? sizeof(void *) : control->alignment;
    if (!control->size || (alignment & (alignment - 1))) abort();
    struct entry *entry = calloc(1, sizeof(*entry));
    if (!entry) abort();
    entry->value = _aligned_malloc(control->size, alignment);
    if (!entry->value) abort();
    if (control->initial) memcpy(entry->value, control->initial, control->size);
    else memset(entry->value, 0, control->size);
    entry->control = control;
    entry->next = head;
    if (!TlsSetValue(slot, entry)) abort();
#ifdef NUCLEON_TLS_TEST
    InterlockedIncrement(&live);
#endif
    return entry->value;
}

static void NTAPI thread_detach(void *module, DWORD reason, void *reserved)
{
    (void)module; (void)reserved;
    if (reason == DLL_THREAD_DETACH && slot != TLS_OUT_OF_INDEXES) {
        struct entry *head = TlsGetValue(slot);
        TlsSetValue(slot, NULL);
        destroy(head);
    }
}
/* Run after the CRT's C++ thread destructors; fibers still share true thread TLS. */
__attribute__((section(".CRT$XLY"), used))
static PIMAGE_TLS_CALLBACK detach_callback = thread_detach;
