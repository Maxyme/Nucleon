/* Owned Darwin companion ABI probe. Does not load Wine or start Steam. */
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <dlfcn.h>
#include <errno.h>
#include <fcntl.h>
#include <sys/mman.h>
#include <unistd.h>

typedef uint32_t (*unix_call)(void *);
struct advice { void *address; size_t size; int32_t advice; uint32_t pad; };
struct mapping { int32_t fd; uint64_t size; void *result; };
#define CHECK(test) do { if (!(test)) { fprintf(stderr, "FEX_UNIXLIB_FAIL line=%d\n", __LINE__); return 1; } } while (0)

int main(int argc, char **argv)
{
    CHECK(argc == 2);
    void *library = dlopen(argv[1], RTLD_NOW | RTLD_LOCAL);
    CHECK(library != NULL);
    unix_call *calls = (unix_call *)dlsym(library, "__wine_unix_call_funcs");
    CHECK(calls != NULL);
    uint32_t pid = 0;
    CHECK(calls[7](&pid) == 0 && pid == (uint32_t)getpid());
    bool enabled = true;
    uint64_t flags = 1;
    struct advice advice = {0};
    advice.advice = 14;
    CHECK(calls[0](&enabled) == 0xc00000bb);
    CHECK(calls[1](&flags) == 0xc00000bb);
    CHECK(calls[2](&advice) == 0xc00000bb);
    FILE *file = tmpfile();
    CHECK(file != NULL);
    CHECK(fwrite("FEX ABI", 1, 7, file) == 7 && fflush(file) == 0);
    struct mapping mapping = {0};
    mapping.fd = dup(fileno(file));
    mapping.size = 7;
    CHECK(mapping.fd >= 0 && calls[6](&mapping) == 0);
    CHECK(mapping.result != NULL && memcmp(mapping.result, "FEX ABI", 7) == 0);
    errno = 0;
    CHECK(fcntl(mapping.fd, F_GETFD) == -1 && errno == EBADF);
    CHECK(munmap(mapping.result, 7) == 0);
    CHECK(fclose(file) == 0 && dlclose(library) == 0);
    puts("NUCLEON_FEX_UNIXLIB_OK");
    return 0;
}
