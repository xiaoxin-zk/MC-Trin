{
  inputs,
  callPackage,
  lib,
  symlinkJoin,

  launchEnv ? {},
  ...
}:
let
  enwrap = callPackage ./enwrap.nix { inherit inputs launchEnv; };
  desktop = callPackage ./desktop.nix { inherit inputs; };
in
  symlinkJoin {
    name = "mctrin-launcher";
    paths = [
      enwrap desktop
    ];
    meta = {
      description = "MC-Trin: a desktop Minecraft Java Edition launcher focused on Simplified Chinese users.";
      longDescription = ''
        MC-Trin is a free, open-source, ad-free, cross-platform Minecraft Java Edition launcher for searching, installing, and updating mods, modpacks, resource packs, and shaders from Modrinth and CurseForge, with the built-in Lab tools.
      '';
      homepage = "https://github.com/xiaoxin-zk/MC-Trin";
      license = lib.licenses.gpl3Only;
      platforms = [ "x86_64-linux" "aarch64-linux" ];
      mainProgram = "MC-Trin";
    };
  }
