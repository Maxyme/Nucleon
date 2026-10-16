/* Verify the actual modules used by an AMD64 caller, rather than just build files. */
#include <stdlib.h>
#include <ctype.h>
static int graphics_module_abi(const char *name)
{
    const char *expected = getenv("NUCLEON_EXPECT_GRAPHICS_ABI");
    if (!expected) return 1;
    HMODULE module = GetModuleHandleA(name);
    if (!module) { fprintf(stderr, "NUCLEON_FRONTEND_FAIL missing %s\n", name); return 0; }
    const BYTE *base = (const BYTE *)module;
    const IMAGE_DOS_HEADER *dos = (const IMAGE_DOS_HEADER *)base;
    const IMAGE_NT_HEADERS64 *nt = (const IMAGE_NT_HEADERS64 *)(base + dos->e_lfanew);
    const IMAGE_DATA_DIRECTORY *entry = &nt->OptionalHeader.DataDirectory[IMAGE_DIRECTORY_ENTRY_LOAD_CONFIG];
    int hybrid = 0;
    if (entry->VirtualAddress && entry->Size >= 208) {
        const BYTE *config = base + entry->VirtualAddress;
        DWORD size;
        ULONGLONG chpe;
        memcpy(&size, config, sizeof(size));
        memcpy(&chpe, config + 200, sizeof(chpe));
        hybrid = size >= 208 && chpe != 0;
    }
    unsigned machine = nt->FileHeader.Machine;
    int native = machine == 0xa641 || machine == 0xa64e || hybrid;
    char path[MAX_PATH];
    if (!GetModuleFileNameA(module, path, sizeof(path))) return 0;
    printf("NUCLEON_FRONTEND_MODULE %s machine=%04x hybrid=%d path=%s\n", name, machine, hybrid, path);
    char key[96], identity[64];
    snprintf(key, sizeof(key), "NUCLEON_IMAGE_%s", name);
    for (char *p = key; *p; ++p) *p = *p == '.' ? '_' : (char)toupper((unsigned char)*p);
    const char *expected_identity = getenv(key);
    snprintf(identity, sizeof(identity), "%08lx:%08lx:%08lx", (unsigned long)nt->FileHeader.TimeDateStamp,
             (unsigned long)nt->OptionalHeader.SizeOfImage, (unsigned long)nt->OptionalHeader.CheckSum);
    if (expected_identity && strcmp(identity, expected_identity)) {
        fprintf(stderr, "NUCLEON_FRONTEND_FAIL wrong component %s image=%s expected=%s\n", name, identity, expected_identity);
        return 0;
    }
    printf("NUCLEON_FRONTEND_IMAGE %s %s\n", name, identity);
    if ((!strcmp(expected, "arm64ec") && !native) ||
        (!strcmp(expected, "x64") && (machine != 0x8664 || native))) {
        fprintf(stderr, "NUCLEON_FRONTEND_FAIL unexpected ABI %s\n", name);
        return 0;
    }
    return 1;
}
