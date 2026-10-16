/* ARM64EC DLL called concurrently by the AMD64 test below. */
#include <windows.h>
#include <stdint.h>
static __thread unsigned counter = 7;
static __thread unsigned char bytes[64] __attribute__((aligned(64)));
__declspec(dllexport) unsigned tls_step(unsigned expected, unsigned next)
{
    if (counter != expected || ((uintptr_t)bytes & 63)) return 1;
    if (expected == 7) {
        for (unsigned i = 0; i < sizeof(bytes); ++i) if (bytes[i]) return 2;
    } else if (bytes[0] != (unsigned char)expected) return 3;
    counter = next;
    bytes[0] = (unsigned char)next;
    return 0;
}
