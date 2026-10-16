/* Exercise a hardware D3D11 device, GPU copy and synchronized CPU readback. */
#define COBJMACROS
#include <d3d11.h>
#include <d3dcompiler.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <wchar.h>
#include "graphics-abi.h"

static HRESULT compile_shader(const char *source, const char *target, ID3DBlob **blob)
{
    ID3DBlob *errors = NULL;
    HRESULT result = D3DCompile(source, strlen(source), "nucleon-triangle", NULL, NULL,
                               "main", target, 0, 0, blob, &errors);
    if (errors)
    {
        fprintf(stderr, "%.*s", (int)ID3D10Blob_GetBufferSize(errors),
                (const char *)ID3D10Blob_GetBufferPointer(errors));
        ID3D10Blob_Release(errors);
    }
    return result;
}

static int triangle(ID3D11Device *device, ID3D11DeviceContext *context, int draw)
{
    ID3DBlob *vertex_code = NULL, *pixel_code = NULL;
    HRESULT result = compile_shader(
        "float4 main(float2 position : POSITION) : SV_Position { return float4(position,0,1); }",
        "vs_4_0", &vertex_code);
    if (FAILED(result)) return 10;
    result = compile_shader("float4 main() : SV_Target { return float4(0,1,0,1); }", "ps_4_0", &pixel_code);
    if (FAILED(result)) return 11;
    ID3D11VertexShader *vertex_shader = NULL;
    ID3D11PixelShader *pixel_shader = NULL;
    ID3D11InputLayout *layout = NULL;
    const void *vertex_bytes = ID3D10Blob_GetBufferPointer(vertex_code);
    SIZE_T vertex_length = ID3D10Blob_GetBufferSize(vertex_code);
    if (FAILED(ID3D11Device_CreateVertexShader(device, vertex_bytes, vertex_length, NULL, &vertex_shader))) return 12;
    if (FAILED(ID3D11Device_CreatePixelShader(device, ID3D10Blob_GetBufferPointer(pixel_code),
        ID3D10Blob_GetBufferSize(pixel_code), NULL, &pixel_shader))) return 13;
    D3D11_INPUT_ELEMENT_DESC element = {"POSITION", 0, DXGI_FORMAT_R32G32_FLOAT, 0, 0,
                                       D3D11_INPUT_PER_VERTEX_DATA, 0};
    if (FAILED(ID3D11Device_CreateInputLayout(device, &element, 1, vertex_bytes, vertex_length, &layout))) return 14;
    const float positions[] = {0.0f, 0.8f, 0.8f, -0.8f, -0.8f, -0.8f};
    D3D11_BUFFER_DESC buffer_desc = {0};
    buffer_desc.ByteWidth = sizeof(positions);
    buffer_desc.Usage = D3D11_USAGE_IMMUTABLE;
    buffer_desc.BindFlags = D3D11_BIND_VERTEX_BUFFER;
    D3D11_SUBRESOURCE_DATA buffer_data = {positions, 0, 0};
    ID3D11Buffer *vertices = NULL;
    if (FAILED(ID3D11Device_CreateBuffer(device, &buffer_desc, &buffer_data, &vertices))) return 15;
    D3D11_TEXTURE2D_DESC desc = {0};
    desc.Width = desc.Height = 64;
    desc.MipLevels = desc.ArraySize = 1;
    desc.Format = DXGI_FORMAT_R8G8B8A8_UNORM;
    desc.SampleDesc.Count = 1;
    desc.Usage = D3D11_USAGE_DEFAULT;
    desc.BindFlags = D3D11_BIND_RENDER_TARGET;
    ID3D11Texture2D *target = NULL, *staging = NULL;
    ID3D11RenderTargetView *view = NULL;
    if (FAILED(ID3D11Device_CreateTexture2D(device, &desc, NULL, &target))) return 16;
    if (FAILED(ID3D11Device_CreateRenderTargetView(device, (ID3D11Resource *)target, NULL, &view))) return 17;
    desc.Usage = D3D11_USAGE_STAGING;
    desc.BindFlags = 0;
    desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ;
    if (FAILED(ID3D11Device_CreateTexture2D(device, &desc, NULL, &staging))) return 18;
    D3D11_RASTERIZER_DESC rasterizer_desc = {0};
    rasterizer_desc.FillMode = D3D11_FILL_SOLID;
    rasterizer_desc.CullMode = D3D11_CULL_NONE;
    rasterizer_desc.DepthClipEnable = TRUE;
    ID3D11RasterizerState *rasterizer = NULL;
    if (FAILED(ID3D11Device_CreateRasterizerState(device, &rasterizer_desc, &rasterizer))) return 19;
    D3D11_VIEWPORT viewport = {0, 0, 64, 64, 0, 1};
    UINT stride = 2 * sizeof(float), offset = 0;
    const float black[] = {0, 0, 0, 1};
    ID3D11DeviceContext_IASetInputLayout(context, layout);
    ID3D11DeviceContext_IASetPrimitiveTopology(context, D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
    ID3D11DeviceContext_IASetVertexBuffers(context, 0, 1, &vertices, &stride, &offset);
    ID3D11DeviceContext_VSSetShader(context, vertex_shader, NULL, 0);
    ID3D11DeviceContext_PSSetShader(context, pixel_shader, NULL, 0);
    ID3D11DeviceContext_RSSetState(context, rasterizer);
    ID3D11DeviceContext_RSSetViewports(context, 1, &viewport);
    ID3D11DeviceContext_OMSetRenderTargets(context, 1, &view, NULL);
    ID3D11DeviceContext_ClearRenderTargetView(context, view, black);
    ID3D11DeviceContext_Draw(context, draw ? 3 : 0, 0);
    ID3D11DeviceContext_CopyResource(context, (ID3D11Resource *)staging, (ID3D11Resource *)target);
    D3D11_MAPPED_SUBRESOURCE mapped;
    if (FAILED(ID3D11DeviceContext_Map(context, (ID3D11Resource *)staging, 0, D3D11_MAP_READ, 0, &mapped))) return 20;
    const uint32_t *center = (const uint32_t *)((const char *)mapped.pData + 32 * mapped.RowPitch);
    if (center[32] != 0xff00ff00 || *(const uint32_t *)mapped.pData != 0xff000000) return 21;
    FILE *image = fopen("triangle.ppm", "wb");
    if (!image) return 22;
    fprintf(image, "P6\n64 64\n255\n");
    for (unsigned y = 0; y < 64; ++y)
        for (unsigned x = 0; x < 64; ++x)
            if (fwrite((const char *)mapped.pData + y * mapped.RowPitch + x * 4, 1, 3, image) != 3) return 23;
    if (fclose(image)) return 24;
    ID3D11DeviceContext_Unmap(context, (ID3D11Resource *)staging, 0);
    ID3D11DeviceContext_ClearState(context);
    ID3D11RasterizerState_Release(rasterizer);
    ID3D11RenderTargetView_Release(view);
    ID3D11Texture2D_Release(staging);
    ID3D11Texture2D_Release(target);
    ID3D11Buffer_Release(vertices);
    ID3D11InputLayout_Release(layout);
    ID3D11PixelShader_Release(pixel_shader);
    ID3D11VertexShader_Release(vertex_shader);
    ID3D10Blob_Release(pixel_code);
    ID3D10Blob_Release(vertex_code);
    puts("NUCLEON_D3D11_TRIANGLE_OK");
    return 0;
}

int wmain(int argc, wchar_t **argv)
{
    ID3D11Device *device = NULL;
    ID3D11DeviceContext *context = NULL;
    D3D_FEATURE_LEVEL requested = D3D_FEATURE_LEVEL_11_0, selected = 0;
    HRESULT result = D3D11CreateDevice(NULL, D3D_DRIVER_TYPE_HARDWARE, NULL, 0,
        &requested, 1, D3D11_SDK_VERSION, &device, &selected, &context);
    if (FAILED(result))
    {
        fprintf(stderr, "NUCLEON_D3D11_FAIL create-device hr=%08lx\n", (unsigned long)result);
        return 1;
    }
    printf("NUCLEON_D3D11_DEVICE feature_level=%04x\n", selected);
    if (!graphics_module_abi("d3d11.dll") || !graphics_module_abi("dxgi.dll")) return 6;
    if (getenv("NUCLEON_EXPECT_METAL") && !graphics_module_abi("winemetal.dll")) return 6;
    uint32_t pixels[16];
    for (unsigned i = 0; i < 16; ++i) pixels[i] = 0xff000000u | (i * 0x0005070bu);
    D3D11_TEXTURE2D_DESC desc = {0};
    desc.Width = desc.Height = 4;
    desc.MipLevels = desc.ArraySize = 1;
    desc.Format = DXGI_FORMAT_R8G8B8A8_UNORM;
    desc.SampleDesc.Count = 1;
    desc.Usage = D3D11_USAGE_DEFAULT;
    desc.BindFlags = D3D11_BIND_SHADER_RESOURCE;
    D3D11_SUBRESOURCE_DATA initial = {pixels, 16, 0};
    ID3D11Texture2D *texture = NULL, *staging = NULL;
    result = ID3D11Device_CreateTexture2D(device, &desc, &initial, &texture);
    if (FAILED(result)) return 2;
    desc.Usage = D3D11_USAGE_STAGING;
    desc.BindFlags = 0;
    desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ;
    result = ID3D11Device_CreateTexture2D(device, &desc, NULL, &staging);
    if (FAILED(result)) return 3;
    ID3D11DeviceContext_CopyResource(context, (ID3D11Resource *)staging, (ID3D11Resource *)texture);
    D3D11_MAPPED_SUBRESOURCE mapped;
    result = ID3D11DeviceContext_Map(context, (ID3D11Resource *)staging, 0, D3D11_MAP_READ, 0, &mapped);
    if (FAILED(result)) return 4;
    for (unsigned y = 0; y < 4; ++y)
        for (unsigned x = 0; x < 4; ++x)
            if (((const uint32_t *)((const char *)mapped.pData + y * mapped.RowPitch))[x] != pixels[y * 4 + x])
                return 5;
    ID3D11DeviceContext_Unmap(context, (ID3D11Resource *)staging, 0);
    ID3D11Texture2D_Release(staging);
    ID3D11Texture2D_Release(texture);
    puts("NUCLEON_D3D11_GPU_COPY_OK");
    int triangle_result = triangle(device, context, !(argc == 2 && !wcscmp(argv[1], L"--no-draw")));
    ID3D11DeviceContext_Release(context);
    ID3D11Device_Release(device);
    if (triangle_result) fprintf(stderr, "NUCLEON_D3D11_FAIL triangle-stage=%d\n", triangle_result);
    return triangle_result;
}
