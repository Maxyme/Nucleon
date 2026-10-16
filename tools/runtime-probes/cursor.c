/* Verify a game's custom cursor can be loaded and selected through Wine.
 * Run with WINEDEBUG=+cursor to inspect the native mapping in the driver log.
 */
#include <windows.h>
#include <stdio.h>

int wmain(int argc, wchar_t **argv)
{
    if (argc != 3) return 1;
    wchar_t *end;
    unsigned long id = wcstoul(argv[2], &end, 10);
    if (!argv[2][0] || *end || !id || id > 65535) return 2;
    HMODULE module = LoadLibraryExW(argv[1], NULL, LOAD_LIBRARY_AS_DATAFILE);
    if (!module) return 3;
    HCURSOR cursor = LoadCursorW(module, MAKEINTRESOURCEW(id));
    if (!cursor) return 4;
    WNDCLASSW cls = {0};
    cls.lpfnWndProc = DefWindowProcW;
    cls.hInstance = GetModuleHandleW(NULL);
    cls.hCursor = cursor;
    cls.lpszClassName = L"NucleonCursorProbe";
    if (!RegisterClassW(&cls)) return 6;
    POINT point;
    if (!GetCursorPos(&point)) return 7;
    HWND window = CreateWindowExW(0, cls.lpszClassName, L"Nucleon cursor check",
        WS_OVERLAPPEDWINDOW, point.x - 32, point.y - 32, 256, 128,
        NULL, NULL, cls.hInstance, NULL);
    if (!window) return 8;
    ShowWindow(window, SW_SHOW);
    UpdateWindow(window);
    SetCapture(window);
    SetCursor(cursor);
    if (GetCursor() != cursor) return 5;
    puts("NUCLEON_CURSOR_SELECTED");
    DWORD until = GetTickCount() + 2000;
    MSG message;
    while ((LONG)(until - GetTickCount()) > 0)
    {
        while (PeekMessageW(&message, NULL, 0, 0, PM_REMOVE))
        {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
        Sleep(10);
    }
    ReleaseCapture();
    DestroyWindow(window);
    SetCursor(NULL);
    FreeLibrary(module);
    return 0;
}
