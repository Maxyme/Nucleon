/* Minimal DXGI/D3D12 initialization probe. No game, Steam or OptiScaler needed.
 * Build: x86_64-w64-mingw32-gcc -Wall -Wextra -Werror d3d12-device.c -ldxguid -o d3d12-device.exe
 * Run in a disposable prefix with the complete selected GPTK runtime.
 */
#define COBJMACROS
#include <initguid.h>
#include <windows.h>
#include <dxgi1_2.h>
#include <d3d12.h>
#include <stdio.h>

typedef HRESULT (WINAPI *factory_fn)(REFIID, void **);
typedef HRESULT (WINAPI *device_fn)(IUnknown *, D3D_FEATURE_LEVEL, REFIID, void **);

int main(void)
{
    setvbuf(stdout, NULL, _IONBF, 0);
    HMODULE dxgi = LoadLibraryA("dxgi.dll");
    HMODULE d3d12 = LoadLibraryA("d3d12.dll");
    printf("load dxgi=%p d3d12=%p\n", (void *)dxgi, (void *)d3d12);
    if (!dxgi || !d3d12) return 2;
    factory_fn factory = (factory_fn)(void *)GetProcAddress(dxgi, "CreateDXGIFactory1");
    device_fn create = (device_fn)(void *)GetProcAddress(d3d12, "D3D12CreateDevice");
    if (!factory || !create) return 2;

    IDXGIFactory1 *f = NULL;
    HRESULT hr = factory(&IID_IDXGIFactory1, (void **)&f);
    printf("factory result=%08lx ptr=%p\n", hr, (void *)f);
    if (FAILED(hr)) return 3;
    IDXGIAdapter1 *adapter = NULL;
    hr = IDXGIFactory1_EnumAdapters1(f, 0, &adapter);
    printf("adapter result=%08lx ptr=%p\n", hr, (void *)adapter);
    if (FAILED(hr)) {
        IDXGIFactory1_Release(f);
        return 4;
    }
    DXGI_ADAPTER_DESC1 description = {0};
    hr = IDXGIAdapter1_GetDesc1(adapter, &description);
    if (SUCCEEDED(hr)) printf("adapter=%ls\n", description.Description);

    ID3D12Device *device = NULL;
    hr = create((IUnknown *)adapter, D3D_FEATURE_LEVEL_11_0,
                &IID_ID3D12Device, (void **)&device);
    printf("device result=%08lx ptr=%p\n", hr, (void *)device);
    if (device) ID3D12Device_Release(device);
    IDXGIAdapter1_Release(adapter);
    IDXGIFactory1_Release(f);
    return FAILED(hr) ? 5 : 0;
}
