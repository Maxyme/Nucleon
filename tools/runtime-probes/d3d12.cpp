#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <d3d12.h>
#include <dxgi1_4.h>
#include <d3dcompiler.h>
#include <cstdio>
#include <cstring>
#include <stdexcept>
#include <string>
#include "graphics-abi.h"

template<class T> struct Owned {
    T *p = nullptr;
    ~Owned() { if (p) p->Release(); }
    T *operator->() const { return p; }
    T **address() { return &p; }
};
static void check(HRESULT hr, const char *stage) {
    if (FAILED(hr)) {
        std::printf("NUCLEON_D3D12_FAIL %s hr=%08lx\n", stage, (unsigned long)hr);
        throw std::runtime_error(stage);
    }
}
static D3D12_RESOURCE_BARRIER transition(ID3D12Resource *resource, D3D12_RESOURCE_STATES before,
                                       D3D12_RESOURCE_STATES after) {
    D3D12_RESOURCE_BARRIER b = {};
    b.Type = D3D12_RESOURCE_BARRIER_TYPE_TRANSITION;
    b.Transition = {resource, D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES, before, after};
    return b;
}
static D3D12_RESOURCE_DESC buffer(UINT64 size, D3D12_RESOURCE_FLAGS flags = D3D12_RESOURCE_FLAG_NONE) {
    D3D12_RESOURCE_DESC d = {};
    d.Dimension = D3D12_RESOURCE_DIMENSION_BUFFER; d.Width = size;
    d.Height = 1; d.DepthOrArraySize = 1; d.MipLevels = 1;
    d.SampleDesc.Count = 1; d.Layout = D3D12_TEXTURE_LAYOUT_ROW_MAJOR; d.Flags = flags;
    return d;
}
static LRESULT CALLBACK window_proc(HWND w, UINT m, WPARAM a, LPARAM b) {
    return DefWindowProcW(w, m, a, b);
}
int main(int argc, char **argv) {
    const bool no_draw = argc > 1 && !std::strcmp(argv[1], "--no-draw");
    try {
        Owned<IDXGIFactory4> factory;
        check(CreateDXGIFactory1(IID_PPV_ARGS(factory.address())), "factory");
        Owned<IDXGIAdapter1> adapter;
        check(factory->EnumAdapters1(0, adapter.address()), "adapter");
        DXGI_ADAPTER_DESC1 adapter_desc = {};
        check(adapter->GetDesc1(&adapter_desc), "adapter-description");
        if (adapter_desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE) throw std::runtime_error("software adapter");
        char name[256] = {};
        WideCharToMultiByte(CP_UTF8, 0, adapter_desc.Description, -1, name, sizeof(name), nullptr, nullptr);
        std::printf("NUCLEON_D3D12_ADAPTER %s\n", name);
        Owned<ID3D12Device> device;
        check(D3D12CreateDevice(adapter.p, D3D_FEATURE_LEVEL_11_0, IID_PPV_ARGS(device.address())), "device");
        std::puts("NUCLEON_D3D12_PASS device");
        if (!graphics_module_abi("d3d12.dll") || !graphics_module_abi("d3d12core.dll") ||
            !graphics_module_abi("dxgi.dll")) throw std::runtime_error("frontend ABI");
        D3D12_COMMAND_QUEUE_DESC qd = {};
        Owned<ID3D12CommandQueue> queue;
        check(device->CreateCommandQueue(&qd, IID_PPV_ARGS(queue.address())), "queue");
        Owned<ID3D12CommandAllocator> allocator;
        check(device->CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT, IID_PPV_ARGS(allocator.address())), "allocator");
        Owned<ID3D12GraphicsCommandList> commands;
        check(device->CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, allocator.p, nullptr,
                                       IID_PPV_ARGS(commands.address())), "commands");
        Owned<ID3D12Fence> fence;
        check(device->CreateFence(0, D3D12_FENCE_FLAG_NONE, IID_PPV_ARGS(fence.address())), "fence");
        HANDLE event = CreateEventW(nullptr, FALSE, FALSE, nullptr);
        if (!event) throw std::runtime_error("event");
        UINT64 serial = 0;
        auto submit = [&]() {
            check(commands->Close(), "close-command-list");
            ID3D12CommandList *list[] = {commands.p};
            queue->ExecuteCommandLists(1, list);
            check(queue->Signal(fence.p, ++serial), "signal");
            check(fence->SetEventOnCompletion(serial, event), "completion-event");
            if (WaitForSingleObject(event, 15000) != WAIT_OBJECT_0) throw std::runtime_error("fence timeout");
            check(device->GetDeviceRemovedReason(), "device-removed");
            check(allocator->Reset(), "reset-allocator");
            check(commands->Reset(allocator.p, nullptr), "reset-commands");
        };
        D3D12_HEAP_PROPERTIES gpu = {}, cpu = {};
        gpu.Type = D3D12_HEAP_TYPE_DEFAULT; cpu.Type = D3D12_HEAP_TYPE_READBACK;
        Owned<ID3D12Resource> readback, storage;
        auto bd = buffer(256), ud = buffer(16, D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS);
        check(device->CreateCommittedResource(&cpu, D3D12_HEAP_FLAG_NONE, &bd, D3D12_RESOURCE_STATE_COPY_DEST,
                                             nullptr, IID_PPV_ARGS(readback.address())), "compute-readback");
        check(device->CreateCommittedResource(&gpu, D3D12_HEAP_FLAG_NONE, &ud, D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                                             nullptr, IID_PPV_ARGS(storage.address())), "compute-storage");
        D3D12_ROOT_PARAMETER parameter = {};
        parameter.ParameterType = D3D12_ROOT_PARAMETER_TYPE_UAV;
        D3D12_ROOT_SIGNATURE_DESC root_desc = {1, &parameter, 0, nullptr, D3D12_ROOT_SIGNATURE_FLAG_NONE};
        Owned<ID3DBlob> signature, errors;
        check(D3D12SerializeRootSignature(&root_desc, D3D_ROOT_SIGNATURE_VERSION_1, signature.address(), errors.address()), "serialize-root");
        Owned<ID3D12RootSignature> root;
        check(device->CreateRootSignature(0, signature->GetBufferPointer(), signature->GetBufferSize(), IID_PPV_ARGS(root.address())), "root");
        const char *cs = "RWStructuredBuffer<uint> result:register(u0); [numthreads(4,1,1)] void main(uint3 i:SV_DispatchThreadID){result[i.x]=0x13579bdfu+i.x;}";
        Owned<ID3DBlob> compute;
        check(D3DCompile(cs, std::strlen(cs), "probe", nullptr, nullptr, "main", "cs_5_0", 0, 0, compute.address(), nullptr), "compile-compute");
        D3D12_COMPUTE_PIPELINE_STATE_DESC cd = {};
        cd.pRootSignature = root.p; cd.CS = {compute->GetBufferPointer(), compute->GetBufferSize()};
        Owned<ID3D12PipelineState> cp;
        check(device->CreateComputePipelineState(&cd, IID_PPV_ARGS(cp.address())), "compute-pipeline");
        commands->SetPipelineState(cp.p); commands->SetComputeRootSignature(root.p);
        commands->SetComputeRootUnorderedAccessView(0, storage->GetGPUVirtualAddress());
        commands->Dispatch(1, 1, 1);
        auto barrier = transition(storage.p, D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COPY_SOURCE);
        commands->ResourceBarrier(1, &barrier); commands->CopyBufferRegion(readback.p, 0, storage.p, 0, 16);
        submit();
        UINT *data = nullptr; D3D12_RANGE range = {0, 16}, none = {0, 0};
        check(readback->Map(0, &range, reinterpret_cast<void **>(&data)), "map-compute");
        bool compute_ok = true;
        for (UINT i = 0; i < 4; ++i) compute_ok &= data[i] == 0x13579bdfu + i;
        readback->Unmap(0, &none);
        if (!compute_ok) throw std::runtime_error("compute pixel mismatch");
        std::puts("NUCLEON_D3D12_PASS compute-readback-fence");
        const char *hlsl = "float4 vs(uint i:SV_VertexID):SV_Position{float2 p[3]={float2(0,.75),float2(-.75,-.75),float2(.75,-.75)};return float4(p[i],0,1);} float4 ps():SV_Target{return float4(0,1,0,1);}";
        Owned<ID3DBlob> vs, ps;
        check(D3DCompile(hlsl, std::strlen(hlsl), "probe", nullptr, nullptr, "vs", "vs_5_0", 0, 0, vs.address(), nullptr), "vertex-shader");
        check(D3DCompile(hlsl, std::strlen(hlsl), "probe", nullptr, nullptr, "ps", "ps_5_0", 0, 0, ps.address(), nullptr), "pixel-shader");
        D3D12_GRAPHICS_PIPELINE_STATE_DESC pd = {};
        pd.pRootSignature = root.p; pd.VS = {vs->GetBufferPointer(), vs->GetBufferSize()};
        pd.PS = {ps->GetBufferPointer(), ps->GetBufferSize()};
        pd.BlendState.RenderTarget[0].RenderTargetWriteMask = D3D12_COLOR_WRITE_ENABLE_ALL;
        pd.SampleMask = UINT_MAX; pd.RasterizerState.FillMode = D3D12_FILL_MODE_SOLID;
        pd.RasterizerState.CullMode = D3D12_CULL_MODE_NONE; pd.RasterizerState.DepthClipEnable = TRUE;
        pd.PrimitiveTopologyType = D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE;
        pd.NumRenderTargets = 1; pd.RTVFormats[0] = DXGI_FORMAT_R8G8B8A8_UNORM; pd.SampleDesc.Count = 1;
        Owned<ID3D12PipelineState> pipeline;
        check(device->CreateGraphicsPipelineState(&pd, IID_PPV_ARGS(pipeline.address())), "graphics-pipeline");
        D3D12_DESCRIPTOR_HEAP_DESC hd = {}; hd.Type = D3D12_DESCRIPTOR_HEAP_TYPE_RTV; hd.NumDescriptors = 1;
        Owned<ID3D12DescriptorHeap> heap;
        check(device->CreateDescriptorHeap(&hd, IID_PPV_ARGS(heap.address())), "rtv-heap");
        auto rtv = heap->GetCPUDescriptorHandleForHeapStart();
        auto draw = [&](ID3D12Resource *image, UINT width, UINT height) {
            device->CreateRenderTargetView(image, nullptr, rtv);
            float black[] = {0, 0, 0, 1}; commands->ClearRenderTargetView(rtv, black, 0, nullptr);
            D3D12_VIEWPORT vp = {0, 0, (float)width, (float)height, 0, 1};
            D3D12_RECT scissor = {0, 0, (LONG)width, (LONG)height};
            commands->RSSetViewports(1, &vp); commands->RSSetScissorRects(1, &scissor);
            commands->SetGraphicsRootSignature(root.p); commands->SetPipelineState(pipeline.p);
            commands->OMSetRenderTargets(1, &rtv, FALSE, nullptr);
            commands->IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            if (!no_draw) commands->DrawInstanced(3, 1, 0, 0);
        };
        D3D12_RESOURCE_DESC td = {}; td.Dimension = D3D12_RESOURCE_DIMENSION_TEXTURE2D;
        td.Width = td.Height = 64; td.DepthOrArraySize = td.MipLevels = 1;
        td.Format = DXGI_FORMAT_R8G8B8A8_UNORM; td.SampleDesc.Count = 1;
        td.Flags = D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET;
        Owned<ID3D12Resource> texture, pixels;
        check(device->CreateCommittedResource(&gpu, D3D12_HEAP_FLAG_NONE, &td, D3D12_RESOURCE_STATE_RENDER_TARGET,
                                             nullptr, IID_PPV_ARGS(texture.address())), "texture");
        D3D12_PLACED_SUBRESOURCE_FOOTPRINT footprint = {}; UINT64 bytes = 0;
        device->GetCopyableFootprints(&td, 0, 1, 0, &footprint, nullptr, nullptr, &bytes);
        auto rb = buffer(bytes);
        check(device->CreateCommittedResource(&cpu, D3D12_HEAP_FLAG_NONE, &rb, D3D12_RESOURCE_STATE_COPY_DEST,
                                             nullptr, IID_PPV_ARGS(pixels.address())), "pixel-readback");
        draw(texture.p, 64, 64);
        barrier = transition(texture.p, D3D12_RESOURCE_STATE_RENDER_TARGET, D3D12_RESOURCE_STATE_COPY_SOURCE);
        commands->ResourceBarrier(1, &barrier);
        D3D12_TEXTURE_COPY_LOCATION src = {}, dst = {};
        src.pResource = texture.p; src.Type = D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX;
        dst.pResource = pixels.p; dst.Type = D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT; dst.PlacedFootprint = footprint;
        commands->CopyTextureRegion(&dst, 0, 0, 0, &src, nullptr); submit();
        BYTE *rgba = nullptr; range = {0, (SIZE_T)bytes};
        check(pixels->Map(0, &range, reinterpret_cast<void **>(&rgba)), "map-pixels");
        const BYTE *center = rgba + 32 * footprint.Footprint.RowPitch + 32 * 4;
        bool triangle_ok = center[0] == 0 && center[1] == 255 && center[2] == 0 &&
                           rgba[0] == 0 && rgba[1] == 0 && rgba[2] == 0;
        std::printf("NUCLEON_D3D12_PIXEL %u,%u,%u\n", center[0], center[1], center[2]);
        pixels->Unmap(0, &none);
        if (!triangle_ok) { std::puts("NUCLEON_D3D12_FAIL triangle-pixels"); CloseHandle(event); return 21; }
        std::puts("NUCLEON_D3D12_PASS triangle-readback");
        WNDCLASSW wc = {}; wc.lpfnWndProc = window_proc; wc.hInstance = GetModuleHandleW(nullptr);
        wc.lpszClassName = L"NucleonDX12Probe"; RegisterClassW(&wc);
        HWND window = CreateWindowW(wc.lpszClassName, L"Nucleon ARM64/FEX Direct3D 12", WS_OVERLAPPEDWINDOW,
                                    CW_USEDEFAULT, CW_USEDEFAULT, 320, 240, nullptr, nullptr, wc.hInstance, nullptr);
        if (!window) throw std::runtime_error("window");
        ShowWindow(window, SW_SHOW);
        DXGI_SWAP_CHAIN_DESC1 sd = {}; sd.Width = 128; sd.Height = 128; sd.Format = td.Format;
        sd.SampleDesc.Count = 1; sd.BufferUsage = DXGI_USAGE_RENDER_TARGET_OUTPUT;
        sd.BufferCount = 2; sd.SwapEffect = DXGI_SWAP_EFFECT_FLIP_DISCARD;
        Owned<IDXGISwapChain1> first;
        check(factory->CreateSwapChainForHwnd(queue.p, window, &sd, nullptr, nullptr, first.address()), "swapchain");
        Owned<IDXGISwapChain3> swap;
        check(first->QueryInterface(IID_PPV_ARGS(swap.address())), "swapchain3");
        for (UINT frame = 0; frame < 60; ++frame) {
            if (frame == 30) {
                check(swap->ResizeBuffers(2, 256, 192, td.Format, 0), "resize");
                std::puts("NUCLEON_D3D12_PASS resize");
            }
            Owned<ID3D12Resource> back;
            check(swap->GetBuffer(swap->GetCurrentBackBufferIndex(), IID_PPV_ARGS(back.address())), "backbuffer");
            barrier = transition(back.p, D3D12_RESOURCE_STATE_PRESENT, D3D12_RESOURCE_STATE_RENDER_TARGET);
            commands->ResourceBarrier(1, &barrier);
            draw(back.p, frame < 30 ? 128 : 256, frame < 30 ? 128 : 192);
            barrier = transition(back.p, D3D12_RESOURCE_STATE_RENDER_TARGET, D3D12_RESOURCE_STATE_PRESENT);
            commands->ResourceBarrier(1, &barrier); submit();
            check(swap->Present(1, 0), "present");
            MSG msg; while (PeekMessageW(&msg, nullptr, 0, 0, PM_REMOVE)) { TranslateMessage(&msg); DispatchMessageW(&msg); }
        }
        std::puts("NUCLEON_D3D12_PASS presentation");
        submit();
        swap.p->Release(); swap.p = nullptr;
        first.p->Release(); first.p = nullptr;
        DestroyWindow(window); CloseHandle(event);
        std::puts("NUCLEON_D3D12_OK");
        return 0;
    } catch (const std::exception &error) {
        std::printf("NUCLEON_D3D12_FAIL %s\n", error.what()); return 1;
    }
}
