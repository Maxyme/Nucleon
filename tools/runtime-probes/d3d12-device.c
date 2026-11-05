/* DXGI/D3D12 initialization and GPU depth-clear probe. No game, Steam or OptiScaler needed.
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

static HRESULT probe_depth_clear(ID3D12Device *device)
{
    ID3D12CommandAllocator *allocator = NULL;
    ID3D12GraphicsCommandList *list = NULL;
    ID3D12DescriptorHeap *descriptors = NULL;
    ID3D12Resource *texture = NULL;
    ID3D12Resource *readback = NULL;
    ID3D12CommandQueue *queue = NULL;
    ID3D12Fence *fence = NULL;
    HANDLE event = NULL;
    HRESULT result = ID3D12Device_CreateCommandAllocator(device, D3D12_COMMAND_LIST_TYPE_DIRECT,
                                                        &IID_ID3D12CommandAllocator, (void **)&allocator);
    if (FAILED(result)) goto done;
    result = ID3D12Device_CreateCommandList(device, 0, D3D12_COMMAND_LIST_TYPE_DIRECT, allocator,
                                           NULL, &IID_ID3D12GraphicsCommandList, (void **)&list);
    if (FAILED(result)) goto done;
    D3D12_DESCRIPTOR_HEAP_DESC heap = {D3D12_DESCRIPTOR_HEAP_TYPE_DSV, 1, D3D12_DESCRIPTOR_HEAP_FLAG_NONE, 0};
    result = ID3D12Device_CreateDescriptorHeap(device, &heap, &IID_ID3D12DescriptorHeap, (void **)&descriptors);
    if (FAILED(result)) goto done;
    D3D12_HEAP_PROPERTIES properties = {0};
    properties.Type = D3D12_HEAP_TYPE_DEFAULT;
    properties.CreationNodeMask = properties.VisibleNodeMask = 1;
    D3D12_RESOURCE_DESC description = {0};
    description.Dimension = D3D12_RESOURCE_DIMENSION_TEXTURE2D;
    description.Width = description.Height = 16;
    description.DepthOrArraySize = description.MipLevels = 1;
    description.Format = DXGI_FORMAT_D32_FLOAT;
    description.SampleDesc.Count = 1;
    description.Flags = D3D12_RESOURCE_FLAG_ALLOW_DEPTH_STENCIL;
    D3D12_CLEAR_VALUE clear = {0};
    clear.Format = DXGI_FORMAT_D32_FLOAT;
    clear.DepthStencil.Depth = 0.25f;
    result = ID3D12Device_CreateCommittedResource(device, &properties, D3D12_HEAP_FLAG_NONE, &description,
                                                 D3D12_RESOURCE_STATE_DEPTH_WRITE, &clear,
                                                 &IID_ID3D12Resource, (void **)&texture);
    if (FAILED(result)) goto done;
    D3D12_CPU_DESCRIPTOR_HANDLE handle;
    descriptors->lpVtbl->GetCPUDescriptorHandleForHeapStart(descriptors, &handle);
    ID3D12Device_CreateDepthStencilView(device, texture, NULL, handle);
    ID3D12GraphicsCommandList_ClearDepthStencilView(list, handle, D3D12_CLEAR_FLAG_DEPTH, 0.25f, 0, 0, NULL);

    D3D12_PLACED_SUBRESOURCE_FOOTPRINT footprint;
    UINT64 size;
    ID3D12Device_GetCopyableFootprints(device, &description, 0, 1, 0, &footprint, NULL, NULL, &size);
    D3D12_RESOURCE_DESC buffer = {0};
    buffer.Dimension = D3D12_RESOURCE_DIMENSION_BUFFER;
    buffer.Width = size;
    buffer.Height = buffer.DepthOrArraySize = buffer.MipLevels = 1;
    buffer.SampleDesc.Count = 1;
    buffer.Layout = D3D12_TEXTURE_LAYOUT_ROW_MAJOR;
    properties.Type = D3D12_HEAP_TYPE_READBACK;
    result = ID3D12Device_CreateCommittedResource(device, &properties, D3D12_HEAP_FLAG_NONE, &buffer,
                                                 D3D12_RESOURCE_STATE_COPY_DEST, NULL,
                                                 &IID_ID3D12Resource, (void **)&readback);
    if (FAILED(result)) goto done;
    D3D12_RESOURCE_BARRIER barrier = {0};
    barrier.Type = D3D12_RESOURCE_BARRIER_TYPE_TRANSITION;
    barrier.Transition.pResource = texture;
    barrier.Transition.Subresource = D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES;
    barrier.Transition.StateBefore = D3D12_RESOURCE_STATE_DEPTH_WRITE;
    barrier.Transition.StateAfter = D3D12_RESOURCE_STATE_COPY_SOURCE;
    ID3D12GraphicsCommandList_ResourceBarrier(list, 1, &barrier);
    D3D12_TEXTURE_COPY_LOCATION source = {0}, destination = {0};
    source.pResource = texture;
    source.Type = D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX;
    destination.pResource = readback;
    destination.Type = D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT;
    destination.PlacedFootprint = footprint;
    ID3D12GraphicsCommandList_CopyTextureRegion(list, &destination, 0, 0, 0, &source, NULL);
    result = ID3D12GraphicsCommandList_Close(list);
    if (FAILED(result)) goto done;
    D3D12_COMMAND_QUEUE_DESC queue_description = {0};
    result = ID3D12Device_CreateCommandQueue(device, &queue_description, &IID_ID3D12CommandQueue, (void **)&queue);
    if (FAILED(result)) goto done;
    result = ID3D12Device_CreateFence(device, 0, D3D12_FENCE_FLAG_NONE, &IID_ID3D12Fence, (void **)&fence);
    if (FAILED(result)) goto done;
    event = CreateEventW(NULL, FALSE, FALSE, NULL);
    if (!event) { result = HRESULT_FROM_WIN32(GetLastError()); goto done; }
    ID3D12CommandList *commands[] = {(ID3D12CommandList *)list};
    ID3D12CommandQueue_ExecuteCommandLists(queue, 1, commands);
    result = ID3D12CommandQueue_Signal(queue, fence, 1);
    if (FAILED(result)) goto done;
    result = ID3D12Fence_SetEventOnCompletion(fence, 1, event);
    if (FAILED(result)) goto done;
    if (WaitForSingleObject(event, 10000) != WAIT_OBJECT_0) { result = E_FAIL; goto done; }
    void *pixels = NULL;
    D3D12_RANGE range = {0, (SIZE_T)size};
    result = ID3D12Resource_Map(readback, 0, &range, &pixels);
    if (SUCCEEDED(result)) {
        for (unsigned int y = 0; y < 16; ++y) {
            const float *row = (const float *)((const unsigned char *)pixels + footprint.Offset + y * footprint.Footprint.RowPitch);
            for (unsigned int x = 0; x < 16; ++x)
                if (row[x] != 0.25f) result = E_FAIL;
        }
        printf("GPU depth readback: %s (expected 0.25)\n", SUCCEEDED(result) ? "passed" : "failed");
        D3D12_RANGE written = {0, 0};
        ID3D12Resource_Unmap(readback, 0, &written);
    }
done:
    printf("depth-clear result=%08lx\n", result);
    if (event) CloseHandle(event);
    if (fence) ID3D12Fence_Release(fence);
    if (queue) ID3D12CommandQueue_Release(queue);
    if (readback) ID3D12Resource_Release(readback);
    if (texture) ID3D12Resource_Release(texture);
    if (descriptors) ID3D12DescriptorHeap_Release(descriptors);
    if (list) ID3D12GraphicsCommandList_Release(list);
    if (allocator) ID3D12CommandAllocator_Release(allocator);
    return result;
}

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
    if (device) {
        const unsigned char *entry = (const unsigned char *)(void *)device->lpVtbl->CheckFeatureSupport;
        printf("CheckFeatureSupport entry=%p bytes=", (const void *)entry);
        for (unsigned int i = 0; i < 16; ++i) printf("%02x", entry[i]);
        putchar('\n');
        D3D12_FEATURE_DATA_D3D12_OPTIONS3 options3 = {0};
        HRESULT feature3 = ID3D12Device_CheckFeatureSupport(device, D3D12_FEATURE_D3D12_OPTIONS3,
                                                          &options3, sizeof(options3));
        printf("options3 result=%08lx\n", feature3);
        D3D12_FEATURE_DATA_D3D12_OPTIONS4 options4 = {0};
        HRESULT feature4 = ID3D12Device_CheckFeatureSupport(device, D3D12_FEATURE_D3D12_OPTIONS4,
                                                  &options4, sizeof(options4));
        printf("options4 result=%08lx\n", feature4);
        hr = FAILED(feature3) ? feature3 : FAILED(feature4) ? feature4 : probe_depth_clear(device);
    }
    if (device) ID3D12Device_Release(device);
    IDXGIAdapter1_Release(adapter);
    IDXGIFactory1_Release(f);
    return FAILED(hr) ? 5 : 0;
}
