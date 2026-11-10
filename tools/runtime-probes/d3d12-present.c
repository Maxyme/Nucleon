/* Bounded DX12 swap-chain, all-pixel readback and presentation check.
 * Build: x86_64-w64-mingw32-gcc -Wall -Wextra -Werror d3d12-present.c -ldxguid -luser32 -o d3d12-present.exe
 */
#define COBJMACROS
#include <initguid.h>
#include <windows.h>
#include <dxgi1_4.h>
#include <d3d12.h>
#include <stdio.h>

#define CHECK(call) do { HRESULT status = (call); if (FAILED(status)) { \
    fprintf(stderr, "NUCLEON_D3D12_FAIL %s hr=%08lx\n", #call, status); goto cleanup; \
} } while (0)

typedef HRESULT (WINAPI *factory_fn)(UINT, REFIID, void **);
typedef HRESULT (WINAPI *device_fn)(IUnknown *, D3D_FEATURE_LEVEL, REFIID, void **);

static DWORD WINAPI watchdog(void *unused)
{
    (void)unused;
    Sleep(30000);
    fputs("NUCLEON_D3D12_FAIL timeout\n", stderr);
    TerminateProcess(GetCurrentProcess(), 124);
    return 0;
}

int main(void)
{
    int result = 1;
    HWND window = NULL;
    IDXGIFactory2 *factory = NULL;
    IDXGISwapChain1 *swap = NULL;
    IDXGISwapChain3 *swap3 = NULL;
    ID3D12Device *device = NULL;
    ID3D12CommandQueue *queue = NULL;
    ID3D12CommandAllocator *allocator = NULL;
    ID3D12GraphicsCommandList *list = NULL;
    ID3D12DescriptorHeap *heap = NULL;
    ID3D12Fence *fence = NULL;
    ID3D12Resource *back[2] = {NULL, NULL}, *readback = NULL;
    HANDLE event = NULL;
    setvbuf(stdout, NULL, _IONBF, 0);
    setvbuf(stderr, NULL, _IONBF, 0);
    HANDLE timer = CreateThread(NULL, 0, watchdog, NULL, 0, NULL);
    if (!timer) goto cleanup;
    CloseHandle(timer);
    HMODULE dxgi = LoadLibraryA("dxgi.dll"), d3d12 = LoadLibraryA("d3d12.dll");
    if (!dxgi || !d3d12) goto cleanup;
    factory_fn create_factory = (factory_fn)(void *)GetProcAddress(dxgi, "CreateDXGIFactory2");
    device_fn create_device = (device_fn)(void *)GetProcAddress(d3d12, "D3D12CreateDevice");
    if (!create_factory || !create_device) goto cleanup;
    CHECK(create_factory(0, &IID_IDXGIFactory2, (void **)&factory));
    CHECK(create_device(NULL, D3D_FEATURE_LEVEL_11_0, &IID_ID3D12Device, (void **)&device));
    D3D12_COMMAND_QUEUE_DESC queue_desc = {0};
    CHECK(ID3D12Device_CreateCommandQueue(device, &queue_desc, &IID_ID3D12CommandQueue, (void **)&queue));
    WNDCLASSA wc = {0};
    wc.lpfnWndProc = DefWindowProcA;
    wc.hInstance = GetModuleHandleA(NULL);
    wc.lpszClassName = "NucleonD3D12Probe";
    if (!RegisterClassA(&wc)) goto cleanup;
    window = CreateWindowA(wc.lpszClassName, "Nucleon DX12 graphics check",
                           WS_OVERLAPPEDWINDOW | WS_VISIBLE, CW_USEDEFAULT, CW_USEDEFAULT,
                           320, 240, NULL, NULL, wc.hInstance, NULL);
    if (!window) goto cleanup;
    DXGI_SWAP_CHAIN_DESC1 swap_desc = {0};
    swap_desc.Width = 256;
    swap_desc.Height = 128;
    swap_desc.Format = DXGI_FORMAT_R8G8B8A8_UNORM;
    swap_desc.SampleDesc.Count = 1;
    swap_desc.BufferUsage = DXGI_USAGE_RENDER_TARGET_OUTPUT;
    swap_desc.BufferCount = 2;
    swap_desc.SwapEffect = DXGI_SWAP_EFFECT_FLIP_DISCARD;
    puts("Creating DX12 swap chain");
    CHECK(IDXGIFactory2_CreateSwapChainForHwnd(factory, (IUnknown *)queue, window, &swap_desc, NULL, NULL, &swap));
    CHECK(IDXGISwapChain1_QueryInterface(swap, &IID_IDXGISwapChain3, (void **)&swap3));
    CHECK(ID3D12Device_CreateCommandAllocator(device, D3D12_COMMAND_LIST_TYPE_DIRECT,
                                             &IID_ID3D12CommandAllocator, (void **)&allocator));
    CHECK(ID3D12Device_CreateCommandList(device, 0, D3D12_COMMAND_LIST_TYPE_DIRECT, allocator,
                                        NULL, &IID_ID3D12GraphicsCommandList, (void **)&list));
    CHECK(ID3D12GraphicsCommandList_Close(list));
    D3D12_DESCRIPTOR_HEAP_DESC heap_desc = {D3D12_DESCRIPTOR_HEAP_TYPE_RTV, 2, D3D12_DESCRIPTOR_HEAP_FLAG_NONE, 0};
    CHECK(ID3D12Device_CreateDescriptorHeap(device, &heap_desc, &IID_ID3D12DescriptorHeap, (void **)&heap));
    D3D12_CPU_DESCRIPTOR_HANDLE handles[2];
    heap->lpVtbl->GetCPUDescriptorHandleForHeapStart(heap, &handles[0]);
    handles[1].ptr = handles[0].ptr + ID3D12Device_GetDescriptorHandleIncrementSize(device, D3D12_DESCRIPTOR_HEAP_TYPE_RTV);
    for (unsigned i = 0; i < 2; ++i) {
        CHECK(IDXGISwapChain1_GetBuffer(swap, i, &IID_ID3D12Resource, (void **)&back[i]));
        ID3D12Device_CreateRenderTargetView(device, back[i], NULL, handles[i]);
    }
    D3D12_RESOURCE_DESC texture;
    back[0]->lpVtbl->GetDesc(back[0], &texture);
    D3D12_PLACED_SUBRESOURCE_FOOTPRINT footprint;
    UINT64 size;
    ID3D12Device_GetCopyableFootprints(device, &texture, 0, 1, 0, &footprint, NULL, NULL, &size);
    D3D12_HEAP_PROPERTIES properties = {0};
    properties.Type = D3D12_HEAP_TYPE_READBACK;
    properties.CreationNodeMask = properties.VisibleNodeMask = 1;
    D3D12_RESOURCE_DESC buffer = {0};
    buffer.Dimension = D3D12_RESOURCE_DIMENSION_BUFFER;
    buffer.Width = size;
    buffer.Height = buffer.DepthOrArraySize = buffer.MipLevels = 1;
    buffer.SampleDesc.Count = 1;
    buffer.Layout = D3D12_TEXTURE_LAYOUT_ROW_MAJOR;
    CHECK(ID3D12Device_CreateCommittedResource(device, &properties, D3D12_HEAP_FLAG_NONE, &buffer,
                                               D3D12_RESOURCE_STATE_COPY_DEST, NULL,
                                               &IID_ID3D12Resource, (void **)&readback));
    CHECK(ID3D12Device_CreateFence(device, 0, D3D12_FENCE_FLAG_NONE, &IID_ID3D12Fence, (void **)&fence));
    event = CreateEventW(NULL, FALSE, FALSE, NULL);
    if (!event) goto cleanup;
    for (unsigned frame = 0; frame < 12; ++frame) {
        UINT index = IDXGISwapChain3_GetCurrentBackBufferIndex(swap3);
        if (index >= 2) goto cleanup;
        CHECK(ID3D12CommandAllocator_Reset(allocator));
        CHECK(ID3D12GraphicsCommandList_Reset(list, allocator, NULL));
        D3D12_RESOURCE_BARRIER barrier = {0};
        barrier.Type = D3D12_RESOURCE_BARRIER_TYPE_TRANSITION;
        barrier.Transition.pResource = back[index];
        barrier.Transition.Subresource = D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES;
        barrier.Transition.StateBefore = D3D12_RESOURCE_STATE_PRESENT;
        barrier.Transition.StateAfter = D3D12_RESOURCE_STATE_RENDER_TARGET;
        ID3D12GraphicsCommandList_ResourceBarrier(list, 1, &barrier);
        float color[] = {0, frame % 2 ? 1.0f : 0.0f, frame % 2 ? 0.0f : 1.0f, 1};
        ID3D12GraphicsCommandList_ClearRenderTargetView(list, handles[index], color, 0, NULL);
        barrier.Transition.StateBefore = D3D12_RESOURCE_STATE_RENDER_TARGET;
        barrier.Transition.StateAfter = D3D12_RESOURCE_STATE_COPY_SOURCE;
        ID3D12GraphicsCommandList_ResourceBarrier(list, 1, &barrier);
        D3D12_TEXTURE_COPY_LOCATION source = {0}, destination = {0};
        source.pResource = back[index];
        source.Type = D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX;
        destination.pResource = readback;
        destination.Type = D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT;
        destination.PlacedFootprint = footprint;
        ID3D12GraphicsCommandList_CopyTextureRegion(list, &destination, 0, 0, 0, &source, NULL);
        barrier.Transition.StateBefore = D3D12_RESOURCE_STATE_COPY_SOURCE;
        barrier.Transition.StateAfter = D3D12_RESOURCE_STATE_PRESENT;
        ID3D12GraphicsCommandList_ResourceBarrier(list, 1, &barrier);
        CHECK(ID3D12GraphicsCommandList_Close(list));
        ID3D12CommandList *commands[] = {(ID3D12CommandList *)list};
        ID3D12CommandQueue_ExecuteCommandLists(queue, 1, commands);
        CHECK(ID3D12CommandQueue_Signal(queue, fence, frame + 1));
        CHECK(ID3D12Fence_SetEventOnCompletion(fence, frame + 1, event));
        if (WaitForSingleObject(event, 10000) != WAIT_OBJECT_0) goto cleanup;
        void *pixels = NULL;
        D3D12_RANGE range = {0, (SIZE_T)size};
        CHECK(ID3D12Resource_Map(readback, 0, &range, &pixels));
        int valid = 1;
        for (UINT y = 0; y < texture.Height; ++y) {
            const unsigned char *row = (const unsigned char *)pixels + footprint.Offset + y * footprint.Footprint.RowPitch;
            for (UINT x = 0; x < texture.Width; ++x) {
                const unsigned char *pixel = row + x * 4;
                if (pixel[0] || pixel[1] != (frame % 2 ? 255 : 0) ||
                    pixel[2] != (frame % 2 ? 0 : 255) || pixel[3] != 255) valid = 0;
            }
        }
        D3D12_RANGE written = {0, 0};
        ID3D12Resource_Unmap(readback, 0, &written);
        if (!valid) { fprintf(stderr, "NUCLEON_D3D12_FAIL readback frame=%u\n", frame); goto cleanup; }
        CHECK(IDXGISwapChain1_Present(swap, 1, 0));
        MSG message;
        while (PeekMessageA(&message, NULL, 0, 0, PM_REMOVE)) {
            TranslateMessage(&message);
            DispatchMessageA(&message);
        }
        Sleep(50);
    }
    puts("NUCLEON_D3D12_OK frames=12 all-pixels-readback-and-present");
    result = 0;
cleanup:
    if (event) CloseHandle(event);
    if (readback) ID3D12Resource_Release(readback);
    if (fence) ID3D12Fence_Release(fence);
    for (unsigned i = 0; i < 2; ++i) if (back[i]) ID3D12Resource_Release(back[i]);
    if (heap) ID3D12DescriptorHeap_Release(heap);
    if (list) ID3D12GraphicsCommandList_Release(list);
    if (allocator) ID3D12CommandAllocator_Release(allocator);
    if (swap3) IDXGISwapChain3_Release(swap3);
    if (swap) IDXGISwapChain1_Release(swap);
    if (queue) ID3D12CommandQueue_Release(queue);
    if (device) ID3D12Device_Release(device);
    if (factory) IDXGIFactory2_Release(factory);
    if (window) DestroyWindow(window);
    return result;
}
