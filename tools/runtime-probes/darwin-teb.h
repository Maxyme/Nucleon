/* Only for the pinned Darwin Wine/FEX ABI, including rebuilt MinGW CRT startup. */
#pragma once
#if defined(__arm64ec__) || defined(__aarch64__)
#include <windef.h>
#ifndef NUCLEON_TEB_TSD_OFFSET
#error "The paired Wine/FEX TEB TSD offset is required"
#endif
#define NUCLEON_STRINGIFY_(x) #x
#define NUCLEON_STRINGIFY(x) NUCLEON_STRINGIFY_(x)
static __inline__ struct _TEB *nucleon_current_teb(void)
{
    struct _TEB *teb;
    __asm__("mrs %0, tpidrro_el0\n\tldr %0, [%0, #" NUCLEON_STRINGIFY(NUCLEON_TEB_TSD_OFFSET) "]" : "=r"(teb));
    return teb;
}
#undef NtCurrentTeb
#define NtCurrentTeb() nucleon_current_teb()
#endif
