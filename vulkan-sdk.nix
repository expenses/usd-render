{ stdenv, vulkan-headers, shaderc }:
stdenv.mkDerivation {
  name = "vulkan-sdk";

  dontUnpack = true;

  # Try and recreate the layer of the lunarg vulkan sdk with a colleciton of includes
  # and the static shaderc libraries.
  installPhase = let
    spirv-reflect = fetchGit {
      url = "https://github.com/KhronosGroup/SPIRV-Reflect";
      rev = "2f7460f0be0f73c9ffde719bc3e924b4250f4d98";
    };
    vma = fetchGit {
      url = "https://github.com/GPUOpen-LibrariesAndSDKs/VulkanMemoryAllocator";
      rev = "d802b362c6c4e0494a05facabad7368df1359272";
    };
  in ''
    mkdir -p $out/include
    ln -s ${shaderc.static}/lib $out/lib
    ln -s ${shaderc.dev}/include/shaderc $out/include/shaderc
    ln -s ${vma}/include $out/include/vma
    ln -s ${spirv-reflect} $out/include/SPIRV-Reflect
    ln -s ${vulkan-headers}/include/vulkan $out/include/vulkan
    ln -s ${vulkan-headers}/include/vk_video $out/include/vk_video
  '';
}
