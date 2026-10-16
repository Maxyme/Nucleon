#include <windows.h>
#include <stdio.h>
typedef unsigned (*step_fn)(unsigned, unsigned);
static step_fn step;
struct fiber_state { void *parent; unsigned error; };
static VOID WINAPI fiber_worker(void *argument)
{
    struct fiber_state *state = argument;
    if (step(7, 1001)) state->error = 1;
    SwitchToFiber(state->parent);
    if (step(1002, 1003)) state->error = 2;
    for (;;) SwitchToFiber(state->parent);
}
static DWORD WINAPI worker(void *argument)
{
    unsigned id = (unsigned)(ULONG_PTR)argument, expected = 7;
    for (unsigned i = 0; i < 10000; ++i) {
        unsigned next = 10000 + id * 10000 + i;
        if (step(expected, next)) return 1;
        expected = next;
        if (!(i % 100)) SwitchToThread();
    }
    return 0;
}
int main(void)
{
    HMODULE dll = LoadLibraryA("frontend-tls.dll");
    if (!dll) return 1;
    step = (step_fn)(ULONG_PTR)GetProcAddress(dll, "tls_step");
    if (!step) return 2;
    LONG (*live)(void) = (LONG (*)(void))(ULONG_PTR)GetProcAddress(dll, "tls_live");
    if (!live) return 6;
    for (unsigned round = 0; round < 4; ++round) {
        HANDLE threads[8];
        for (unsigned i = 0; i < 8; ++i) {
            threads[i] = CreateThread(NULL, 0, worker, (void *)(ULONG_PTR)i, 0, NULL);
            if (!threads[i]) return 3;
        }
        if (WaitForMultipleObjects(8, threads, TRUE, 30000) != WAIT_OBJECT_0) return 4;
        for (unsigned i = 0; i < 8; ++i) {
            DWORD code;
            if (!GetExitCodeThread(threads[i], &code) || code) return 5;
            CloseHandle(threads[i]);
        }
        if (live()) return 7;
    }
    struct fiber_state state = {ConvertThreadToFiber(NULL), 0};
    if (!state.parent) return 8;
    void *fiber = CreateFiber(0, fiber_worker, &state);
    if (!fiber) return 9;
    SwitchToFiber(fiber);
    if (state.error || step(1001, 1002)) return 10;
    SwitchToFiber(fiber);
    if (state.error || step(1003, 1004)) return 11;
    DeleteFiber(fiber);
    if (step(1004, 1005) || !ConvertFiberToThread()) return 12;
    FreeLibrary(dll);
    puts("NUCLEON_FRONTEND_TLS_PASS isolation-initialization-alignment-cleanup-fiber-sharing");
    return 0;
}
