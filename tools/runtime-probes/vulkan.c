#define WIN32_LEAN_AND_MEAN
#define VK_USE_PLATFORM_WIN32_KHR
#define VK_NO_PROTOTYPES
#include <windows.h>
#include <vulkan/vulkan.h>
#include <stdio.h>
#include <stdlib.h>

#define GET(name) PFN_vk##name name = (PFN_vk##name)get(instance, "vk" #name); if (!name) return 2
#define OK(call) do { VkResult r = (call); if (r != VK_SUCCESS) { printf("NUCLEON_VULKAN_FAIL %s %d\n", #call, r); return 3; } } while (0)
static LRESULT CALLBACK procedure(HWND w, UINT m, WPARAM a, LPARAM b) { return DefWindowProcW(w,m,a,b); }
int main(void) {
    HMODULE dll = LoadLibraryW(L"vulkan-1.dll");
    if (!dll) { printf("NUCLEON_VULKAN_FAIL load %lu\n", GetLastError()); return 1; }
    PFN_vkGetInstanceProcAddr get = (PFN_vkGetInstanceProcAddr)(void *)GetProcAddress(dll,"vkGetInstanceProcAddr");
    if (!get) return 2;
    VkInstance instance = VK_NULL_HANDLE;
    GET(CreateInstance);
    const char *extensions[] = {VK_KHR_SURFACE_EXTENSION_NAME, VK_KHR_WIN32_SURFACE_EXTENSION_NAME};
    VkApplicationInfo app = {.sType=VK_STRUCTURE_TYPE_APPLICATION_INFO,.pApplicationName="Nucleon FEX Vulkan probe",.apiVersion=VK_API_VERSION_1_3};
    VkInstanceCreateInfo info = {.sType=VK_STRUCTURE_TYPE_INSTANCE_CREATE_INFO,.pApplicationInfo=&app,
                                .enabledExtensionCount=2,.ppEnabledExtensionNames=extensions};
    OK(CreateInstance(&info,NULL,&instance));
    GET(EnumeratePhysicalDevices); GET(GetPhysicalDeviceProperties); GET(GetPhysicalDeviceFeatures2); GET(GetPhysicalDeviceProperties2);
    GET(GetPhysicalDeviceQueueFamilyProperties); GET(CreateDevice); GET(DestroyDevice); GET(DestroyInstance);
    GET(CreateWin32SurfaceKHR); GET(DestroySurfaceKHR); GET(GetPhysicalDeviceSurfaceSupportKHR);
    VkPhysicalDevice physical = VK_NULL_HANDLE; uint32_t count=1;
    OK(EnumeratePhysicalDevices(instance,&count,&physical)); if (!count) return 4;
    VkPhysicalDeviceProperties properties; GetPhysicalDeviceProperties(physical,&properties);
    VkPhysicalDeviceDriverProperties driver={.sType=VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_DRIVER_PROPERTIES};
    VkPhysicalDeviceProperties2 details={.sType=VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_PROPERTIES_2,.pNext=&driver};
    GetPhysicalDeviceProperties2(physical,&details);
    printf("NUCLEON_VULKAN_DRIVER %s %s\n",driver.driverName,driver.driverInfo);
    printf("NUCLEON_VULKAN_DEVICE %s api=%u.%u.%u\n",properties.deviceName,
           VK_VERSION_MAJOR(properties.apiVersion),VK_VERSION_MINOR(properties.apiVersion),VK_VERSION_PATCH(properties.apiVersion));
    VkPhysicalDeviceMutableDescriptorTypeFeaturesEXT mut = {.sType=VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_MUTABLE_DESCRIPTOR_TYPE_FEATURES_EXT};
    VkPhysicalDeviceTransformFeedbackFeaturesEXT xfb = {.sType=VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_TRANSFORM_FEEDBACK_FEATURES_EXT,.pNext=&mut};
    VkPhysicalDeviceRobustness2FeaturesEXT robust = {.sType=VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_ROBUSTNESS_2_FEATURES_EXT,.pNext=&xfb};
    VkPhysicalDeviceVulkan13Features f13 = {.sType=VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_VULKAN_1_3_FEATURES,.pNext=&robust};
    VkPhysicalDeviceVulkan12Features f12 = {.sType=VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_VULKAN_1_2_FEATURES,.pNext=&f13};
    VkPhysicalDeviceVulkan11Features f11 = {.sType=VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_VULKAN_1_1_FEATURES,.pNext=&f12};
    VkPhysicalDeviceFeatures2 f2 = {.sType=VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_FEATURES_2,.pNext=&f11};
    GetPhysicalDeviceFeatures2(physical,&f2);
    printf("NUCLEON_VULKAN_FEATURES robust=%u null=%u xfb=%u mutable=%u bda=%u geometry=%u\n",robust.robustBufferAccess2,robust.nullDescriptor,xfb.transformFeedback,mut.mutableDescriptorType,f12.bufferDeviceAddress,f2.features.geometryShader);
    if (!robust.robustBufferAccess2 || !robust.nullDescriptor || !xfb.transformFeedback || !mut.mutableDescriptorType || !f12.bufferDeviceAddress)
        { puts("NUCLEON_VULKAN_FAIL required-features"); return 5; }
    WNDCLASSW wc = {0}; wc.lpfnWndProc=procedure; wc.hInstance=GetModuleHandleW(NULL); wc.lpszClassName=L"NucleonVulkanProbe";
    RegisterClassW(&wc);
    HWND window = CreateWindowW(wc.lpszClassName,L"Nucleon ARM64 Vulkan probe",WS_OVERLAPPEDWINDOW,
                                 CW_USEDEFAULT,CW_USEDEFAULT,160,120,NULL,NULL,wc.hInstance,NULL);
    if (!window) return 6;
    VkWin32SurfaceCreateInfoKHR si={.sType=VK_STRUCTURE_TYPE_WIN32_SURFACE_CREATE_INFO_KHR,.hinstance=wc.hInstance,.hwnd=window};
    VkSurfaceKHR surface; OK(CreateWin32SurfaceKHR(instance,&si,NULL,&surface));
    GetPhysicalDeviceQueueFamilyProperties(physical,&count,NULL);
    VkQueueFamilyProperties *families=calloc(count,sizeof(*families));
    if (!families) return 7;
    GetPhysicalDeviceQueueFamilyProperties(physical,&count,families);
    uint32_t family=count;
    for (uint32_t i=0;i<count;i++) { VkBool32 supported=VK_FALSE;
        OK(GetPhysicalDeviceSurfaceSupportKHR(physical,i,surface,&supported));
        if ((families[i].queueFlags & VK_QUEUE_GRAPHICS_BIT) && supported) { family=i; break; } }
    free(families); if (family==count) return 8;
    float priority=1;
    VkDeviceQueueCreateInfo qi={.sType=VK_STRUCTURE_TYPE_DEVICE_QUEUE_CREATE_INFO,.queueFamilyIndex=family,.queueCount=1,.pQueuePriorities=&priority};
    const char *de[]={VK_KHR_SWAPCHAIN_EXTENSION_NAME,VK_EXT_ROBUSTNESS_2_EXTENSION_NAME,VK_EXT_TRANSFORM_FEEDBACK_EXTENSION_NAME,VK_EXT_MUTABLE_DESCRIPTOR_TYPE_EXTENSION_NAME};
    VkDeviceCreateInfo di={.sType=VK_STRUCTURE_TYPE_DEVICE_CREATE_INFO,.pNext=&f2,.queueCreateInfoCount=1,.pQueueCreateInfos=&qi,.enabledExtensionCount=4,.ppEnabledExtensionNames=de};
    VkDevice device; OK(CreateDevice(physical,&di,NULL,&device));
    DestroyDevice(device,NULL); DestroySurfaceKHR(instance,surface,NULL); DestroyWindow(window); DestroyInstance(instance,NULL);
    puts("NUCLEON_VULKAN_OK device-and-win32-surface"); return 0;
}
