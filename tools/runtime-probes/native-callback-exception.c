/* D3D12 native callback exception regression for an Intel Wine/GPTK runtime.
 * Build: x86_64-w64-mingw32-gcc -Wall -Wextra -Werror native-callback-exception.c -ldxguid -o native-callback-exception.exe
 * Based on the invalid-object callback check in Switchyard Wine revision
 * 34a09170d3754522dc55416617c6754dd2d9a086 (LGPL-2.1-or-later):
 * https://github.com/jungwuk-ryu/switchyard-wine/blob/34a09170d3754522dc55416617c6754dd2d9a086/switchyard/tests/native_callback_exception.c
 */
#define COBJMACROS
#include <initguid.h>
#include <windows.h>
#include <d3d12.h>
#include <stdio.h>

typedef HRESULT (WINAPI *device_fn)(IUnknown *, D3D_FEATURE_LEVEL, REFIID, void **);
typedef HRESULT (WINAPI *reset_fn)(ID3D12CommandAllocator *);

static DWORD WINAPI watchdog(void *unused)
{
    (void)unused;
    Sleep(15000);
    fputs("Native callback exception test timed out\n", stderr);
    TerminateProcess(GetCurrentProcess(), 124);
    return 0;
}

static LONG WINAPI unhandled(EXCEPTION_POINTERS *exception)
{
    fprintf(stderr, "Unhandled exception: %08lx\n", exception->ExceptionRecord->ExceptionCode);
    ExitProcess(99);
    return EXCEPTION_EXECUTE_HANDLER;
}

int main(void)
{
    setvbuf(stdout, NULL, _IONBF, 0);
    setvbuf(stderr, NULL, _IONBF, 0);
    SetUnhandledExceptionFilter(unhandled);
    HANDLE timer = CreateThread(NULL, 0, watchdog, NULL, 0, NULL);
    if (!timer) return 2;
    CloseHandle(timer);
    HMODULE module = LoadLibraryA("d3d12.dll");
    if (!module) return 3;
    device_fn create = (device_fn)(void *)GetProcAddress(module, "D3D12CreateDevice");
    if (!create) return 4;
    ID3D12Device *device = NULL;
    HRESULT result = create(NULL, D3D_FEATURE_LEVEL_11_0, &IID_ID3D12Device, (void **)&device);
    printf("device=%08lx\n", result);
    if (FAILED(result)) return 5;
    ID3D12CommandAllocator *allocator = NULL;
    result = ID3D12Device_CreateCommandAllocator(device, D3D12_COMMAND_LIST_TYPE_DIRECT,
                                                &IID_ID3D12CommandAllocator, (void **)&allocator);
    if (FAILED(result)) { ID3D12Device_Release(device); return 6; }
    /* The native method must fault on the invalid object; its wrapper must
     * return E_FAIL and restore the caller's thread state for the next call. */
    reset_fn reset = allocator->lpVtbl->Reset;
    puts("Calling native Reset with an invalid object");
    result = reset(NULL);
    printf("invalid native callback=%08lx\n", result);
    if (result != E_FAIL) return 7;
    result = ID3D12CommandAllocator_Reset(allocator);
    printf("valid native callback after recovery=%08lx\n", result);
    ID3D12CommandAllocator_Release(allocator);
    ID3D12Device_Release(device);
    if (FAILED(result)) return 8;
    puts("Native callback exception dispatch test passed");
    return 0;
}
