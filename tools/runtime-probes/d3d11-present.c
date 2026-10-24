/* A bounded DX11 window/readback probe. Does not launch a game or require Steam. */
#define COBJMACROS
#include <d3d11.h>
#include <stdio.h>

#define CHECK(call) do { HRESULT result = (call); if (FAILED(result)) { \
    fprintf(stderr, "NUCLEON_D3D11_FAIL %s hr=%08lx\n", #call, (unsigned long)result); \
    goto cleanup; } } while (0)

int main(void)
{
    int result = 1;
    HWND window = NULL;
    IDXGISwapChain *swap = NULL;
    ID3D11Device *device = NULL;
    ID3D11DeviceContext *context = NULL;
    ID3D11Texture2D *back = NULL, *staging = NULL;
    ID3D11RenderTargetView *view = NULL;
    WNDCLASSA wc = {0};
    wc.lpfnWndProc = DefWindowProcA;
    wc.hInstance = GetModuleHandleA(NULL);
    wc.lpszClassName = "NucleonD3D11Probe";
    if (!RegisterClassA(&wc)) goto cleanup;
    window = CreateWindowA(wc.lpszClassName, "Nucleon DX11 graphics check",
        WS_OVERLAPPEDWINDOW | WS_VISIBLE, CW_USEDEFAULT, CW_USEDEFAULT,
        320, 240, NULL, NULL, wc.hInstance, NULL);
    if (!window) goto cleanup;
    DXGI_SWAP_CHAIN_DESC desc = {0};
    desc.BufferDesc.Width = 256;
    desc.BufferDesc.Height = 128;
    desc.BufferDesc.Format = DXGI_FORMAT_R8G8B8A8_UNORM;
    desc.SampleDesc.Count = 1;
    desc.BufferUsage = DXGI_USAGE_RENDER_TARGET_OUTPUT;
    desc.BufferCount = 1;
    desc.OutputWindow = window;
    desc.Windowed = TRUE;
    desc.SwapEffect = DXGI_SWAP_EFFECT_DISCARD;
    D3D_FEATURE_LEVEL requested = D3D_FEATURE_LEVEL_11_0, selected = 0;
    CHECK(D3D11CreateDeviceAndSwapChain(NULL, D3D_DRIVER_TYPE_HARDWARE,
        NULL, 0, &requested, 1, D3D11_SDK_VERSION, &desc, &swap, &device,
        &selected, &context));
    CHECK(IDXGISwapChain_GetBuffer(swap, 0, &IID_ID3D11Texture2D, (void **)&back));
    CHECK(ID3D11Device_CreateRenderTargetView(device, (ID3D11Resource *)back, NULL, &view));
    D3D11_TEXTURE2D_DESC texture;
    ID3D11Texture2D_GetDesc(back, &texture);
    texture.Usage = D3D11_USAGE_STAGING;
    texture.BindFlags = 0;
    texture.CPUAccessFlags = D3D11_CPU_ACCESS_READ;
    texture.MiscFlags = 0;
    CHECK(ID3D11Device_CreateTexture2D(device, &texture, NULL, &staging));
    for (unsigned frame = 0; frame < 12; ++frame) {
        float color[] = {0, frame % 2 ? 1.0f : 0.0f, frame % 2 ? 0.0f : 1.0f, 1};
        ID3D11DeviceContext_ClearRenderTargetView(context, view, color);
        ID3D11DeviceContext_CopyResource(context, (ID3D11Resource *)staging,
            (ID3D11Resource *)back);
        D3D11_MAPPED_SUBRESOURCE mapped;
        CHECK(ID3D11DeviceContext_Map(context, (ID3D11Resource *)staging, 0,
            D3D11_MAP_READ, 0, &mapped));
        int valid = 1;
        for (unsigned y = 0; y < texture.Height; ++y) {
            const unsigned char *row = (const unsigned char *)mapped.pData + y * mapped.RowPitch;
            for (unsigned x = 0; x < texture.Width; ++x) {
                const unsigned char *p = row + 4 * x;
                if (p[0] || p[1] != (frame % 2 ? 255 : 0) ||
                    p[2] != (frame % 2 ? 0 : 255) || p[3] != 255) valid = 0;
            }
        }
        ID3D11DeviceContext_Unmap(context, (ID3D11Resource *)staging, 0);
        if (!valid) {
            fprintf(stderr, "NUCLEON_D3D11_FAIL readback frame=%u\n", frame);
            goto cleanup;
        }
        CHECK(IDXGISwapChain_Present(swap, 1, 0));
        MSG msg;
        while (PeekMessageA(&msg, NULL, 0, 0, PM_REMOVE)) {
            TranslateMessage(&msg);
            DispatchMessageA(&msg);
        }
        Sleep(50);
    }
    printf("NUCLEON_D3D11_OK bits=%u feature=%04x frames=12 all-pixels-readback-and-present\n",
        (unsigned)sizeof(void *) * 8, selected);
    result = 0;
cleanup:
    if (view) ID3D11RenderTargetView_Release(view);
    if (staging) ID3D11Texture2D_Release(staging);
    if (back) ID3D11Texture2D_Release(back);
    if (swap) IDXGISwapChain_Release(swap);
    if (context) ID3D11DeviceContext_Release(context);
    if (device) ID3D11Device_Release(device);
    if (window) DestroyWindow(window);
    return result;
}
