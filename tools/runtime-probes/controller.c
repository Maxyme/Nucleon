/* Owned XInput diagnostic: run with the same Wine runtime/prefix as the game.
 * Exit 0 means a controller was detected, not that gameplay input is verified.
 */
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <xinput.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

typedef DWORD (WINAPI *get_state_fn)(DWORD, XINPUT_STATE *);

int main(int argc, char **argv)
{
    const char *libraries[] = {"xinput1_3.dll", "xinput1_4.dll", "xinput9_1_0.dll"};
    unsigned long seconds = 15;
    if (argc > 2) return 2;
    if (argc == 2)
    {
        char *end;
        seconds = strtoul(argv[1], &end, 10);
        if (!*argv[1] || *end || seconds < 1 || seconds > 300) return 2;
    }
    setvbuf(stdout, NULL, _IONBF, 0);
    for (unsigned n = 0; n < sizeof(libraries) / sizeof(libraries[0]); ++n)
    {
        HMODULE module = LoadLibraryA(libraries[n]);
        get_state_fn get_state = module ? (get_state_fn)(void *)GetProcAddress(module, "XInputGetState") : NULL;
        if (!get_state)
            printf("NUCLEON_XINPUT_LOAD_FAILED library=%s error=%lu\n", libraries[n], GetLastError());
        else
            for (DWORD slot = 0; slot < XUSER_MAX_COUNT; ++slot)
            {
                XINPUT_STATE state = {0};
                DWORD status = get_state(slot, &state);
                printf("NUCLEON_XINPUT_DEVICE library=%s slot=%lu status=%lu\n", libraries[n], slot, status);
            }
        if (module) FreeLibrary(module);
    }

    HMODULE module = LoadLibraryA(libraries[0]);
    get_state_fn get_state = module ? (get_state_fn)(void *)GetProcAddress(module, "XInputGetState") : NULL;
    if (!get_state)
    {
        if (module) FreeLibrary(module);
        return 2;
    }
    XINPUT_GAMEPAD previous[XUSER_MAX_COUNT] = {0};
    unsigned connected = 0, changed = 0;
    ULONGLONG end = GetTickCount64() + seconds * 1000;
    while (GetTickCount64() < end)
    {
        for (DWORD slot = 0; slot < XUSER_MAX_COUNT; ++slot)
        {
            XINPUT_STATE state = {0};
            if (get_state(slot, &state) != ERROR_SUCCESS) continue;
            unsigned bit = 1u << slot;
            int different = memcmp(&previous[slot], &state.Gamepad, sizeof(state.Gamepad));
            if ((connected & bit) && different) changed |= bit;
            if (!(connected & bit) || different)
                printf("NUCLEON_XINPUT_STATE slot=%lu buttons=%04x LT=%u RT=%u LX=%d LY=%d RX=%d RY=%d\n",
                       slot, state.Gamepad.wButtons, state.Gamepad.bLeftTrigger, state.Gamepad.bRightTrigger,
                       state.Gamepad.sThumbLX, state.Gamepad.sThumbLY, state.Gamepad.sThumbRX, state.Gamepad.sThumbRY);
            connected |= bit;
            previous[slot] = state.Gamepad;
        }
        Sleep(50);
    }
    printf("NUCLEON_XINPUT_RESULT connected_mask=%x changed_mask=%x\n", connected, changed);
    FreeLibrary(module);
    return connected ? 0 : 1;
}
