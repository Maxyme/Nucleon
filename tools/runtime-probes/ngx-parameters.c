/* GPTK beta 3 NGX parameter probe, without a game or OptiScaler.
 * Build: x86_64-w64-mingw32-gcc -Wall -Wextra -Werror ngx-parameters.c -o ngx-parameters.exe
 * Pass the selected runtime's nvngx.dll as a Windows path (Z:\\runtime\\nvngx.dll).
 * The Microsoft overload order is verified against the beta 3 native vtable;
 * it differs from the declaration order in NVIDIA's public NGX header.
 */
#include <windows.h>
#include <stdio.h>

typedef unsigned int (WINAPI *allocate_fn)(void **);
typedef unsigned int (WINAPI *destroy_fn)(void *);
typedef void (WINAPI *set_ui_fn)(void *, const char *, unsigned int);
typedef void (WINAPI *set_f_fn)(void *, const char *, float);
typedef void (WINAPI *set_d_fn)(void *, const char *, double);
typedef unsigned int (WINAPI *get_ui_fn)(void *, const char *, unsigned int *);
typedef unsigned int (WINAPI *get_f_fn)(void *, const char *, float *);
typedef unsigned int (WINAPI *get_d_fn)(void *, const char *, double *);

int main(int argc, char **argv)
{
    setvbuf(stdout, NULL, _IONBF, 0);
    if (argc != 2) return 2;
    HMODULE module = LoadLibraryA(argv[1]);
    if (!module) {
        printf("load failed: %lu\n", GetLastError());
        return 3;
    }
    allocate_fn allocate = (allocate_fn)(void *)GetProcAddress(module, "NVSDK_NGX_D3D12_AllocateParameters");
    destroy_fn destroy = (destroy_fn)(void *)GetProcAddress(module, "NVSDK_NGX_D3D12_DestroyParameters");
    if (!allocate || !destroy) return 4;

    void *params = NULL;
    unsigned int result = allocate(&params);
    printf("allocate=%x ptr=%p\n", result, params);
    if (result != 1 || !params) return 5;
    void **vtable = *(void ***)params;
    for (unsigned int n = 0; n < 10000; ++n) {
        ((set_ui_fn)vtable[4])(params, "probe.ui", n);
        ((set_f_fn)vtable[6])(params, "probe.float", 0.25f);
        ((set_d_fn)vtable[5])(params, "probe.double", 0.125);
        unsigned int integer = 0;
        float single = 0;
        double dbl = 0;
        if (((get_ui_fn)vtable[12])(params, "probe.ui", &integer) != 1 || integer != n ||
            ((get_f_fn)vtable[14])(params, "probe.float", &single) != 1 || single != 0.25f ||
            ((get_d_fn)vtable[13])(params, "probe.double", &dbl) != 1 || dbl != 0.125) {
            printf("roundtrip failed: %u %.9g %.17g\n", integer, (double)single, dbl);
            destroy(params);
            return 6;
        }
    }
    printf("10000 NGX integer/float/double roundtrips passed\n");
    result = destroy(params);
    printf("destroy=%x\n", result);
    return result == 1 ? 0 : 7;
}
