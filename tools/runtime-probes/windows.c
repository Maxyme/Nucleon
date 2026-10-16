/* Small owned workload: no Steam, graphics, network or installed games. */
#include <windows.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <wchar.h>

static LONG counter;

#if defined(__i386__)
static volatile LONG vectored_exception_seen;

static LONG CALLBACK exception_handler(EXCEPTION_POINTERS *exception)
{
    if (exception->ExceptionRecord->ExceptionCode != 0xe0424242)
        return EXCEPTION_CONTINUE_SEARCH;
    vectored_exception_seen = 1;
    return EXCEPTION_CONTINUE_EXECUTION;
}
#endif


__declspec(noinline) static uint32_t shared_read(volatile uint32_t *pointer)
{
    return *pointer;
}

__declspec(noinline) static void shared_write(volatile uint32_t *pointer, uint32_t value)
{
    *pointer = value;
}

static DWORD WINAPI worker(void *unused)
{
    (void)unused;
    for (unsigned i = 0; i < 20000; ++i) InterlockedIncrement(&counter);
    return 0;
}

static int fail(const char *stage)
{
    fprintf(stderr, "NUCLEON_FEX_FAIL stage=%s error=%lu\n", stage, GetLastError());
    return 1;
}

int wmain(int argc, wchar_t **argv)
{
    if (argc == 2 && !wcscmp(argv[1], L"--child")) return 37;
    setbuf(stdout, NULL);
    printf("NUCLEON_FEX_BEGIN pointer_bits=%u\n", (unsigned)(sizeof(void *) * 8));

    unsigned char source[4096], destination[4096];
    uint64_t sum = 0;
    for (unsigned i = 0; i < sizeof(source); ++i) source[i] = (i * 17u) & 255u;
    memcpy(destination, source, sizeof(source));
    for (unsigned i = 0; i < sizeof(destination); ++i) sum += destination[i];
    if (sum != 522240 || memcmp(source, destination, sizeof(source))) return fail("memory");
    puts("NUCLEON_FEX_PASS memory");

    HANDLE threads[4];
    for (unsigned i = 0; i < 4; ++i)
    {
        threads[i] = CreateThread(NULL, 0, worker, NULL, 0, NULL);
        if (!threads[i]) return fail("create-thread");
    }
    if (WaitForMultipleObjects(4, threads, TRUE, 20000) != WAIT_OBJECT_0)
        return fail("wait-threads");
    for (unsigned i = 0; i < 4; ++i) CloseHandle(threads[i]);
    if (counter != 80000) return fail("thread-atomics");
    puts("NUCLEON_FEX_PASS threads");

    volatile int caught = 0;
#if defined(__i386__)
    /* LLVM-MinGW's i686 target cannot emit the x64 test's MS-style SEH tables. */
    void *handler = AddVectoredExceptionHandler(1, exception_handler);
    if (!handler) return fail("exception-handler");
    RaiseException(0xe0424242, 0, 0, NULL);
    caught = vectored_exception_seen;
    if (!RemoveVectoredExceptionHandler(handler)) return fail("remove-exception-handler");
#else
    __try { RaiseException(0xe0424242, 0, 0, NULL); }
    __except(GetExceptionCode() == 0xe0424242 ? EXCEPTION_EXECUTE_HANDLER : EXCEPTION_CONTINUE_SEARCH)
    { caught = 1; }
#endif
    if (!caught) return fail("exception");
    puts("NUCLEON_FEX_PASS exception");

#if defined(__x86_64__)
    if (argc == 2 && !wcscmp(argv[1], L"--shared-data"))
    {
        void *(__cdecl *get_shared_data)(void) = (void *)GetProcAddress(
            GetModuleHandleW(L"ntdll.dll"), "__wine_get_user_shared_data");
        if (!get_shared_data) return fail("shared-data-export");
        const unsigned char *page = get_shared_data();
        if (!page) return fail("shared-data-mapping");
        uint32_t expected = *(const uint32_t *)(page + 0x26c); /* NtMajorVersion, stable */
        volatile uint32_t observed = 0;
        caught = 0;
        __try { observed = shared_read((void *)(uintptr_t)0x7ffe026c); }
        __except(GetExceptionCode() == EXCEPTION_ACCESS_VIOLATION ? EXCEPTION_EXECUTE_HANDLER : EXCEPTION_CONTINUE_SEARCH)
        { caught = 1; }
        if (caught || !expected || observed != expected) return fail("shared-data-read");
        puts("NUCLEON_FEX_PASS shared-data-read");

        caught = 0;
        __try { shared_write((void *)(uintptr_t)0x7ffe026c, expected); }
        __except(GetExceptionCode() == EXCEPTION_ACCESS_VIOLATION ? EXCEPTION_EXECUTE_HANDLER : EXCEPTION_CONTINUE_SEARCH)
        { caught = 1; }
        if (!caught) return fail("shared-data-write-protection");
        caught = 0;
        __try { observed = shared_read((void *)(uintptr_t)0x7ffe1000); }
        __except(GetExceptionCode() == EXCEPTION_ACCESS_VIOLATION ? EXCEPTION_EXECUTE_HANDLER : EXCEPTION_CONTINUE_SEARCH)
        { caught = 1; }
        if (!caught) return fail("shared-data-boundary");
        puts("NUCLEON_FEX_PASS shared-data-protection");
    }

#endif

#if defined(__x86_64__)
    unsigned char *code = VirtualAlloc(NULL, 4096, MEM_COMMIT | MEM_RESERVE, PAGE_EXECUTE_READWRITE);
    if (!code) return fail("allocate-code");
    unsigned char instructions[] = {0xb8, 42, 0, 0, 0, 0xc3}; /* mov eax,42; ret */
    memcpy(code, instructions, sizeof(instructions));
    FlushInstructionCache(GetCurrentProcess(), code, sizeof(instructions));
    if (((int (*)(void))code)() != 42) return fail("jit-first");
    code[1] = 43;
    FlushInstructionCache(GetCurrentProcess(), code, sizeof(instructions));
    if (((int (*)(void))code)() != 43) return fail("jit-modified");
    VirtualFree(code, 0, MEM_RELEASE);
    puts("NUCLEON_FEX_PASS self-modifying-code");
#endif

    wchar_t executable[32768], command[32790];
    DWORD length = GetModuleFileNameW(NULL, executable, 32768);
    if (!length || length >= 32768) return fail("executable-path");
    if (swprintf(command, 32790, L"\"%ls\" --child", executable) < 0) return fail("child-command");
    STARTUPINFOW startup = {0};
    PROCESS_INFORMATION process = {0};
    startup.cb = sizeof(startup);
    if (!CreateProcessW(executable, command, NULL, NULL, FALSE, 0, NULL, NULL, &startup, &process))
        return fail("create-process");
    DWORD exit_code = 0;
    if (WaitForSingleObject(process.hProcess, 20000) != WAIT_OBJECT_0 ||
        !GetExitCodeProcess(process.hProcess, &exit_code) || exit_code != 37)
        return fail("child-exit");
    CloseHandle(process.hThread);
    CloseHandle(process.hProcess);
    puts("NUCLEON_FEX_PASS child-process");
    puts("NUCLEON_FEX_SMOKE_OK");
    return 0;
}
