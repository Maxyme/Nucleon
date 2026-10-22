#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <stdio.h>

typedef void *(__cdecl *create_interface_fn)(const char *, int *);
typedef int (__cdecl *create_pipe_fn)(void *);
typedef int (__cdecl *connect_user_fn)(void *, int);
typedef void (__cdecl *release_user_fn)(void *, int, int);
typedef int (__cdecl *release_pipe_fn)(void *, int);
int main(void) {
    static WCHAR result_path[32768];
    unsigned result_length = GetEnvironmentVariableW(L"NUCLEON_STEAM_RESULT",result_path,32768);
    if (result_length && result_length < 32768 && !_wfreopen(result_path,L"w",stdout)) return 19;
    setvbuf(stdout,NULL,_IONBF,0);
    HMODULE valve = LoadLibraryW(L"C:\\Program Files (x86)\\Steam\\steamclient64.dll");
    if (!valve) { printf("NUCLEON_STEAM_ROUTE_LOAD_FAILED %lu\n",GetLastError()); return 10; }
    create_interface_fn create = (create_interface_fn)(void *)GetProcAddress(valve,"CreateInterface");
    HMODULE owner = NULL;
    if (!create || !GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
                                      (LPCWSTR)(void *)create,&owner) || owner != valve) {
        puts("NUCLEON_STEAM_ROUTE_FAIL export-owner"); return 11;
    }
    void *client = create("SteamClient017",NULL);
    if (!client) { puts("NUCLEON_STEAM_ROUTE_FAIL interface"); return 12; }
    void **table = *(void ***)client;
    int pipe = ((create_pipe_fn)table[0])(client);
    if (!pipe) { puts("NUCLEON_STEAM_ROUTE_FAIL pipe"); return 13; }
    int user = ((connect_user_fn)table[2])(client,pipe);
    if (!user) { ((release_pipe_fn)table[1])(client,pipe); puts("NUCLEON_STEAM_ROUTE_FAIL user"); return 14; }
    ((release_user_fn)table[4])(client,pipe,user);
    ((release_pipe_fn)table[1])(client,pipe);
    static WCHAR api_path[32768];
    unsigned length = GetEnvironmentVariableW(L"NUCLEON_STEAM_API",api_path,32768);
    if (!length || length >= 32768) { puts("NUCLEON_STEAM_ROUTE_FAIL missing-api-path"); return 16; }
    HMODULE api = LoadLibraryW(api_path);
    typedef unsigned char (__cdecl *steam_bool_fn)(void);
    typedef void (__cdecl *steam_void_fn)(void);
    steam_bool_fn running = api ? (steam_bool_fn)(void *)GetProcAddress(api,"SteamAPI_IsSteamRunning") : NULL;
    steam_bool_fn init = api ? (steam_bool_fn)(void *)GetProcAddress(api,"SteamAPI_Init") : NULL;
    steam_void_fn shutdown = api ? (steam_void_fn)(void *)GetProcAddress(api,"SteamAPI_Shutdown") : NULL;
    if (!running || !init || !shutdown) { puts("NUCLEON_STEAM_ROUTE_FAIL api-exports"); return 17; }
    if (!running()) { puts("NUCLEON_STEAM_NOT_RUNNING"); return 15; }
    if (!init()) { puts("NUCLEON_STEAM_ROUTE_FAIL api-init"); return 18; }
    puts("NUCLEON_STEAM_API_OK running-and-initialized");
    shutdown();
    FreeLibrary(api);
    FreeLibrary(valve);
    puts("NUCLEON_STEAM_ROUTE_OK valve-export-owner-native-steam-session"); return 0;
}
