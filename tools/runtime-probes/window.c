/* Bounded GDI window/message test. No Steam or external application. */
#include <windows.h>
#include <stdio.h>

static unsigned paints;

static LRESULT CALLBACK window_proc(HWND window, UINT message, WPARAM wparam, LPARAM lparam)
{
    if (message == WM_PAINT)
    {
        PAINTSTRUCT paint;
        HDC dc = BeginPaint(window, &paint);
        RECT area;
        GetClientRect(window, &area);
        HBRUSH brush = CreateSolidBrush(RGB(40, 160, 90));
        if (brush && FillRect(dc, &area, brush)) ++paints;
        if (brush) DeleteObject(brush);
        EndPaint(window, &paint);
        return 0;
    }
    if (message == WM_TIMER)
    {
        DestroyWindow(window);
        return 0;
    }
    if (message == WM_DESTROY)
    {
        PostQuitMessage(0);
        return 0;
    }
    return DefWindowProcW(window, message, wparam, lparam);
}

int wmain(void)
{
    HINSTANCE instance = GetModuleHandleW(NULL);
    WNDCLASSW cls = {0};
    cls.lpfnWndProc = window_proc;
    cls.hInstance = instance;
    cls.lpszClassName = L"NucleonFexWindowProbe";
    if (!RegisterClassW(&cls)) return 1;
    HWND window = CreateWindowExW(0, cls.lpszClassName, L"Nucleon ARM64/FEX test",
        WS_OVERLAPPEDWINDOW, CW_USEDEFAULT, CW_USEDEFAULT, 480, 240, NULL, NULL, instance, NULL);
    if (!window) return 2;
    ShowWindow(window, SW_SHOWNOACTIVATE);
    UpdateWindow(window);
    if (!SetTimer(window, 1, 1000, NULL)) return 3;
    MSG message;
    int status;
    while ((status = GetMessageW(&message, NULL, 0, 0)) > 0)
    {
        TranslateMessage(&message);
        DispatchMessageW(&message);
    }
    if (status < 0 || !paints) return 4;
    printf("NUCLEON_FEX_WINDOW_OK paints=%u\n", paints);
    return 0;
}
