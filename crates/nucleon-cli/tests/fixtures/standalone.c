/* Small owned console workload for optional native standalone integration checks. */
#include <windows.h>
#include <stdio.h>
#include <wchar.h>

int wmain(int argc, wchar_t **argv)
{
    wchar_t cwd[32768], value[64];
    GetCurrentDirectoryW(32768, cwd);
    wprintf(L"CWD=%ls\n", cwd);
    wprintf(L"STEAM_ID=%ls\n", GetEnvironmentVariableW(L"SteamAppId", value, 64) ? value : L"unset");
    wprintf(L"STEAM_DATA=%ls\n", GetEnvironmentVariableW(L"STEAM_COMPAT_DATA_PATH", value, 64) ? value : L"unset");
    wprintf(L"STEAM_REDIRECT=%ls\n", GetEnvironmentVariableW(L"NUCLEON_STEAMCLIENT_REDIRECT", value, 64) ? value : L"unset");
    for (int i = 1; i < argc; i++) wprintf(L"ARG[%d]=<%ls>\n", i, argv[i]);
    FILE *arguments = fopen("C:\\nucleon-arguments.bin", "wb");
    if (!arguments) return 8;
    for (int i = 1; i < argc; i++) fwrite(argv[i], sizeof(wchar_t), wcslen(argv[i]) + 1, arguments);
    fclose(arguments);
    if (argc > 1 && !wcscmp(argv[1], L"--write")) {
        FILE *file = fopen("C:\\nucleon-shared.txt", "wb");
        if (!file) return 3;
        fputs("installed", file);
        fclose(file);
    }
    if (argc > 1 && !wcscmp(argv[1], L"--read")) {
        char data[32] = {0};
        FILE *file = fopen("C:\\nucleon-shared.txt", "rb");
        if (!file) return 4;
        fread(data, 1, sizeof(data) - 1, file);
        fclose(file);
        wprintf(L"SHARED=%hs\n", data);
    }
    if (argc > 1 && !wcscmp(argv[1], L"--spawn")) {
        wchar_t path[32768], child[32768], command[32768];
        GetModuleFileNameW(NULL, path, 32768);
        wcscpy(child, path);
        wchar_t *slash = wcsrchr(child, L'\\');
        if (!slash) return 5;
        wcscpy(slash + 1, L"renamed-helper.exe");
        if (!CopyFileW(path, child, FALSE)) return 6;
        swprintf(command, 32768, L"\"%ls\" --child", child);
        STARTUPINFOW startup = { .cb = sizeof(startup) };
        PROCESS_INFORMATION process;
        fflush(stdout);
        if (!CreateProcessW(child, command, NULL, NULL, TRUE, 0, NULL, NULL, &startup, &process)) return 7;
        CloseHandle(process.hThread);
        CloseHandle(process.hProcess);
    }
    if (argc > 1 && !wcscmp(argv[1], L"--child")) {
        Sleep(2500);
        wprintf(L"CHILD_COMPLETED\n");
    }
    if (argc > 1 && !wcscmp(argv[1], L"--hold")) {
        wprintf(L"HOLD_READY\n");
        fflush(stdout);
        Sleep(30000);
    }
    return argc > 1 && !wcscmp(argv[1], L"--exit17") ? 17 : 0;
}
