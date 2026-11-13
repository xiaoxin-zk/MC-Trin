{
  inputs,
  makeDesktopItem,
  ...
}:
makeDesktopItem {
  categories = [ "Game" ];
  desktopName = "MC-Trin";
  exec = "MC-Trin";
  icon = "${../apps/app/icons/icon.png}";
  mimeTypes = [
    "application/x-modrinth-modpack+zip"
    "x-scheme-handler/mctrin"
  ];
  name = "MC-Trin";
  startupWMClass = "MC-Trin";
  terminal = false;
  type = "Application";
}
