/* Observe the address mapping required by the selected Wine loader. */
#include <errno.h>
#include <stdio.h>
#include <sys/mman.h>
#include <sys/sysctl.h>
#include <unistd.h>

int main(void)
{
    int translated = 0;
    size_t length = sizeof(translated);
    void *address = (void *)0x10000;
    size_t size = 0xffff0000;
    sysctlbyname("sysctl.proc_translated", &translated, &length, NULL, 0);
    errno = 0;
    void *mapped = mmap(address, size, PROT_NONE,
                        MAP_FIXED | MAP_NORESERVE | MAP_PRIVATE | MAP_ANON, -1, 0);
    int error = mapped == MAP_FAILED ? errno : 0;
    printf("{\"native_arm64\":%s,\"translated\":%s,\"page_size\":%ld,"
           "\"low_address_mapping\":%s,\"mapping_errno\":%d}\n",
#if defined(__aarch64__)
           "true",
#else
           "false",
#endif
           translated ? "true" : "false", sysconf(_SC_PAGESIZE),
           mapped == address ? "true" : "false", error);
    if (mapped != MAP_FAILED) munmap(mapped, size);
    return translated ? 1 : 0;
}
